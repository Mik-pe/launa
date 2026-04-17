//! Extracted application logic for the Launa spa controller.
//!
//! `SpaApp` owns all stateful firmware logic — registration, command tracking,
//! pump timers, hold timers, stale detection, diagnostics, fault handling.
//! It exposes a pure synchronous API that returns `Vec<AppAction>` side effects.
//!
//! The ESP32 `main.rs` becomes thin IO wiring: receive frame → `app.process_frame()`
//! → execute actions. Tests exercise the exact same logic.
//!
//! # Example (desktop test)
//!
//! ```
//! use launa_core::{SpaApp, AppAction};
//! use launa_sim::VirtualClock;
//! use launa_hal::Clock;
//!
//! let clock = Box::leak(Box::new(VirtualClock::new()));
//! let mut app = SpaApp::new(clock);
//!
//! // Process a tick, get actions back
//! let actions = app.tick();
//! for action in actions {
//!     // handle or assert on action
//! }
//! ```

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use launa_hal::{Clock, Timestamp};
use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{Frame, FrameEncoder};
use launa_protocol::registration::{RegistrationAction, RegistrationStateMachine};
use launa_protocol::status::{HeatingMode, PumpState, StatusUpdate, TempRange, TemperatureScale};

// ── Rate limiting ──────────────────────────────────────────────────────

/// Maximum number of MQTT commands allowed per rate-limit window.
/// Protects the spa RS-485 bus from command flooding.
pub const RATE_LIMIT_MAX_COMMANDS: usize = 10;

/// Duration of the rate-limit window in milliseconds.
/// After this window elapses, the command counter resets.
pub const RATE_LIMIT_WINDOW_MS: u64 = 10_000;

/// Tracks command count within a sliding time window.
///
/// Commands exceeding `RATE_LIMIT_MAX_COMMANDS` per `RATE_LIMIT_WINDOW_MS`
/// are dropped to protect the spa RS-485 bus.
///
/// Uses the [`Clock`] trait for time injection, making it fully testable
/// on desktop without `embassy_time::Instant`.
pub struct RateLimiter {
    /// Number of commands seen in the current window.
    count: usize,
    /// Start time of the current window (milliseconds since epoch).
    window_start_ms: u64,
}

impl RateLimiter {
    /// Create a new rate limiter with no commands counted.
    pub const fn new() -> Self {
        RateLimiter {
            count: 0,
            window_start_ms: 0,
        }
    }

    /// Check if a command is allowed under the rate limit.
    ///
    /// Returns `true` if the command should be forwarded, `false` if it
    /// should be dropped. Automatically resets the window when it expires.
    ///
    /// # Arguments
    /// * `now_ms` - Current time in milliseconds from a Clock source.
    pub fn check(&mut self, now_ms: u64) -> bool {
        // Reset window if expired
        if now_ms.saturating_sub(self.window_start_ms) >= RATE_LIMIT_WINDOW_MS {
            self.count = 0;
            self.window_start_ms = now_ms;
        }

        self.count += 1;
        self.count <= RATE_LIMIT_MAX_COMMANDS
    }
}

// ── Time constants ─────────────────────────────────────────────────────

const COMMAND_ACK_TIMEOUT_MS: u64 = 5_000;
const MAX_COMMAND_RETRIES: u8 = 2;
const MAX_PENDING_COMMANDS: usize = 8;
const MAX_COMMAND_QUEUE: usize = 32;

const DEFAULT_PUMP_DURATION_MS: u64 = 20 * 60 * 1000;
const DEFAULT_HOLD_MODE_TIMEOUT_MS: u64 = 60 * 60 * 1000;

const STALE_PROBE_INTERVAL_MS: u64 = 5_000;
const STALE_THRESHOLD_MS: u64 = 30_000;
const REGISTRATION_TIMEOUT_MS: u64 = 5_000;
const DIAGNOSTICS_INTERVAL_MS: u64 = 60_000;
const HEAP_CHECK_INTERVAL_MS: u64 = 30_000;
const HEAP_WARN_THRESHOLD: usize = 4096;
const HEAP_CRIT_THRESHOLD: usize = 1024;

// ── AppAction ──────────────────────────────────────────────────────────

/// Side effects the app logic can request.
///
/// The caller (ESP32 main loop or test harness) is responsible for executing these.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    /// Write encoded frame bytes to UART.
    SendFrame(Vec<u8>),

    /// Publish status state to MQTT.
    PublishState {
        status: StatusUpdate,
        fault: Option<String>,
        recovering_from_stale: bool,
    },

    /// Publish availability status to MQTT.
    PublishAvailability { online: bool },

    /// Publish stale availability to MQTT.
    PublishStaleAvailability,

    /// Publish all HA discovery configs.
    PublishDiscovery,

    /// Publish diagnostics JSON.
    PublishDiagnostics {
        uptime_secs: u64,
        frames_received: u32,
        command_retries: u32,
        command_drops: u32,
    },

    /// Publish an alert.
    PublishAlert { level: String, message: String },

    /// Request OTA firmware update.
    RequestOta { url: String },
}

// ── CommandTracker (clock-based) ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedChange {
    PumpOn { item: ToggleItem },
    PumpOff { item: ToggleItem },
    TemperatureSet { temp: u8 },
    LightToggled { item: ToggleItem, pre_state: bool },
    HoldModeToggled { pre_state: bool },
    HeatingModeToggled { pre_mode: HeatingMode },
    TempRangeToggled { pre_range: TempRange },
}

impl ExpectedChange {
    fn from_command(cmd: &Command, pre_status: &StatusUpdate) -> Option<Self> {
        match cmd {
            Command::ToggleItem(item) => {
                if let Some(idx) = item.pump_index() {
                    let is_on = matches!(pre_status.pumps[idx], PumpState::Low | PumpState::High);
                    Some(if is_on {
                        ExpectedChange::PumpOff { item: *item }
                    } else {
                        ExpectedChange::PumpOn { item: *item }
                    })
                } else if let Some(idx) = item.light_index() {
                    Some(ExpectedChange::LightToggled {
                        item: *item,
                        pre_state: pre_status.lights[idx],
                    })
                } else {
                    match item {
                        ToggleItem::Blower => Some(if pre_status.blower {
                            ExpectedChange::PumpOff { item: *item }
                        } else {
                            ExpectedChange::PumpOn { item: *item }
                        }),
                        ToggleItem::HoldMode => Some(ExpectedChange::HoldModeToggled {
                            pre_state: pre_status.is_hold,
                        }),
                        ToggleItem::HeatingMode => Some(ExpectedChange::HeatingModeToggled {
                            pre_mode: pre_status.heating_mode,
                        }),
                        ToggleItem::TemperatureRange => Some(ExpectedChange::TempRangeToggled {
                            pre_range: pre_status.temp_range,
                        }),
                        _ => None,
                    }
                }
            }
            Command::SetTemperature(temp) => Some(ExpectedChange::TemperatureSet { temp: *temp }),
            _ => None,
        }
    }
}

struct PendingCommand {
    command: Command,
    expected: ExpectedChange,
    sent_at: Timestamp,
    retries: u8,
}

/// Tracks pending commands and verifies them against status updates.
pub struct CommandTracker {
    pending: Vec<PendingCommand>,
    dropped_count: u32,
    retry_count: u32,
}

/// Result of verifying pending commands against a status update.
pub struct VerifyResult {
    /// Commands that timed out and should be retried.
    pub retries: Vec<Command>,
    /// Number of commands that exceeded max retries and were dropped.
    pub dropped: u32,
}

impl CommandTracker {
    pub fn new() -> Self {
        CommandTracker {
            pending: Vec::new(),
            dropped_count: 0,
            retry_count: 0,
        }
    }

    pub fn track(&mut self, command: Command, pre_status: &StatusUpdate, now: Timestamp) {
        if self.pending.len() >= MAX_PENDING_COMMANDS {
            return;
        }
        if let Some(expected) = ExpectedChange::from_command(&command, pre_status) {
            self.pending.push(PendingCommand {
                command,
                expected,
                sent_at: now,
                retries: 0,
            });
        }
    }

    pub fn verify(&mut self, status: &StatusUpdate, now: Timestamp) -> VerifyResult {
        let mut confirmed = Vec::new();
        let mut to_retry = Vec::new();
        let mut dropped_this_call: u32 = 0;

        for i in (0..self.pending.len()).rev() {
            let pending = &self.pending[i];
            let elapsed = now.elapsed_since(pending.sent_at);

            if Self::is_confirmed(&pending.expected, status) {
                confirmed.push(i);
            } else if elapsed >= COMMAND_ACK_TIMEOUT_MS {
                if pending.retries < MAX_COMMAND_RETRIES {
                    to_retry.push(i);
                } else {
                    confirmed.push(i);
                    dropped_this_call += 1;
                }
            }
        }

        let mut retries = Vec::new();
        for &i in &to_retry {
            let pending = &mut self.pending[i];
            pending.retries += 1;
            pending.sent_at = now;
            retries.push(pending.command.clone());
        }

        self.retry_count += retries.len() as u32;

        let mut to_remove = confirmed;
        to_remove.sort();
        for &i in to_remove.iter().rev() {
            self.pending.remove(i);
        }

        self.dropped_count += dropped_this_call;

        VerifyResult {
            retries,
            dropped: dropped_this_call,
        }
    }

    fn is_confirmed(expected: &ExpectedChange, status: &StatusUpdate) -> bool {
        match expected {
            ExpectedChange::PumpOn { item } => {
                if let Some(idx) = item.pump_index() {
                    matches!(status.pumps[idx], PumpState::Low | PumpState::High)
                } else {
                    match item {
                        ToggleItem::Blower => status.blower,
                        _ => false,
                    }
                }
            }
            ExpectedChange::PumpOff { item } => {
                if let Some(idx) = item.pump_index() {
                    status.pumps[idx] == PumpState::Off
                } else {
                    match item {
                        ToggleItem::Blower => !status.blower,
                        _ => false,
                    }
                }
            }
            ExpectedChange::TemperatureSet { temp } => {
                // set_temp is decoded (÷2 for Celsius, ÷1 for Fahrenheit).
                // Multiply back to raw wire value for comparison.
                let divisor: f32 = match status.temperature_scale {
                    TemperatureScale::Celsius => 2.0,
                    TemperatureScale::Fahrenheit => 1.0,
                };
                ((status.set_temp * divisor) as u8) == *temp
            }
            ExpectedChange::LightToggled { item, pre_state } => {
                if let Some(idx) = item.light_index() {
                    status.lights[idx] != *pre_state
                } else {
                    status.lights[0] != *pre_state
                }
            }
            ExpectedChange::HoldModeToggled { pre_state } => status.is_hold != *pre_state,
            ExpectedChange::HeatingModeToggled { pre_mode } => status.heating_mode != *pre_mode,
            ExpectedChange::TempRangeToggled { pre_range } => status.temp_range != *pre_range,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn total_dropped(&self) -> u32 {
        self.dropped_count
    }

    pub fn total_retries(&self) -> u32 {
        self.retry_count
    }

    /// Increment the dropped command counter (e.g. when the command queue is full).
    pub fn record_dropped(&mut self) {
        self.dropped_count += 1;
    }

    /// Reset all tracked state (e.g. on bus reset).
    pub fn reset(&mut self) {
        self.pending.clear();
    }
}

// ── PumpTimer (clock-based) ────────────────────────────────────────────

pub struct PumpTimer {
    pump: ToggleItem,
    started_at: Option<Timestamp>,
    duration_ms: u64,
}

impl PumpTimer {
    pub fn new(pump: ToggleItem) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration_ms: DEFAULT_PUMP_DURATION_MS,
        }
    }

    pub fn start(&mut self, now: Timestamp) -> Command {
        self.started_at = Some(now);
        Command::ToggleItem(self.pump)
    }

    pub fn start_with_minutes(&mut self, minutes: u32, now: Timestamp) -> Command {
        self.duration_ms = minutes as u64 * 60 * 1000;
        self.start(now)
    }

    pub fn cancel(&mut self) {
        self.started_at = None;
    }

    pub fn tick(&mut self, now: Timestamp, current_state: PumpState) -> Option<Command> {
        if let Some(started_at) = self.started_at {
            let is_on = matches!(current_state, PumpState::Low | PumpState::High);
            if !is_on {
                self.started_at = None;
                return None;
            }
            if now.elapsed_since(started_at) >= self.duration_ms {
                self.started_at = None;
                return Some(Command::ToggleItem(self.pump));
            }
        }
        None
    }

    pub fn remaining_ms(&self, now: Timestamp) -> u64 {
        if let Some(started_at) = self.started_at {
            self.duration_ms
                .saturating_sub(now.elapsed_since(started_at))
        } else {
            0
        }
    }

    pub fn is_running(&self) -> bool {
        self.started_at.is_some()
    }
}

pub struct PumpTimerManager {
    timers: [PumpTimer; 6],
}

impl PumpTimerManager {
    pub fn new() -> Self {
        PumpTimerManager {
            timers: [
                PumpTimer::new(ToggleItem::Pump1),
                PumpTimer::new(ToggleItem::Pump2),
                PumpTimer::new(ToggleItem::Pump3),
                PumpTimer::new(ToggleItem::Pump4),
                PumpTimer::new(ToggleItem::Pump5),
                PumpTimer::new(ToggleItem::Pump6),
            ],
        }
    }

    pub fn tick_all(&mut self, now: Timestamp, pumps: &[PumpState; 6]) -> Vec<Command> {
        let mut cmds = Vec::new();
        for (i, timer) in self.timers.iter_mut().enumerate() {
            if let Some(c) = timer.tick(now, pumps[i]) {
                cmds.push(c);
            }
        }
        cmds
    }

    pub fn start_timer(&mut self, pump_index: u8, minutes: u32, now: Timestamp) -> Option<Command> {
        let i = (pump_index as usize).checked_sub(1)?;
        if i >= self.timers.len() {
            return None;
        }
        Some(self.timers[i].start_with_minutes(minutes, now))
    }
}

// ── HoldModeTimer (clock-based) ────────────────────────────────────────

pub struct HoldModeTimer {
    entered_at: Option<Timestamp>,
    timeout_ms: u64,
    /// True after timer fires; prevents re-arming until hold mode is released.
    fired: bool,
}

impl HoldModeTimer {
    pub fn new() -> Self {
        HoldModeTimer {
            entered_at: None,
            timeout_ms: DEFAULT_HOLD_MODE_TIMEOUT_MS,
            fired: false,
        }
    }

    pub fn with_timeout_ms(timeout_ms: u64) -> Self {
        HoldModeTimer {
            entered_at: None,
            timeout_ms,
            fired: false,
        }
    }

    pub fn tick(&mut self, now: Timestamp, is_hold: bool) -> Option<Command> {
        if is_hold {
            if self.fired {
                // Already fired — wait for hold mode to be released before re-arming.
                return None;
            }
            if self.entered_at.is_none() {
                self.entered_at = Some(now);
            } else if now.elapsed_since(self.entered_at.unwrap()) >= self.timeout_ms {
                self.entered_at = None;
                self.fired = true;
                return Some(Command::ToggleItem(ToggleItem::HoldMode));
            }
        } else {
            self.entered_at = None;
            self.fired = false;
        }
        None
    }

    pub fn is_active(&self) -> bool {
        self.entered_at.is_some()
    }

    pub fn remaining_ms(&self, now: Timestamp) -> u64 {
        if let Some(entered_at) = self.entered_at {
            self.timeout_ms
                .saturating_sub(now.elapsed_since(entered_at))
        } else {
            0
        }
    }
}

// ── HeapMonitor (abstracted) ───────────────────────────────────────────

/// Heap monitoring state. The caller provides the free heap value.
pub struct HeapMonitor {
    last_check: Option<Timestamp>,
}

impl HeapMonitor {
    pub fn new() -> Self {
        HeapMonitor { last_check: None }
    }

    /// Check heap usage. Returns `Some(critical)` when a check fires:
    /// - `Some(true)` = critically low (< 1 KiB)
    /// - `Some(false)` = warning (< 4 KiB but >= 1 KiB)
    /// - `None` = not time to check yet, or heap is fine
    pub fn tick(&mut self, now: Timestamp, free_heap: usize) -> Option<bool> {
        let should_check = self.last_check.map_or(true, |last| {
            now.elapsed_since(last) >= HEAP_CHECK_INTERVAL_MS
        });
        if !should_check {
            return None;
        }
        self.last_check = Some(now);

        if free_heap < HEAP_CRIT_THRESHOLD {
            Some(true)
        } else if free_heap < HEAP_WARN_THRESHOLD {
            Some(false)
        } else {
            None
        }
    }
}

// ── SpaApp ─────────────────────────────────────────────────────────────

/// The core application logic, extracted from the ESP32 main loop.
///
/// Owns all stateful logic with zero hardware dependencies. Feed it frames,
/// commands, and periodic ticks — it returns `Vec<AppAction>` side effects
/// for the caller to execute.
pub struct SpaApp<'a> {
    clock: &'a dyn Clock,

    // Registration
    registration: RegistrationStateMachine,
    registration_started_at: Option<Timestamp>,

    // Command tracking
    cmd_tracker: CommandTracker,
    /// Queue of commands waiting for the next Ready window.
    command_queue: Vec<Command>,

    // Timers
    pump_timers: PumpTimerManager,
    hold_timer: HoldModeTimer,
    heap_monitor: HeapMonitor,

    // State tracking
    last_status: Option<StatusUpdate>,
    last_fault: Option<String>,
    client_id: Option<u8>,
    last_status_time: Option<Timestamp>,
    last_probe_time: Option<Timestamp>,
    last_diag_time: Option<Timestamp>,
    was_stale: bool,

    // Counters
    frames_received: u32,
    boot_time: Timestamp,
}

impl<'a> SpaApp<'a> {
    pub fn new(clock: &'a dyn Clock) -> Self {
        let now = clock.now();
        SpaApp {
            clock,
            registration: RegistrationStateMachine::new(),
            registration_started_at: None,
            cmd_tracker: CommandTracker::new(),
            command_queue: Vec::new(),
            pump_timers: PumpTimerManager::new(),
            hold_timer: HoldModeTimer::new(),
            heap_monitor: HeapMonitor::new(),
            last_status: None,
            last_fault: None,
            client_id: None,
            last_status_time: None,
            last_probe_time: None,
            last_diag_time: None,
            was_stale: false,
            frames_received: 0,
            boot_time: now,
        }
    }

    /// Whether the controller has completed registration.
    pub fn is_registered(&self) -> bool {
        self.registration.is_registered()
    }

    /// The assigned client ID, if registered.
    pub fn client_id(&self) -> Option<u8> {
        self.client_id
    }

    /// The last received status, if any.
    pub fn last_status(&self) -> Option<&StatusUpdate> {
        self.last_status.as_ref()
    }

    /// The last fault string, if any.
    pub fn last_fault(&self) -> Option<&str> {
        self.last_fault.as_deref()
    }

    /// Number of commands in the queue waiting for Ready windows.
    pub fn queued_command_count(&self) -> usize {
        self.command_queue.len()
    }

    /// Total dropped commands.
    pub fn total_dropped(&self) -> u32 {
        self.cmd_tracker.total_dropped()
    }

    /// Total command retries.
    pub fn total_retries(&self) -> u32 {
        self.cmd_tracker.total_retries()
    }

    /// Total frames received.
    pub fn frames_received(&self) -> u32 {
        self.frames_received
    }

    /// Whether the spa is currently detected as stale (no status for 30s).
    pub fn is_stale(&self) -> bool {
        self.was_stale
    }

    /// Force registration (for tests).
    pub fn force_registered(&mut self, client_id: u8) {
        self.registration.process([0xFE, 0xBF], &[0x00]);
        self.registration.process([0xFE, 0xBF], &[0x02, client_id]);
        self.client_id = Some(client_id);
    }

    /// Force-reset the registration state machine to WaitingForQuery (for tests).
    /// Useful after injecting rapid NewClientQuery frames that leave the SM
    /// in WaitingForAssignment state with no pending assignment response.
    pub fn force_reset_registration(&mut self) {
        self.registration.reset();
        self.client_id = None;
        self.registration_started_at = None;
    }

    /// Start a pump timer. Returns actions including the toggle-on command.
    pub fn start_pump_timer(&mut self, pump_index: u8, minutes: u32) -> Vec<AppAction> {
        let now = self.clock.now();
        let mut actions = Vec::new();
        if let Some(cmd) = self.pump_timers.start_timer(pump_index, minutes, now) {
            let encoded = encode_command(&cmd);
            actions.push(AppAction::SendFrame(encoded));
        }
        actions
    }

    // ── Main processing methods ─────────────────────────────────────

    /// Process an incoming frame from the spa.
    pub fn process_frame(&mut self, frame: &Frame) -> Vec<AppAction> {
        let now = self.clock.now();
        let mut actions = Vec::new();

        // Handle registration
        if !self.registration.is_registered() {
            let action = self
                .registration
                .process(frame.message_type, &frame.payload);
            match action {
                RegistrationAction::SendIdRequest => {
                    let encoded = FrameEncoder::encode([0xFE, 0xBF], &[0x01, 0x02, 0xF1, 0x73])
                        .expect("registration payload should fit in frame");
                    actions.push(AppAction::SendFrame(encoded));
                    self.registration_started_at = Some(now);
                }
                RegistrationAction::SendIdAck { client_id: id } => {
                    let encoded = FrameEncoder::encode([id, 0xBF], &[0x03])
                        .expect("ack payload should fit in frame");
                    actions.push(AppAction::SendFrame(encoded));
                    self.client_id = Some(id);
                    self.registration_started_at = None;
                }
                RegistrationAction::None => {}
            }
            return actions;
        }

        // Dispatch incoming message
        let message = dispatch_frame(frame);

        match message {
            IncomingMessage::StatusUpdate(status) => {
                self.frames_received += 1;

                // Verify pending commands
                let result = self.cmd_tracker.verify(&status, now);
                for cmd in result.retries {
                    let encoded = encode_command(&cmd);
                    actions.push(AppAction::SendFrame(encoded));
                }

                // Tick pump timers
                let expired = self.pump_timers.tick_all(now, &status.pumps);
                for cmd in expired {
                    let encoded = encode_command(&cmd);
                    actions.push(AppAction::SendFrame(encoded));
                }

                // Hold mode safety timeout
                if let Some(cmd) = self.hold_timer.tick(now, status.is_hold) {
                    let encoded = encode_command(&cmd);
                    actions.push(AppAction::SendFrame(encoded));
                }

                self.last_status = Some(status.clone());
                self.last_status_time = Some(now);
                self.last_probe_time = Some(now);

                let recovering = self.was_stale;
                if recovering {
                    self.was_stale = false;
                }

                actions.push(AppAction::PublishState {
                    status,
                    fault: self.last_fault.clone(),
                    recovering_from_stale: recovering,
                });
            }
            IncomingMessage::Ready => {
                // Dequeue one command or send NothingToSend
                if let Some(cmd) = self.command_queue.pop() {
                    let encoded = encode_command(&cmd);
                    actions.push(AppAction::SendFrame(encoded));
                    if let Some(ref pre_status) = self.last_status {
                        self.cmd_tracker.track(cmd, pre_status, now);
                    }
                } else if let Some(cid) = self.client_id {
                    let cmd = Command::NothingToSend { client_id: cid };
                    let encoded = encode_command(&cmd);
                    actions.push(AppAction::SendFrame(encoded));
                }
            }
            IncomingMessage::NewClientQuery => {
                self.registration.reset();
                self.client_id = None;
                self.command_queue.clear();
                self.cmd_tracker.reset();
            }
            IncomingMessage::ClientIdAssignment { id } => {
                self.client_id = Some(id);
            }
            IncomingMessage::FaultLogResponse(fault_log) => {
                self.last_fault = Some(format!(
                    "{:?} ({}d ago, {}:{:02}, set={})",
                    fault_log.message_code,
                    fault_log.days_ago,
                    fault_log.hour,
                    fault_log.minute,
                    fault_log.set_temperature
                ));
            }
            IncomingMessage::ConfigurationResponse(_)
            | IncomingMessage::InformationResponse(_)
            | IncomingMessage::FilterCyclesResponse(_)
            | IncomingMessage::ControlConfiguration(_)
            | IncomingMessage::PreferencesResponse { .. }
            | IncomingMessage::SetupParametersResponse { .. } => {}
            IncomingMessage::Unknown { .. } => {}
        }

        actions
    }

    /// Handle an incoming MQTT command.
    pub fn on_mqtt_command(&mut self, cmd: Command) -> Vec<AppAction> {
        if self.command_queue.len() >= MAX_COMMAND_QUEUE {
            // Queue full — drop the command and increment the dropped counter
            self.cmd_tracker.record_dropped();
            return Vec::new();
        }
        // Queue command for next Ready window
        self.command_queue.push(cmd);
        Vec::new()
    }

    /// Periodic tick for time-based checks: stale detection, diagnostics,
    /// registration timeout, heap monitoring.
    ///
    /// Call this regularly (e.g., every main loop iteration or every 100ms).
    pub fn tick(&mut self) -> Vec<AppAction> {
        let now = self.clock.now();
        let mut actions = Vec::new();

        // Registration timeout
        if !self.registration.is_registered() {
            if let Some(started) = self.registration_started_at {
                if now.elapsed_since(started) >= REGISTRATION_TIMEOUT_MS {
                    actions.push(AppAction::PublishAlert {
                        level: String::from("warn"),
                        message: String::from("registration_timeout"),
                    });
                    self.registration.reset();
                    self.registration_started_at = None;
                }
            }
        } else {
            self.registration_started_at = None;
        }

        // Stale detection
        if let Some(last) = self.last_status_time {
            let elapsed = now.elapsed_since(last);

            // Probe at 5s intervals
            let should_probe = self
                .last_probe_time
                .map_or(true, |lp| now.elapsed_since(lp) >= STALE_PROBE_INTERVAL_MS);

            if elapsed >= STALE_PROBE_INTERVAL_MS && should_probe {
                // Use lightweight NothingToSend instead of ConfigurationRequest
                // to detect bus activity without triggering heavy full-config response
                if let Some(cid) = self.client_id {
                    let cmd = Command::NothingToSend { client_id: cid };
                    let encoded = encode_command(&cmd);
                    actions.push(AppAction::SendFrame(encoded));
                }
                self.last_probe_time = Some(now);
            }

            // Stale at 30s
            if elapsed >= STALE_THRESHOLD_MS && !self.was_stale {
                self.was_stale = true;
                actions.push(AppAction::PublishAlert {
                    level: String::from("warn"),
                    message: String::from("spa_communication_lost"),
                });
                // Publish stale state if we have a known status
                if let Some(ref stale_status) = self.last_status {
                    actions.push(AppAction::PublishState {
                        status: stale_status.clone(),
                        fault: self.last_fault.clone(),
                        recovering_from_stale: false,
                    });
                    actions.push(AppAction::PublishStaleAvailability);
                }
            }
        }

        // Diagnostics publishing (every 60s)
        let should_diag = self
            .last_diag_time
            .map_or(true, |ld| now.elapsed_since(ld) >= DIAGNOSTICS_INTERVAL_MS);
        if should_diag {
            self.last_diag_time = Some(now);
            let uptime_ms = now.elapsed_since(self.boot_time);
            actions.push(AppAction::PublishDiagnostics {
                uptime_secs: uptime_ms / 1000,
                frames_received: self.frames_received,
                command_retries: self.cmd_tracker.total_retries(),
                command_drops: self.cmd_tracker.total_dropped(),
            });
        }

        actions
    }

    /// Check heap status. The caller provides the current free heap value.
    /// Returns actions for alerts if heap is low.
    pub fn check_heap(&mut self, free_heap: usize) -> Vec<AppAction> {
        let now = self.clock.now();
        let mut actions = Vec::new();
        match self.heap_monitor.tick(now, free_heap) {
            Some(true) => {
                actions.push(AppAction::PublishAlert {
                    level: String::from("error"),
                    message: String::from("heap_critically_low"),
                });
            }
            Some(false) => {
                // Warning only, no alert action needed
            }
            None => {}
        }
        actions
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn encode_command(cmd: &Command) -> Vec<u8> {
    let (msg_type, payload) = cmd.encode();
    FrameEncoder::encode(msg_type, &payload).expect("command payload should fit in frame")
}

// ── Remote Log Buffer ──────────────────────────────────────────────────

/// Maximum number of log entries in the ring buffer.
/// Keep small to avoid heap pressure on 32 KiB ESP32.
pub const REMOTE_LOG_BUF_SIZE: usize = 16;

/// Maximum length of a single log message (bytes). Longer messages are truncated.
pub const MAX_LOG_MESSAGE_LEN: usize = 128;

/// A single captured log entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub level: &'static str,
    pub message: String,
    pub timestamp_ms: u64,
}

/// Ring buffer state for captured log messages.
///
/// Extracted from `app/src/remote_log.rs` with `Clock` trait injection
/// instead of `embassy_time::Instant` for desktop testability.
pub struct RemoteLogBuffer {
    entries: Vec<LogEntry>,
    head: usize,
    len: usize,
    enabled: bool,
}

impl RemoteLogBuffer {
    /// Create a new empty log buffer.
    pub fn new() -> Self {
        RemoteLogBuffer {
            entries: Vec::new(),
            head: 0,
            len: 0,
            enabled: false,
        }
    }

    /// Initialize the buffer with capacity. Must be called once before use.
    pub fn init(&mut self) {
        if self.entries.is_empty() {
            self.entries.reserve_exact(REMOTE_LOG_BUF_SIZE);
        }
    }

    /// Enable or disable log capture.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Whether log capture is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Push a log entry into the ring buffer.
    /// If the buffer is full, the oldest entry is overwritten.
    pub fn push(&mut self, level: &'static str, message: &str, timestamp_ms: u64) {
        if !self.enabled {
            return;
        }

        let truncated: String = message.chars().take(MAX_LOG_MESSAGE_LEN).collect();
        let entry = LogEntry {
            level,
            message: truncated,
            timestamp_ms,
        };

        if self.len < REMOTE_LOG_BUF_SIZE {
            self.entries.push(entry);
            self.len = self.entries.len();
            self.head = self.entries.len() % REMOTE_LOG_BUF_SIZE;
        } else {
            if self.head < self.entries.len() {
                self.entries[self.head] = entry;
            }
            self.head = (self.head + 1) % REMOTE_LOG_BUF_SIZE;
        }
    }

    /// Drain all captured log entries, returning them as a Vec and clearing the buffer.
    /// Entries are returned in chronological order (oldest first).
    pub fn drain(&mut self) -> Vec<LogEntry> {
        if self.len == 0 {
            return Vec::new();
        }

        let capacity = self.entries.len().min(self.len);
        let mut result = Vec::new();
        for i in 0..capacity {
            let idx = (self.head + i) % self.entries.len();
            if idx < self.entries.len() {
                result.push(self.entries[idx].clone());
            }
        }

        self.entries.clear();
        self.head = 0;
        self.len = 0;

        result
    }

    /// Number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for RemoteLogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::vec;
    use launa_protocol::frame::Frame;
    use launa_protocol::status::{TemperatureScale, TimeFormat};
    use launa_sim::VirtualClock;

    fn make_app_with_clock() -> (&'static VirtualClock, SpaApp<'static>) {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let app = SpaApp::new(clock);
        (clock, app)
    }

    fn status_frame() -> Frame {
        let mut payload = vec![0u8; 24];
        payload[2] = 100; // current temp
        payload[20] = 104; // set temp
        Frame {
            message_type: [0xFF, 0xAF],
            payload,
        }
    }

    fn ready_frame() -> Frame {
        Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        }
    }

    fn new_client_query_frame() -> Frame {
        Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x00],
        }
    }

    fn client_id_assignment_frame(id: u8) -> Frame {
        Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02, id],
        }
    }

    // ── RateLimiter tests (VAL-PROTO-013) ──────────────────────────

    #[test]
    fn test_rate_limiter_under_limit_passes() {
        let mut rl = RateLimiter::new();
        // All commands up to RATE_LIMIT_MAX_COMMANDS should pass
        for i in 1..=RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(1_000), "command {} should pass (under limit)", i);
        }
    }

    #[test]
    fn test_rate_limiter_over_limit_rejects() {
        let mut rl = RateLimiter::new();
        // Fill up to the limit
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(1_000));
        }
        // Next command should be rejected
        assert!(!rl.check(1_000), "command beyond limit should be rejected");
    }

    #[test]
    fn test_rate_limiter_window_resets_after_timeout() {
        let mut rl = RateLimiter::new();

        // Fill up to the limit at t=1000ms
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(1_000));
        }
        // Rejected at t=1000ms
        assert!(!rl.check(1_000));

        // Still rejected within the same window (t=5000ms, window not expired)
        assert!(!rl.check(5_000));

        // After window expires (RATE_LIMIT_WINDOW_MS = 10_000),
        // t=11000ms is 10000ms after window_start_ms=1000, so window resets
        assert!(rl.check(11_000), "command should pass after window expires");

        // Count should have reset — we can send RATE_LIMIT_MAX_COMMANDS - 1 more
        for i in 1..RATE_LIMIT_MAX_COMMANDS {
            assert!(
                rl.check(11_000),
                "command {} after window reset should pass",
                i
            );
        }
        // Next one should be rejected again
        assert!(
            !rl.check(11_000),
            "should be rejected after filling new window"
        );
    }

    #[test]
    fn test_rate_limiter_burst_of_max_plus_one_rejects_last() {
        let mut rl = RateLimiter::new();

        // Send exactly RATE_LIMIT_MAX_COMMANDS + 1 commands in a burst
        let mut passed = 0usize;
        let mut rejected = 0usize;
        for _ in 0..=RATE_LIMIT_MAX_COMMANDS {
            if rl.check(1_000) {
                passed += 1;
            } else {
                rejected += 1;
            }
        }

        assert_eq!(
            passed, RATE_LIMIT_MAX_COMMANDS,
            "exactly RATE_LIMIT_MAX_COMMANDS should pass"
        );
        assert_eq!(rejected, 1, "exactly 1 command should be rejected");
    }

    #[test]
    fn test_rate_limiter_new_starts_at_zero() {
        let rl = RateLimiter::new();
        assert_eq!(rl.count, 0);
        assert_eq!(rl.window_start_ms, 0);
    }

    #[test]
    fn test_rate_limiter_first_check_passes() {
        let mut rl = RateLimiter::new();
        assert!(rl.check(0));
    }

    #[test]
    fn test_rate_limiter_window_boundary_exact() {
        let mut rl = RateLimiter::new();

        // First command at t=0 starts the window
        assert!(rl.check(0));

        // Fill the rest
        for _ in 1..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(0));
        }
        assert!(!rl.check(0)); // over limit

        // Exactly at window boundary (RATE_LIMIT_WINDOW_MS) should reset
        assert!(
            rl.check(RATE_LIMIT_WINDOW_MS),
            "exactly at window boundary should reset and pass"
        );
    }

    #[test]
    fn test_rate_limiter_window_just_before_boundary_does_not_reset() {
        let mut rl = RateLimiter::new();

        // First command at t=0
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(0));
        }
        assert!(!rl.check(0)); // over limit

        // Just before boundary — window NOT expired
        assert!(
            !rl.check(RATE_LIMIT_WINDOW_MS - 1),
            "one ms before window boundary should still reject"
        );
    }

    #[test]
    fn test_rate_limiter_multiple_window_cycles() {
        let mut rl = RateLimiter::new();

        // Window 1: t=0
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(0));
        }
        assert!(!rl.check(0));

        // Window 2: t=RATE_LIMIT_WINDOW_MS
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(RATE_LIMIT_WINDOW_MS));
        }
        assert!(!rl.check(RATE_LIMIT_WINDOW_MS));

        // Window 3: t=2*RATE_LIMIT_WINDOW_MS
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(2 * RATE_LIMIT_WINDOW_MS));
        }
        assert!(!rl.check(2 * RATE_LIMIT_WINDOW_MS));
    }

    #[test]
    fn test_rate_limiter_rejects_continuous_after_limit() {
        let mut rl = RateLimiter::new();

        // Fill limit
        for _ in 0..RATE_LIMIT_MAX_COMMANDS {
            assert!(rl.check(100));
        }

        // Multiple rejections in a row
        for i in 0..5 {
            assert!(
                !rl.check(100 + i),
                "continuous command {} should be rejected",
                i
            );
        }
    }

    // ── SpaApp tests ───────────────────────────────────────────────

    #[test]
    fn test_spa_app_new() {
        let (_clock, app) = make_app_with_clock();
        assert!(!app.is_registered());
        assert!(app.last_status().is_none());
        assert!(app.client_id().is_none());
        assert_eq!(app.frames_received(), 0);
    }

    #[test]
    fn test_registration_flow() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;

        // Send NewClientQuery → should request ID
        let actions = app.process_frame(&new_client_query_frame());
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            AppAction::SendFrame(_bytes) => {
                // Frame is HDLC-encoded, just check it's a non-empty frame
            }
            _ => panic!("Expected SendFrame"),
        }
        assert!(!app.is_registered());

        // Send ClientIdAssignment
        let actions = app.process_frame(&client_id_assignment_frame(0x05));
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            AppAction::SendFrame(_bytes) => {
                // ACK frame
            }
            _ => panic!("Expected SendFrame"),
        }
        assert!(app.is_registered());
        assert_eq!(app.client_id(), Some(0x05));
    }

    #[test]
    fn test_status_update() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        let actions = app.process_frame(&status_frame());
        assert_eq!(app.frames_received(), 1);

        // Should have a PublishState action
        let has_state = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishState { .. }));
        assert!(has_state);
    }

    #[test]
    fn test_command_queued_and_sent_on_ready() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // First, get a status so the tracker has a pre_status
        app.process_frame(&status_frame());

        // Queue a command
        let cmd_actions = app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert!(cmd_actions.is_empty()); // Command is queued, not sent immediately
        assert_eq!(app.queued_command_count(), 1);

        // Ready arrives → command is dequeued and sent
        let actions = app.process_frame(&ready_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send);
        assert_eq!(app.queued_command_count(), 0);
    }

    #[test]
    fn test_nothing_to_send_on_ready() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        let actions = app.process_frame(&ready_frame());
        // Should send NothingToSend
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send);
    }

    #[test]
    fn test_stale_detection() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get a status
        app.process_frame(&status_frame());

        // Advance past stale threshold
        clock.advance_ms(31_000);

        let actions = app.tick();

        // Should have stale alert and stale availability
        let has_alert = actions.iter().any(|a| {
            matches!(a, AppAction::PublishAlert { message, .. } if message == "spa_communication_lost")
        });
        assert!(has_alert);
        assert!(app.is_stale());
    }

    #[test]
    fn test_stale_recovery() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get a status
        app.process_frame(&status_frame());

        // Go stale
        clock.advance_ms(31_000);
        app.tick();
        assert!(app.is_stale());

        // Receive a new status → should recover
        let actions = app.process_frame(&status_frame());
        assert!(!app.is_stale());

        let recovering = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishState {
                    recovering_from_stale: true,
                    ..
                }
            )
        });
        assert!(recovering);
    }

    #[test]
    fn test_registration_timeout() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;

        // Start registration
        app.process_frame(&new_client_query_frame());

        // Advance past timeout
        clock.advance_ms(6_000);

        let actions = app.tick();
        let has_alert = actions.iter().any(|a| {
            matches!(a, AppAction::PublishAlert { message, .. } if message == "registration_timeout")
        });
        assert!(has_alert);
        assert!(!app.is_registered());
    }

    #[test]
    fn test_diagnostics_periodic() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;

        // Advance past diagnostics interval
        clock.advance_ms(61_000);

        let actions = app.tick();
        let has_diag = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishDiagnostics { .. }));
        assert!(has_diag);
    }

    #[test]
    fn test_hold_mode_timer() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get a status without hold mode
        app.process_frame(&status_frame());

        // Create a status with hold mode (payload[0] == 0x05 means is_hold)
        let mut hold_frame = status_frame();
        hold_frame.payload[0] = 0x05;
        app.process_frame(&hold_frame);

        // Advance past hold timeout (60 min)
        clock.advance_ms(61 * 60 * 1000);

        // Send another status with hold still active → timer fires
        let actions = app.process_frame(&hold_frame);
        // The hold timer should have fired a toggle command
        assert!(actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))));

        // Bug 1 fix: after firing, subsequent ticks with is_hold=true should NOT re-fire
        clock.advance_ms(5_000);
        let actions2 = app.process_frame(&hold_frame);
        let has_send2 = actions2
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            !has_send2,
            "hold timer should NOT re-fire while hold mode is still active after firing"
        );

        // Advance more time — still should not re-fire
        clock.advance_ms(61 * 60 * 1000);
        let actions3 = app.process_frame(&hold_frame);
        let has_send3 = actions3
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            !has_send3,
            "hold timer should NOT re-fire even after another full timeout period"
        );

        // Now release hold mode — timer should re-arm
        let release_frame = status_frame(); // is_hold = false
        app.process_frame(&release_frame);

        // Re-enter hold mode
        app.process_frame(&hold_frame);

        // Advance past timeout again → should fire again
        clock.advance_ms(61 * 60 * 1000);
        let actions4 = app.process_frame(&hold_frame);
        assert!(
            actions4
                .iter()
                .any(|a| matches!(a, AppAction::SendFrame(_))),
            "hold timer should fire again after hold mode was released and re-entered"
        );
    }

    #[test]
    fn test_pump_timer_expiry() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Start pump 1 timer for 1 minute
        let actions = app.start_pump_timer(1, 1);
        assert!(actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))));

        // Get a status with pump running
        let mut status = status_frame();
        status.payload[11] = 0x01; // Pump 1 = Low
        app.process_frame(&status);

        // Advance past timer
        clock.advance_ms(61_000);

        // Next status should trigger auto-off
        let actions = app.process_frame(&status);
        assert!(actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))));
    }

    #[test]
    fn test_command_tracker_confirm() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&status_frame());

        // Queue and send toggle on Ready
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.process_frame(&ready_frame());

        assert_eq!(app.queued_command_count(), 0);

        // Status comes back with pump on → command confirmed
        let mut new_status = status_frame();
        new_status.payload[11] = 0x01; // Pump 1 = Low
        let _actions = app.process_frame(&new_status);

        // No retry actions
        assert_eq!(app.total_retries(), 0);
        assert_eq!(app.total_dropped(), 0);
    }

    #[test]
    fn test_command_tracker_timeout_retry() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status (pump off)
        app.process_frame(&status_frame());

        // Queue and send toggle on Ready
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.process_frame(&ready_frame());

        // Advance past timeout (5s) but don't change status
        clock.advance_ms(6_000);

        // Same status arrives → pump still off → timeout triggers retry
        let _actions = app.process_frame(&status_frame());
        assert!(app.total_retries() > 0);
    }

    #[test]
    fn test_bus_reset_reregistration() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Bus reset — resets registration state, no frames sent
        let actions = app.process_frame(&new_client_query_frame());
        assert!(!app.is_registered());
        assert_eq!(app.client_id(), None);
        // When already registered, the frame goes through dispatch, which
        // resets registration. No SendFrame is produced at this point.
        // The next NewClientQuery from the spa will trigger re-registration.
        assert!(actions.is_empty());

        // Next NewClientQuery starts re-registration
        let actions = app.process_frame(&new_client_query_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send);
    }

    #[test]
    fn test_fault_log_captured() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get a status first
        app.process_frame(&status_frame());

        // Simulate fault log response
        let fault_frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![
                0x28, 0x03, 0x01, 0x1B, 0x02, 0x0E, 0x1E, 0x04, 0x68, 0x68, 0x66,
            ],
        };
        app.process_frame(&fault_frame);
        assert!(app.last_fault().is_some());
    }

    #[test]
    fn test_heap_critical_alert() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;

        // Advance past check interval
        clock.advance_ms(61_000);

        let actions = app.check_heap(500); // Very low
        let has_alert = actions.iter().any(|a| {
            matches!(a, AppAction::PublishAlert { message, .. } if message == "heap_critically_low")
        });
        assert!(has_alert);
    }

    #[test]
    fn test_ready_window_queues_multiple() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&status_frame());

        // Queue 3 commands
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump3));
        assert_eq!(app.queued_command_count(), 3);

        // First Ready → send pump1
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), 2);

        // Second Ready → send pump2
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), 1);

        // Third Ready → send pump3
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), 0);
    }

    #[test]
    fn test_command_queue_cap() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Fill the queue up to MAX_COMMAND_QUEUE
        for _ in 0..MAX_COMMAND_QUEUE {
            app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        }
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 0);

        // Next command should be dropped
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 1);

        // Queue another — also dropped
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump3));
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 2);

        // Drain one via Ready, then queue should accept again
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE - 1);

        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 2); // no new drops
    }

    // ── Temperature confirmation tests (Celsius bug fix) ───────────

    /// Helper: build a StatusUpdate with explicit scale and set_temp.
    fn make_status(set_temp: f32, scale: TemperatureScale) -> StatusUpdate {
        StatusUpdate {
            current_temp: Some(38.0),
            set_temp,
            hour: 0,
            minute: 0,
            heating_mode: HeatingMode::Ready,
            temperature_scale: scale,
            time_format: TimeFormat::Hour24,
            filter_mode: 0,
            is_heating: false,
            temp_range: TempRange::High,
            pumps: [PumpState::Off; 6],
            circ_pump: false,
            blower: false,
            mister: false,
            lights: [false; 2],
            is_priming: false,
            is_hold: false,
            notification_type: 0,
            panel_locked: false,
            settings_lock: false,
            m8_cycle_time: 0,
            sensor_a_temp: Some(set_temp),
            sensor_b_temp: None,
            hold_timer_minutes: None,
        }
    }

    /// Helper: build a Celsius StatusUpdate from a raw wire value.
    /// Raw 76 → set_temp=38.0, raw 77 → set_temp=38.5, etc.
    fn make_celsius_status(raw_set_temp: u8) -> StatusUpdate {
        make_status(raw_set_temp as f32 / 2.0, TemperatureScale::Celsius)
    }

    /// Helper: build a Fahrenheit StatusUpdate from a raw wire value.
    fn make_fahrenheit_status(raw_set_temp: u8) -> StatusUpdate {
        make_status(raw_set_temp as f32, TemperatureScale::Fahrenheit)
    }

    /// Test is_confirmed directly for Fahrenheit.
    #[test]
    fn test_temperature_confirm_fahrenheit() {
        let expected = ExpectedChange::TemperatureSet { temp: 104 };
        let status = make_fahrenheit_status(104);
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Celsius SetTemperature(76) confirmed when status.set_temp=38.0 (raw 76/2).
    #[test]
    fn test_temperature_confirm_celsius_even_raw() {
        let expected = ExpectedChange::TemperatureSet { temp: 76 };
        let status = make_celsius_status(76); // set_temp = 38.0
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Celsius SetTemperature(77) confirmed when status.set_temp=38.5 (raw 77/2).
    #[test]
    fn test_temperature_confirm_celsius_odd_raw() {
        let expected = ExpectedChange::TemperatureSet { temp: 77 };
        let status = make_celsius_status(77); // set_temp = 38.5
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Celsius boundary odd value: SetTemperature(83) → set_temp=41.5.
    #[test]
    fn test_temperature_confirm_celsius_boundary_odd() {
        let expected = ExpectedChange::TemperatureSet { temp: 83 };
        let status = make_celsius_status(83); // set_temp = 41.5
        assert!(CommandTracker::is_confirmed(&expected, &status));
    }

    /// Temperature mismatch does NOT confirm; triggers retry in both scales.
    #[test]
    fn test_temperature_mismatch_triggers_retry_both_scales() {
        use launa_hal::Clock;

        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut tracker = CommandTracker::new();
        let now = clock.now();

        // Fahrenheit: SetTemperature(104), but spa reports set_temp=100
        let pre_status_f = make_fahrenheit_status(100);
        tracker.track(Command::SetTemperature(104), &pre_status_f, now);

        // Advance past ACK timeout
        clock.advance_ms(COMMAND_ACK_TIMEOUT_MS + 1);
        let now_after = clock.now();

        let status_f = make_fahrenheit_status(100);
        let result = tracker.verify(&status_f, now_after);
        assert_eq!(result.retries.len(), 1);
        assert_eq!(result.dropped, 0);

        // Celsius: SetTemperature(76), but spa reports set_temp=74/2=37.0
        let mut tracker_c = CommandTracker::new();
        clock.advance_ms(10_000); // ensure fresh timestamps
        let now2 = clock.now();

        let pre_status_c = make_celsius_status(74); // 37.0
        tracker_c.track(Command::SetTemperature(76), &pre_status_c, now2);

        clock.advance_ms(COMMAND_ACK_TIMEOUT_MS + 1);
        let now2_after = clock.now();

        let status_c = make_celsius_status(74); // 37.0 — mismatch
        let result_c = tracker_c.verify(&status_c, now2_after);
        assert_eq!(result_c.retries.len(), 1);
        assert_eq!(result_c.dropped, 0);
    }

    // ── Stale probe lightweight command tests ───────────────────────

    /// Helper: extract all SendFrame payloads from a list of actions.
    fn collect_sent_frames(actions: &[AppAction]) -> Vec<&Vec<u8>> {
        actions
            .iter()
            .filter_map(|a| match a {
                AppAction::SendFrame(data) => Some(data),
                _ => None,
            })
            .collect()
    }

    /// VAL-CORE-006: Stale probe must NOT contain ConfigurationRequest bytes.
    #[test]
    fn test_stale_probe_not_configuration_request() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status to establish last_status_time
        app.process_frame(&status_frame());

        // Advance past probe interval (5s)
        clock.advance_ms(6_000);
        let actions = app.tick();

        let frames = collect_sent_frames(&actions);
        // The ConfigurationRequest encodes as [0x0A, 0xBF, 0x04] in the raw frame.
        // Check that no sent frame contains these bytes in sequence.
        let config_req_pattern: &[u8] = &[0x0A, 0xBF, 0x04];
        for frame in &frames {
            // Scan for the 3-byte sequence (may be HDLC-stuffed, but the
            // payload bytes before stuffing must not be [0x04] when msg_type
            // is [0x0A, 0xBF]).
            assert!(
                !contains_sequence(frame, config_req_pattern),
                "stale probe should NOT contain ConfigurationRequest bytes [0x0A, 0xBF, 0x04], got {:?}",
                frame
            );
        }
    }

    /// VAL-CORE-007: Stale probe uses a lightweight command (NothingToSend).
    #[test]
    fn test_stale_probe_uses_lightweight_command() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&status_frame());

        // Advance past probe interval
        clock.advance_ms(6_000);
        let actions = app.tick();

        let frames = collect_sent_frames(&actions);
        assert!(
            !frames.is_empty(),
            "stale probe should send at least one frame"
        );

        // The probe should be a NothingToSend: msg_type=[client_id, 0xBF], payload=[0x07]
        let expected = {
            let (mt, payload) = Command::NothingToSend { client_id: 0x03 }.encode();
            FrameEncoder::encode(mt, &payload).expect("encode should succeed")
        };
        assert!(
            frames.iter().any(|f| *f == &expected),
            "stale probe should send NothingToSend, got frames: {:?}",
            frames
        );
    }

    /// VAL-CORE-008: Stale probes fire at 5-second intervals.
    #[test]
    fn test_stale_probe_interval_preserved() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        app.process_frame(&status_frame());

        // Advance to 6s → first probe
        clock.advance_ms(6_000);
        let actions1 = app.tick();
        assert!(
            collect_sent_frames(&actions1).iter().any(|f| !f.is_empty()),
            "first probe should fire after 5s+"
        );

        // Advance only 3s → no probe yet (interval is 5s)
        clock.advance_ms(3_000);
        let actions2 = app.tick();
        let frames2 = collect_sent_frames(&actions2);
        assert!(
            frames2.is_empty(),
            "no probe should fire at 3s after last probe"
        );

        // Advance to 5s total since last probe → second probe
        clock.advance_ms(2_000);
        let actions3 = app.tick();
        assert!(
            collect_sent_frames(&actions3).iter().any(|f| !f.is_empty()),
            "second probe should fire at 5s after last probe"
        );
    }

    /// VAL-CORE-009: 30-second stale threshold and alert unchanged.
    #[test]
    fn test_stale_threshold_unchanged_after_probe_fix() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        app.process_frame(&status_frame());

        // Advance past stale threshold (30s)
        clock.advance_ms(31_000);
        let actions = app.tick();

        // Should have stale alert
        let has_alert = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
            )
        });
        assert!(has_alert, "should publish stale alert at 30s");

        // Should have stale availability
        let has_stale_avail = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishStaleAvailability));
        assert!(has_stale_avail, "should publish stale availability at 30s");

        assert!(app.is_stale());
    }

    /// Helper: check if a byte slice contains a subsequence.
    fn contains_sequence(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.len() > haystack.len() {
            return false;
        }
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    // ── Pump timer cancellation tests (VAL-BM-009, VAL-BM-010) ──────

    /// VAL-BM-009: PumpTimer::tick() with !is_on cancels timer and doesn't re-fire.
    /// Start the timer, feed an Off status (pump not running), verify:
    /// 1. Timer returns None on the tick where !is_on is detected
    /// 2. Timer is no longer running after cancellation
    /// 3. Subsequent ticks with past-duration elapsed time do NOT fire
    #[test]
    fn test_pump_timer_cancel_on_pump_off() {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut timer = PumpTimer::new(ToggleItem::Pump1);

        // Start timer for 1 minute
        let now = clock.now();
        let cmd = timer.start(now);
        assert!(matches!(cmd, Command::ToggleItem(ToggleItem::Pump1)));
        assert!(timer.is_running());

        // Advance past the timer duration
        clock.advance_ms(61_000);
        let later = clock.now();

        // Pump is OFF — timer should cancel and return None
        let result = timer.tick(later, PumpState::Off);
        assert!(
            result.is_none(),
            "tick with !is_on should return None, not a toggle command"
        );
        assert!(
            !timer.is_running(),
            "timer should not be running after cancellation"
        );

        // Advance well past the duration — should NOT re-fire
        clock.advance_ms(120_000);
        let much_later = clock.now();
        let result2 = timer.tick(much_later, PumpState::Off);
        assert!(
            result2.is_none(),
            "cancelled timer should never re-fire, even after duration passes"
        );
        assert!(!timer.is_running());
    }

    /// VAL-BM-009 extended: After cancellation, restarting the timer works correctly
    /// and fires at the new duration.
    #[test]
    fn test_pump_timer_cancel_then_restart() {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let mut timer = PumpTimer::new(ToggleItem::Pump1);

        // Start timer for 1 minute
        let now = clock.now();
        timer.start_with_minutes(1, now);
        assert!(timer.is_running());

        // Cancel by feeding Off
        clock.advance_ms(30_000);
        let result = timer.tick(clock.now(), PumpState::Off);
        assert!(result.is_none());
        assert!(!timer.is_running());

        // Restart the timer for 1 minute
        clock.advance_ms(10_000);
        let restart_time = clock.now();
        timer.start_with_minutes(1, restart_time);
        assert!(timer.is_running());

        // Feed pump running status — timer is running but not expired yet
        let result = timer.tick(clock.now(), PumpState::Low);
        assert!(result.is_none(), "timer should not fire before duration");

        // Advance past 1 minute duration
        clock.advance_ms(61_000);
        let result = timer.tick(clock.now(), PumpState::Low);
        assert!(
            result.is_some(),
            "restarted timer should fire at new duration"
        );
        assert!(matches!(
            result,
            Some(Command::ToggleItem(ToggleItem::Pump1))
        ));
        assert!(!timer.is_running());
    }

    /// VAL-BM-010: SpaApp cancels pump timer on Off status — no toggle-off SendFrame.
    /// Start pump timer → pump turns off externally via status frame →
    /// advance past timer duration → verify NO auto-off toggle is sent.
    #[test]
    fn test_spaapp_pump_timer_cancel_on_external_off() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Start pump 1 timer for 1 minute
        let actions = app.start_pump_timer(1, 1);
        assert!(
            actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "start_pump_timer should return toggle-on action"
        );

        // Feed status with pump running (timer ticks normally)
        let mut status_on = status_frame();
        status_on.payload[11] = 0x01; // Pump 1 = Low
        app.process_frame(&status_on);

        // Now pump turns off externally (someone pressed the physical button)
        let status_off = status_frame(); // pump 1 = Off (default)
        let actions = app.process_frame(&status_off);

        // Check no auto-off toggle from the pump timer — it should be cancelled
        // The process_frame may return PublishState and other actions, but NO SendFrame
        // from the pump timer auto-off (the timer was cancelled because pump is off)
        let timer_toggle = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        // Note: there might be SendFrame from command tracker retries, but NOT from
        // pump timer. Let's verify by advancing past duration and checking no fire.
        assert!(
            !timer_toggle
                || !actions.iter().any(|a| {
                    // Check if there's a toggle-off frame that isn't from command tracker
                    matches!(a, AppAction::SendFrame(_))
                }),
            "no auto-off toggle should be sent when pump is externally turned off"
        );

        // Advance past the timer duration
        clock.advance_ms(61_000);

        // Feed another status with pump still off — should NOT trigger auto-off
        let actions = app.process_frame(&status_off);
        let has_toggle_off = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            !has_toggle_off,
            "no toggle-off SendFrame should appear after timer was cancelled by external Off"
        );
    }

    // ── WiFi reconnection lifecycle test (VAL-PL-004) ──────────────

    /// Simulates a WiFi reconnection scenario:
    /// 1. Normal operation with regular status updates
    /// 2. Bus silence (simulating WiFi/spa communication loss)
    /// 3. Stale detection at 30s threshold
    /// 4. Stale probe messages sent while stale
    /// 5. Communication resumes → recovery flag set
    /// 6. Normal operation resumes
    #[test]
    fn test_wifi_reconnection_lifecycle() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Phase 1: Normal operation — process a few status updates
        for _ in 0..3 {
            let actions = app.process_frame(&status_frame());
            assert!(
                actions
                    .iter()
                    .any(|a| matches!(a, AppAction::PublishState { .. })),
                "normal status should produce PublishState"
            );
            clock.advance_ms(1_000);
        }
        assert!(!app.is_stale());
        assert_eq!(app.frames_received(), 3);

        // Phase 2: Bus silence — advance to 6s (triggers first stale probe)
        clock.advance_ms(6_000);
        let actions = app.tick();
        let has_probe = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_probe, "should send stale probe at 6s");
        assert!(!app.is_stale(), "should not be stale yet at 6s");

        // Phase 3: Continue silence past 30s threshold
        clock.advance_ms(25_000); // total 31s since last status
        let actions = app.tick();

        // Verify stale alert published
        let has_alert = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
            )
        });
        assert!(has_alert, "should publish stale alert at 30s");

        // Verify stale availability published
        let has_stale_avail = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishStaleAvailability));
        assert!(has_stale_avail, "should publish stale availability at 30s");
        assert!(app.is_stale(), "should be stale after 30s silence");

        // Phase 4: More probes while stale
        clock.advance_ms(10_000); // now 41s since last status
        let actions = app.tick();
        let has_probe2 = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_probe2, "should continue probing while stale");

        // Phase 5: Communication resumes — process a new status
        let actions = app.process_frame(&status_frame());
        assert!(
            !app.is_stale(),
            "should no longer be stale after status received"
        );

        // Verify recovery flag is set
        let has_recovery = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishState {
                    recovering_from_stale: true,
                    ..
                }
            )
        });
        assert!(
            has_recovery,
            "first status after stale should have recovery flag"
        );

        // Phase 6: Normal operation resumes
        clock.advance_ms(2_000);
        let actions = app.process_frame(&status_frame());
        let no_recovery = actions.iter().all(|a| {
            !matches!(
                a,
                AppAction::PublishState {
                    recovering_from_stale: true,
                    ..
                }
            )
        });
        assert!(
            no_recovery,
            "subsequent statuses should not have recovery flag"
        );
    }

    // ── VAL-PROTO-005: Command queuing until Ready frame ───────────

    /// Helper: create a Ready frame with a specific client ID.
    /// After registration, the spa sends <ID> BF 06 instead of 10 BF 06.
    fn registered_ready_frame(client_id: u8) -> Frame {
        Frame {
            message_type: [client_id, 0xBF],
            payload: vec![0x06],
        }
    }

    /// VAL-PROTO-005: Commands queued via on_mqtt_command() must not produce
    /// a SendFrame action until a Ready frame is received.
    #[test]
    fn test_no_send_frame_until_ready() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        let actions = app.process_frame(&status_frame());
        // Status produces PublishState, but no SendFrame
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(!has_send, "status frame alone should not produce SendFrame");

        // Queue a command
        let cmd_actions = app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert!(
            cmd_actions.is_empty(),
            "on_mqtt_command should return no actions"
        );
        assert_eq!(app.queued_command_count(), 1);

        // Send another status frame — still no SendFrame (no Ready yet)
        let actions2 = app.process_frame(&status_frame());
        let has_send2 = actions2
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            !has_send2,
            "status frame should not dequeue commands — only Ready does"
        );
        assert_eq!(
            app.queued_command_count(),
            1,
            "command should still be in queue"
        );

        // Send yet another status — command still held
        let actions3 = app.process_frame(&status_frame());
        let has_send3 = actions3
            .iter()
            .any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            !has_send3,
            "command should remain queued across multiple status frames"
        );
        assert_eq!(app.queued_command_count(), 1);
    }

    /// VAL-PROTO-005: After Ready frame, queued command is dequeued and sent.
    /// Verifies the exact sequence: queue → no send → Ready → send.
    #[test]
    fn test_command_dequeued_only_on_ready() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Get initial status
        app.process_frame(&status_frame());

        // Queue command
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Multiple status frames — no dequeue
        for _ in 0..5 {
            app.process_frame(&status_frame());
            assert_eq!(app.queued_command_count(), 1);
        }

        // Now send Ready → command dequeued
        let actions = app.process_frame(&ready_frame());
        let send_frames: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, AppAction::SendFrame(_)))
            .collect();
        assert_eq!(
            send_frames.len(),
            1,
            "Ready should dequeue exactly one command"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    /// VAL-PROTO-005: Multiple commands dequeued one at a time per Ready frame.
    /// Each Ready frame dequeues exactly one command from the front of the queue.
    #[test]
    fn test_commands_dequeued_one_per_ready() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        // Queue 3 commands
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump3));
        assert_eq!(app.queued_command_count(), 3);

        // Status frames don't dequeue
        app.process_frame(&status_frame());
        assert_eq!(app.queued_command_count(), 3);

        // Ready 1 → dequeue first command
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), 2);

        // Another status — no additional dequeue
        app.process_frame(&status_frame());
        assert_eq!(app.queued_command_count(), 2);

        // Ready 2 → dequeue second
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), 1);

        // Ready 3 → dequeue third
        app.process_frame(&ready_frame());
        assert_eq!(app.queued_command_count(), 0);
    }

    /// VAL-PROTO-005: Ready frame with registered client ID also dequeues commands.
    /// After registration, the spa sends <ID> BF 06 instead of 10 BF 06.
    #[test]
    fn test_registered_client_ready_dequeues_command() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x05);

        app.process_frame(&status_frame());
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Use client-ID ready frame instead of generic 10 BF
        let actions = app.process_frame(&registered_ready_frame(0x05));
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            has_send,
            "registered client Ready frame should dequeue command"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    /// VAL-PROTO-005: SetTemperature command is held until Ready and tracked after dequeue.
    #[test]
    fn test_set_temperature_held_until_ready_and_tracked() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        // Queue temperature command
        app.on_mqtt_command(Command::SetTemperature(104));
        assert_eq!(app.queued_command_count(), 1);

        // Status doesn't dequeue
        app.process_frame(&status_frame());
        assert_eq!(app.queued_command_count(), 1);

        // Ready dequeues and starts tracking
        let actions = app.process_frame(&ready_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send);
        assert_eq!(app.queued_command_count(), 0);
        // Tracker should be monitoring the command
        assert!(app.total_dropped() == 0);
    }

    /// VAL-PROTO-005: Ready without queued command sends NothingToSend.
    #[test]
    fn test_ready_without_command_sends_nothing() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        // No command queued → Ready sends NothingToSend
        let actions = app.process_frame(&ready_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            has_send,
            "Ready without queued command should send NothingToSend"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    /// VAL-PROTO-005: Command queued before first status is held, then sent on Ready.
    #[test]
    fn test_command_queued_before_first_status() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Queue command BEFORE any status frame
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Ready arrives before status — command is sent but NOT tracked
        // (no pre_status for tracker). This is expected behavior: the command
        // goes on the wire even if we can't track confirmation.
        let actions = app.process_frame(&ready_frame());
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            has_send,
            "Ready should dequeue command even without prior status"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    // ── RemoteLogBuffer tests (VAL-PROTO-019) ──────────────────────

    #[test]
    fn test_remote_log_buffer_fifo_order() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        buf.push("warn", "first", 1000);
        buf.push("error", "second", 2000);
        buf.push("warn", "third", 3000);

        assert_eq!(buf.len(), 3);
        let entries = buf.drain();
        assert_eq!(entries.len(), 3);
        // FIFO: oldest first
        assert_eq!(entries[0].level, "warn");
        assert_eq!(entries[0].message, "first");
        assert_eq!(entries[1].level, "error");
        assert_eq!(entries[1].message, "second");
        assert_eq!(entries[2].level, "warn");
        assert_eq!(entries[2].message, "third");
    }

    #[test]
    fn test_remote_log_buffer_wrap_around_overwrite() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        // Fill beyond capacity (REMOTE_LOG_BUF_SIZE = 16)
        for i in 0..REMOTE_LOG_BUF_SIZE + 4 {
            buf.push("warn", &format!("msg {}", i), i as u64 * 100);
        }

        assert_eq!(buf.len(), REMOTE_LOG_BUF_SIZE);

        let entries = buf.drain();
        assert_eq!(entries.len(), REMOTE_LOG_BUF_SIZE);
        // First entry should be msg 4 (oldest surviving after overwrite)
        assert_eq!(entries[0].message, "msg 4");
        // Last entry should be msg 19 (most recent)
        assert_eq!(entries[entries.len() - 1].message, "msg 19");
        // Entries should be in chronological order
        for i in 0..entries.len() - 1 {
            assert!(
                entries[i].timestamp_ms <= entries[i + 1].timestamp_ms,
                "entries should be in chronological order"
            );
        }
    }

    #[test]
    fn test_remote_log_buffer_drain_clears() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        buf.push("warn", "test", 1000);
        assert_eq!(buf.len(), 1);
        assert!(!buf.is_empty());

        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);

        // Second drain returns empty
        let entries2 = buf.drain();
        assert!(entries2.is_empty());
    }

    #[test]
    fn test_remote_log_buffer_enable_disable_toggle() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();

        // Disabled by default
        assert!(!buf.is_enabled());

        // Push while disabled — no effect
        buf.push("warn", "should not appear", 1000);
        assert!(buf.is_empty());

        // Enable
        buf.set_enabled(true);
        assert!(buf.is_enabled());

        buf.push("warn", "should appear", 2000);
        assert_eq!(buf.len(), 1);

        // Disable
        buf.set_enabled(false);
        buf.push("error", "should not appear either", 3000);
        assert_eq!(buf.len(), 1); // Still 1, not 2

        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "should appear");
    }

    #[test]
    fn test_remote_log_buffer_message_truncation() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        let long_msg: String = "x".repeat(200);
        buf.push("warn", &long_msg, 1000);

        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].message.len() <= MAX_LOG_MESSAGE_LEN,
            "message should be truncated to MAX_LOG_MESSAGE_LEN"
        );
        // Truncation is by chars, so len may be <= MAX_LOG_MESSAGE_LEN
        assert_eq!(entries[0].message.len(), MAX_LOG_MESSAGE_LEN);
    }

    #[test]
    fn test_remote_log_buffer_empty_drain() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        let entries = buf.drain();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_remote_log_buffer_push_after_drain() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        buf.push("warn", "first batch", 1000);
        let _ = buf.drain();

        // Push after drain should work
        buf.push("error", "second batch", 2000);
        let entries = buf.drain();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "second batch");
    }

    #[test]
    fn test_remote_log_buffer_default() {
        let buf = RemoteLogBuffer::default();
        assert!(buf.is_empty());
        assert!(!buf.is_enabled());
    }

    #[test]
    fn test_remote_log_buffer_exact_capacity() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        // Fill exactly to capacity
        for i in 0..REMOTE_LOG_BUF_SIZE {
            buf.push("warn", &format!("msg {}", i), i as u64);
        }
        assert_eq!(buf.len(), REMOTE_LOG_BUF_SIZE);

        let entries = buf.drain();
        assert_eq!(entries.len(), REMOTE_LOG_BUF_SIZE);
        assert_eq!(entries[0].message, "msg 0");
        assert_eq!(entries[entries.len() - 1].message, "msg 15");
    }

    #[test]
    fn test_remote_log_buffer_multiple_wrap_arounds() {
        let mut buf = RemoteLogBuffer::new();
        buf.init();
        buf.set_enabled(true);

        // Push 3x capacity
        for i in 0..REMOTE_LOG_BUF_SIZE * 3 {
            buf.push("warn", &format!("msg {}", i), i as u64);
        }
        assert_eq!(buf.len(), REMOTE_LOG_BUF_SIZE);

        let entries = buf.drain();
        // Should contain the last REMOTE_LOG_BUF_SIZE entries
        assert_eq!(entries[0].message, "msg 32");
        assert_eq!(entries[entries.len() - 1].message, "msg 47");
    }
}
