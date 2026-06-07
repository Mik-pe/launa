//! Core spa application logic.
//!
//! `SpaApp` owns all stateful firmware logic — registration, command tracking,
//! pump timers, hold timers, stale detection, diagnostics, fault handling.
//! It exposes a pure synchronous API that returns `Vec<AppAction>` side effects.

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use launa_hal::{Clock, Timestamp};
use launa_protocol::command::Command;
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::{Frame, FrameEncoder};
use launa_protocol::registration::{
    RegistrationAction, RegistrationMessage, RegistrationStateMachine,
};
use launa_protocol::status::StatusUpdate;

use crate::actions::AppAction;
use crate::command_tracker::CommandTracker;
use crate::heap_monitor::HeapMonitor;
use crate::rate_log::RateLog;
use crate::timers::{HoldModeTimer, PumpTimerManager};
use crate::types::{
    CTS_LOSS_THRESHOLD_MS, DIAGNOSTICS_INTERVAL_MS, MAX_COMMAND_QUEUE, REGISTRATION_TIMEOUT_MS,
    STALE_PROBE_INTERVAL_MS, STALE_THRESHOLD_MS,
};

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
    command_queue: VecDeque<Command>,

    // Registration responses (ClientIdAck, ExistingClientRequest) are now sent
    // immediately via AppAction::SendFrame rather than queued for the next CTS.
    // This matches real Balboa display panel behavior (acks within ~0.3ms).

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
    /// Frames received while unregistered (for diagnostics).
    unregistered_frames_received: u32,

    /// Track whether we've emitted the "no_registration_query" alert to avoid
    /// flooding. Reset when registration completes or the SM resets.
    no_query_alert_sent: bool,

    /// Rate limiter for the per-frame registration log to avoid flooding
    /// UART and remote log when unregistered.
    reg_log: RateLog,

    boot_time: Timestamp,

    /// Unique client hash for RS-485 channel assignment (2 bytes).
    /// Derived from device-specific data (e.g. ESP32 MAC address) so that
    /// multiple devices on the same bus receive distinct channel IDs.
    client_hash: [u8; 2],

    /// Number of consecutive failed registration attempts (ID request sent
    /// but no ClientIdAssignment received within timeout). After
    /// REGISTRATION_HASH_ROTATE_THRESHOLD attempts, the hash is rotated
    /// to try a different identity on the bus.
    failed_registration_attempts: u8,

    /// Timestamp of the last frame received while unregistered.
    /// Used to detect that the spa bus is alive even though we haven't
    /// completed registration (e.g., spa rebooted and forgot our ID).
    last_unregistered_frame_time: Option<Timestamp>,

    /// Timestamp of the last proactive registration probe (unsolicited ID
    /// request sent because we're unregistered but receiving bus traffic).
    last_registration_probe_time: Option<Timestamp>,

    /// Timestamp of the last CTS (Ready) frame received on our assigned channel.
    /// Used to detect spa reboots (spa forgets clients and stops sending CTS
    /// on our channel).
    last_cts_time: Option<Timestamp>,
}

impl<'a> SpaApp<'a> {
    pub fn new(clock: &'a dyn Clock) -> Self {
        Self::with_client_hash(clock, [0x00, 0x01])
    }

    /// Create a SpaApp with client hash derived from a device ID string.
    ///
    /// Uses FNV-1a 16-bit hashing. Avoids 0x0000 and 0xF173 (Balboa reserved).
    pub fn new_from_device_id(clock: &'a dyn Clock, device_id: &str) -> Self {
        Self::with_client_hash(clock, derive_client_hash(device_id))
    }

    /// Create a new SpaApp with a specific client hash for RS-485 registration.
    ///
    /// The `client_hash` should be derived from unique device identity (e.g.
    /// the ESP32 MAC address) so that multiple devices on the same RS-485 bus
    /// receive distinct channel assignments from the spa controller.
    pub fn with_client_hash(clock: &'a dyn Clock, client_hash: [u8; 2]) -> Self {
        let now = clock.now();
        SpaApp {
            clock,
            registration: RegistrationStateMachine::new(client_hash),
            registration_started_at: None,
            cmd_tracker: CommandTracker::new(),
            command_queue: VecDeque::new(),
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
            unregistered_frames_received: 0,
            no_query_alert_sent: false,
            reg_log: RateLog::new(),
            boot_time: now,
            client_hash,
            failed_registration_attempts: 0,
            last_unregistered_frame_time: None,
            last_registration_probe_time: None,
            last_cts_time: None,
        }
    }

    /// Set a previously-assigned channel for existing client reconnection.
    ///
    /// When set, the next `NewClientQuery` will trigger an
    /// `ExistingClientRequest` (FE BF 04 <channel> <hash>) instead of a
    /// `NewClientResponse`, asking the spa to reassign the same channel.
    pub fn set_previous_channel(&mut self, channel: u8) {
        self.registration =
            RegistrationStateMachine::with_previous_channel(self.client_hash, channel);
    }

    /// Whether the controller has completed registration.
    pub fn is_registered(&self) -> bool {
        self.registration.is_registered()
    }

    /// The assigned client ID, if registered.
    pub fn client_id(&self) -> Option<u8> {
        self.client_id
    }

    /// The current client hash used for RS-485 registration.
    pub fn client_hash(&self) -> [u8; 2] {
        self.client_hash
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

    /// Total frames received (post-registration status frames).
    pub fn frames_received(&self) -> u32 {
        self.frames_received
    }

    /// Frames received while unregistered.
    pub fn unregistered_frames_received(&self) -> u32 {
        self.unregistered_frames_received
    }

    /// Whether the spa is currently detected as stale (no status for 30s).
    pub fn is_stale(&self) -> bool {
        self.was_stale
    }

    /// Registration state as a static string for diagnostics.
    pub fn registration_state_str(&self) -> &'static str {
        match self.registration.state() {
            launa_protocol::registration::RegistrationState::WaitingForQuery => "waiting_for_query",
            launa_protocol::registration::RegistrationState::WaitingForAssignment => {
                "waiting_for_assignment"
            }
            launa_protocol::registration::RegistrationState::WaitingForExistingResponse => {
                "waiting_for_existing_response"
            }
            launa_protocol::registration::RegistrationState::Registered { .. } => "registered",
        }
    }

    /// Force registration (for tests).
    pub fn force_registered(&mut self, client_id: u8) {
        self.registration
            .process(&RegistrationMessage::NewClientQuery);
        self.registration
            .process(&RegistrationMessage::ClientIdAssignment {
                channel: client_id,
                client_hash: self.client_hash,
            });
        self.client_id = Some(client_id);
        self.failed_registration_attempts = 0;
    }

    /// Reset registration state and optionally clear command queues.
    ///
    /// Centralizes the registration reset pattern used on CTS loss, stale
    /// detection, and test helpers. When `clear_commands` is true, also
    /// clears the command queue, tracker, and pump timers.
    fn reset_registration_state(&mut self, clear_commands: bool) {
        self.registration.reset();
        self.client_id = None;
        self.registration_started_at = None;
        self.last_unregistered_frame_time = None;
        self.last_registration_probe_time = None;
        self.last_cts_time = None;
        if clear_commands {
            self.command_queue.clear();
            self.cmd_tracker.reset();
            self.pump_timers.cancel_all();
        }
    }

    /// Force-reset the registration state machine to WaitingForQuery (for tests).
    /// Useful after injecting rapid NewClientQuery frames that leave the SM
    /// in WaitingForAssignment state with no pending assignment response.
    pub fn force_reset_registration(&mut self) {
        self.reset_registration_state(false);
    }

    /// Rotate the client hash to try a different identity on the bus.
    ///
    /// Called after repeated registration failures (timeout waiting for
    /// ClientIdAssignment). XORs the attempt counter into both hash bytes
    /// Force-publish the current state, bypassing change detection.
    ///
    /// Used when a mode toggle (sniff) changes the `sniff_mode`
    /// `sniff_mode` flags in the state JSON but the underlying `StatusUpdate`
    /// hasn't changed. Without this, the MQTT task's change detection would
    /// suppress the publish and the UI would never see the mode change.
    pub fn force_publish(&self) -> Vec<AppAction> {
        let mut actions = Vec::new();
        if let Some(ref status) = self.last_status {
            actions.push(AppAction::PublishState {
                status: status.clone(),
                fault: self.last_fault.clone(),
                recovering_from_stale: false,
                registration_state: self.registration_state_str(),
            });
        }
        actions
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

    /// Process an incoming frame from the spa.
    pub fn process_frame(&mut self, frame: &Frame) -> Vec<AppAction> {
        let now = self.clock.now();
        let mut actions = Vec::new();
        let reg_state = self.registration_state_str();

        // Handle registration when unregistered
        if !self.registration.is_registered() {
            self.unregistered_frames_received += 1;
            self.last_unregistered_frame_time = Some(now);

            match RegistrationMessage::parse(frame.message_type, &frame.payload) {
                Ok(msg) => {
                    let now_secs = now.as_secs() as u32;
                    match self.reg_log.check(now_secs, 5) {
                        Ok(suppressed) => {
                            if suppressed > 0 {
                                log::info!(
                                    "REG: state={:?}, msg={:?} (suppressed {})",
                                    self.registration.state(),
                                    msg,
                                    suppressed,
                                );
                            } else {
                                log::info!(
                                    "REG: state={:?}, msg={:?}",
                                    self.registration.state(),
                                    msg,
                                );
                            }
                        }
                        Err(_) => { /* suppressed */ }
                    }

                    let action = self.registration.process(&msg);
                    match action {
                        RegistrationAction::SendNewClientResponse => {
                            // The sync fast-path in uart_task sends the NewClientResponse
                            // directly when it sees FE BF 00 — no action needed here.
                            self.registration_started_at = Some(now);
                        }
                        RegistrationAction::SendClientIdAck { client_id: id } => {
                            let msg = RegistrationMessage::ClientIdAck { channel: id };
                            match msg.encode() {
                                Ok(encoded) => {
                                    log::info!(
                                        "REG: sending ClientIdAck immediately for channel 0x{:02X}",
                                        id
                                    );
                                    actions.push(AppAction::SendFrame(encoded));
                                    self.client_id = Some(id);
                                    self.registration_started_at = None;
                                    self.failed_registration_attempts = 0;
                                    self.last_unregistered_frame_time = None;
                                    self.last_registration_probe_time = None;
                                }
                                Err(e) => {
                                    log::error!("REG: failed to encode ID ack: {:?}", e);
                                }
                            }
                        }
                        RegistrationAction::SendExistingClientRequest { message } => {
                            match message.encode() {
                                Ok(encoded) => {
                                    log::info!(
                                        "REG: sending existing client request immediately (ch={:?}, hash={:02X}{:02X})",
                                        self.registration.previous_channel(),
                                        self.client_hash[0],
                                        self.client_hash[1],
                                    );
                                    actions.push(AppAction::SendFrame(encoded));
                                    self.registration_started_at = Some(now);
                                }
                                Err(e) => {
                                    log::error!(
                                        "REG: failed to encode existing client request: {:?}",
                                        e
                                    );
                                }
                            }
                        }
                        RegistrationAction::None => {}
                    }
                }
                Err(_) => {
                    // Not a registration message — ignore
                }
            }
            return actions;
        }

        // Dispatch incoming message
        let message = dispatch_frame(frame);

        match message {
            IncomingMessage::StatusUpdate(status) => {
                if self.registration.is_registered() {
                    self.frames_received += 1;
                }

                // Command tracking only works when registered (need client ID)
                if self.registration.is_registered() {
                    let result = self.cmd_tracker.verify(&status, now);
                    for cmd in result.retries {
                        self.command_queue.push_back(cmd);
                    }

                    let expired = self.pump_timers.tick_all(now, &status.pumps);
                    for cmd in expired {
                        self.command_queue.push_back(cmd);
                    }

                    if let Some(cmd) = self.hold_timer.tick(now, status.is_hold) {
                        self.command_queue.push_back(cmd);
                    }
                }

                self.last_status = Some(status.clone());
                self.last_status_time = Some(now);
                self.last_probe_time = Some(now);

                // Only recover from stale when registered — stale detection
                // resets registration, and the recovery flag should only be
                // set once registration is re-established.
                let recovering = self.was_stale && self.registration.is_registered();
                if recovering {
                    self.was_stale = false;
                }

                actions.push(AppAction::PublishState {
                    status,
                    fault: self.last_fault.clone(),
                    recovering_from_stale: recovering,
                    registration_state: reg_state,
                });
            }
            IncomingMessage::Ready { channel }
                // Only handle Ready when registered — commands require a client ID.
                if self.registration.is_registered() => {
                    // Only respond to CTS on our own channel
                    if let Some(my_id) = self.client_id {
                        if channel != my_id {
                            return actions;
                        }
                    }
                    // Record that we received a valid CTS on our channel
                    self.last_cts_time = Some(now);
                    if let Some(cmd) = self.command_queue.pop_front() {
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
            IncomingMessage::Registration(
                launa_protocol::registration::RegistrationMessage::NewClientQuery,
            ) => {
                // Already registered — ignore the periodic query.
                // The spa sends FE BF 00 every ~2s to discover *new* clients.
                // We already have a valid ID, so no action needed.
            }
            IncomingMessage::Registration(
                launa_protocol::registration::RegistrationMessage::ClientIdAssignment {
                    channel,
                    ..
                },
            ) => {
                self.client_id = Some(channel);
            }
            IncomingMessage::Registration(_) => {
                // Other registration messages when already registered — ignore.
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
            _ => {}
        }

        actions
    }

    /// Handle an incoming MQTT command.
    pub fn on_mqtt_command(&mut self, cmd: Command) -> Vec<AppAction> {
        // For SetTemperature, replace any existing queued SetTemperature
        // so rapid presses only send the latest desired value.
        if let Command::SetTemperature(temp) = cmd {
            for entry in self.command_queue.iter_mut() {
                if matches!(entry, Command::SetTemperature(_)) {
                    *entry = Command::SetTemperature(temp);
                    return Vec::new();
                }
            }
        }

        // For SetTime with is_24h, first queue a SetPreference to ensure the
        // spa's clock mode matches before sending the time command.
        if let Command::SetTime { is_24h: true, .. } = &cmd {
            let pref = Command::SetPreference {
                code: launa_protocol::command::preference::CLOCK_MODE,
                value: 1,
            };
            // Only queue if not already queued
            if !self.command_queue.iter().any(|c| {
                matches!(
                    c,
                    Command::SetPreference {
                        code: launa_protocol::command::preference::CLOCK_MODE,
                        ..
                    }
                )
            }) {
                if self.command_queue.len() >= MAX_COMMAND_QUEUE {
                    self.cmd_tracker.record_dropped();
                } else {
                    self.command_queue.push_back(pref);
                }
            }
        }

        // For ToggleItem, cancel out duplicate toggles: if the same item is
        // already queued, remove it (two toggles = no-op) instead of queuing
        // another one. This prevents rapid button presses from cycling the
        // pump through Off→Low→High→Off with no visible effect.
        if let Command::ToggleItem(new_item) = cmd {
            let mut removed = false;
            let mut i = 0;
            while i < self.command_queue.len() {
                if let Command::ToggleItem(existing) = &self.command_queue[i] {
                    if existing == &new_item {
                        self.command_queue.remove(i);
                        removed = true;
                        break;
                    }
                }
                i += 1;
            }
            if removed {
                return Vec::new();
            }
        }

        if self.command_queue.len() >= MAX_COMMAND_QUEUE {
            // Queue full — drop the command and increment the dropped counter
            self.cmd_tracker.record_dropped();
            return Vec::new();
        }
        // Queue command for next Ready window
        self.command_queue.push_back(cmd);
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
                    self.failed_registration_attempts += 1;
                    actions.push(AppAction::PublishAlert {
                        level: String::from("warn"),
                        message: String::from("registration_timeout"),
                    });
                    self.registration.reset();
                    self.registration_started_at = None;
                }
            }
            // Alert if no registration query seen for 30s after boot
            if !self.no_query_alert_sent && self.registration_started_at.is_none() {
                let elapsed_since_boot = now.elapsed_since(self.boot_time);
                if elapsed_since_boot >= STALE_THRESHOLD_MS {
                    actions.push(AppAction::PublishAlert {
                        level: String::from("warn"),
                        message: String::from("no_registration_query"),
                    });
                    self.no_query_alert_sent = true;
                }
            }

            // NOTE: The proactive registration probe is disabled because it
            // interferes with the spa's native FE BF 00 query cycle. The spa
            // sends new-client queries ~1/s, but the proactive probe transitions
            // the SM to WaitingForAssignment, causing real queries to be ignored.
            // See: https://github.com/.../issues/... for details.
        } else {
            self.registration_started_at = None;
            self.no_query_alert_sent = false;
        }

        // CTS loss detection: if registered and no CTS on our channel for 5 seconds,
        // the spa likely rebooted and forgot about us. Reset registration.
        if self.registration.is_registered() && !self.was_stale {
            let cts_lost = match self.last_cts_time {
                Some(last_cts) => now.elapsed_since(last_cts) >= CTS_LOSS_THRESHOLD_MS,
                None => {
                    // We're registered but have never received a CTS yet.
                    // Don't trigger CTS loss until we've seen at least one CTS —
                    // the spa may not have sent one yet (it sends CTS every ~20ms
                    // but there's a brief window after registration before the first
                    // CTS on our channel arrives).
                    false
                }
            };

            if cts_lost {
                let elapsed_desc = match self.last_cts_time {
                    Some(last_cts) => format!("{}ms since last CTS", now.elapsed_since(last_cts)),
                    None => String::from("registered but never received CTS"),
                };
                log::warn!(
                    "CTS loss detected: {} — resetting registration",
                    elapsed_desc
                );
                self.reset_registration_state(true);
                actions.push(AppAction::PublishAlert {
                    level: String::from("warn"),
                    message: String::from("cts_loss"),
                });
            }
        }

        // Stale detection
        if let Some(last) = self.last_status_time {
            let elapsed = now.elapsed_since(last);

            // Probe at 5s intervals
            let should_probe = self
                .last_probe_time
                .is_none_or(|lp| now.elapsed_since(lp) >= STALE_PROBE_INTERVAL_MS);

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

            // Stale at 30s — reset registration so we re-register on recovery.
            // The spa may have rebooted and forgotten our client ID.
            if elapsed >= STALE_THRESHOLD_MS && !self.was_stale {
                self.was_stale = true;
                self.reset_registration_state(true);
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
                        registration_state: self.registration_state_str(),
                    });
                    actions.push(AppAction::PublishStaleAvailability);
                }
            }
        }

        // Diagnostics publishing (every 60s)
        let should_diag = self
            .last_diag_time
            .is_none_or(|ld| now.elapsed_since(ld) >= DIAGNOSTICS_INTERVAL_MS);
        if should_diag {
            self.last_diag_time = Some(now);
            let uptime_ms = now.elapsed_since(self.boot_time);
            actions.push(AppAction::PublishDiagnostics {
                uptime_secs: uptime_ms / 1000,
                frames_received: self.frames_received,
                unregistered_frames: self.unregistered_frames_received,
                command_retries: self.cmd_tracker.total_retries(),
                command_drops: self.cmd_tracker.total_dropped(),
                registration_state: self.registration_state_str(),
                frame_errors: 0,
                uart_bytes: 0,
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

/// Derive a 2-byte client hash from a device ID string using FNV-1a 16-bit.
///
/// Avoids 0x0000 (null) and 0xF173 (Balboa reserved).
pub fn derive_client_hash(device_id: &str) -> [u8; 2] {
    let mut h: u16 = 0x811C; // FNV-1a 16-bit offset basis
    for &b in device_id.as_bytes() {
        h ^= b as u16;
        h = h.wrapping_mul(0x0101); // FNV-1a 16-bit prime
    }
    if h == 0 {
        h = 1;
    }
    if h == 0xF173 {
        h = 0xF174;
    }
    [(h >> 8) as u8, (h & 0xFF) as u8]
}

fn encode_command(cmd: &Command) -> Vec<u8> {
    let (msg_type, payload) = match cmd.encode() {
        Ok(v) => v,
        Err(e) => {
            log::error!("Command encode failed for {:?}: {:?}", cmd, e);
            return Vec::new();
        }
    };
    match FrameEncoder::encode(msg_type, &payload) {
        Ok(encoded) => encoded,
        Err(e) => {
            log::error!("Frame encode failed for {:?}: {:?}", cmd, e);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::AppAction;
    use crate::types::COMMAND_ACK_TIMEOUT_MS;
    use alloc::boxed::Box;
    use alloc::vec;
    use launa_protocol::command::ToggleItem;
    use launa_protocol::frame::Frame;
    use launa_sim::VirtualClock;

    // Cross-reference: structurally identical helpers exist in
    // launa-integration-tests/tests/common/mod.rs (make_spaapp, make_status_frame,
    // make_ready_frame). These are NOT consolidated into a shared crate because
    // launa-core does not (and should not) depend on launa-integration-tests, and
    // extracting a shared test-util crate would add a new workspace dependency
    // for test-only code.

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

    fn ready_frame(channel: u8) -> Frame {
        Frame {
            message_type: [channel, 0xBF],
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

        // Send NewClientQuery → NewClientResponse is handled by the sync fast-path
        // in uart_task (no pending response queued in SpaApp)
        let actions = app.process_frame(&new_client_query_frame());
        assert_eq!(
            actions.len(),
            0,
            "NewClientResponse is handled by sync fast-path"
        );
        assert!(!app.is_registered());

        // Send ClientIdAssignment → ACK is sent immediately
        let actions = app.process_frame(&client_id_assignment_frame(0x05));
        assert_eq!(
            actions.len(),
            1,
            "ACK should be sent immediately on assignment"
        );
        match &actions[0] {
            AppAction::SendFrame(_bytes) => {}
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
        let actions = app.process_frame(&ready_frame(0x03));
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(has_send);
        assert_eq!(app.queued_command_count(), 0);
    }

    #[test]
    fn test_nothing_to_send_on_ready() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        let actions = app.process_frame(&ready_frame(0x03));
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

        // Go stale — this resets registration
        clock.advance_ms(31_000);
        app.tick();
        assert!(app.is_stale());
        assert!(!app.is_registered(), "stale should reset registration");

        // Re-register (simulates spa sending NewClientQuery)
        app.process_frame(&new_client_query_frame());
        let _actions = app.process_frame(&client_id_assignment_frame(0x03));
        assert!(app.is_registered());
        assert!(app.client_id().is_some());

        // Next status frame should recover from stale
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
    fn test_hash_rotates_after_repeated_registration_failures() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        let _original_hash = app.client_hash;

        // Simulate 10 failed registration attempts (query → timeout, no assignment)
        for _ in 0..10 {
            app.process_frame(&new_client_query_frame()); // sends ID request
            clock.advance_ms(6_000); // past 5s timeout
            app.tick(); // triggers timeout, increments counter
        }

        assert_eq!(app.failed_registration_attempts, 10);
    }

    /// Proactive probe is disabled — it interfered with the spa's native
    /// FE BF 00 query cycle by keeping the SM in WaitingForAssignment.
    /// Keeping the "not sent when no bus traffic" test as a regression guard.

    /// No unsolicited frames should be sent when there is no bus traffic.
    #[test]
    fn test_no_frames_sent_when_no_bus_traffic() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;

        // Advance without receiving any frames
        clock.advance_ms(11_000);
        let actions = app.tick();

        let has_frame = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(!has_frame, "should NOT send any frames when no bus traffic");
    }

    /// Proactive probe should not fire while a registration is already in
    /// progress (WaitingForAssignment after sending ID request).
    #[test]
    fn test_no_probe_while_registration_in_progress() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;

        // Trigger normal registration: spa sends query, we respond with ID request
        app.process_frame(&new_client_query_frame());
        assert!(!app.is_registered());
        // SM is now in WaitingForAssignment, registration_started_at is set

        // Receive a status frame (bus is active)
        app.process_frame(&status_frame());

        // Advance 3s — still within the 5s registration timeout
        clock.advance_ms(3_000);
        let actions = app.tick();

        // Should NOT send another probe — registration is still in progress
        let probe_count = actions
            .iter()
            .filter(|a| matches!(a, AppAction::SendFrame(_)))
            .count();
        assert_eq!(
            probe_count, 0,
            "should NOT send proactive probe while registration is in progress"
        );
    }

    #[test]
    fn test_failed_attempts_reset_on_successful_registration() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;

        // Accumulate some failures
        for _ in 0..5 {
            app.process_frame(&new_client_query_frame());
            clock.advance_ms(6_000);
            app.tick();
        }
        assert_eq!(app.failed_registration_attempts, 5);

        // Now succeed — complete registration (NewClientResponse suppressed,
        // fast-path handles it; ClientIdAck sent immediately)
        app.process_frame(&new_client_query_frame()); // suppressed, fast-path handles
        app.process_frame(&client_id_assignment_frame(0x04)); // sends ACK immediately

        assert!(app.is_registered());
        assert_eq!(app.failed_registration_attempts, 0);
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

        // Send another status with hold still active → timer fires, command QUEUED
        let actions = app.process_frame(&hold_frame);
        // Bug 6 fix: hold timer command should be QUEUED, not sent immediately
        assert!(
            !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "hold timer expiry should NOT produce immediate SendFrame"
        );
        assert_eq!(
            app.queued_command_count(),
            1,
            "hold timer expiry should queue the command"
        );

        // Ready frame should dequeue and send the hold toggle command
        let ready_actions = app.process_frame(&ready_frame(0x03));
        assert!(
            ready_actions
                .iter()
                .any(|a| matches!(a, AppAction::SendFrame(_))),
            "Ready frame should dequeue and send the hold toggle command"
        );
        assert_eq!(app.queued_command_count(), 0);

        // Simulate hold mode being released (the toggle worked).
        // This resets the hold timer's fired flag and allows it to re-arm.
        let release_frame = status_frame(); // is_hold = false
        app.process_frame(&release_frame);
        // Advance past any ACK timeouts so retries settle
        clock.advance_ms(COMMAND_ACK_TIMEOUT_MS + 1);
        app.process_frame(&status_frame());
        // Drain any remaining queue
        while app.queued_command_count() > 0 {
            app.process_frame(&ready_frame(0x03));
        }

        // Bug 1 fix: after firing AND hold mode released, re-enter hold mode.
        // The timer should fire again after the timeout.
        app.process_frame(&hold_frame);
        clock.advance_ms(61 * 60 * 1000);
        let _actions_refire = app.process_frame(&hold_frame);
        assert_eq!(
            app.queued_command_count(),
            1,
            "hold timer should fire again after hold mode was released and re-entered"
        );

        // And Ready frame dequeues it
        let ready_actions2 = app.process_frame(&ready_frame(0x03));
        assert!(
            ready_actions2
                .iter()
                .any(|a| matches!(a, AppAction::SendFrame(_))),
            "Ready frame should dequeue and send the re-armed hold toggle"
        );
        assert_eq!(app.queued_command_count(), 0);
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

        // Next status should trigger auto-off — command QUEUED, not sent immediately
        let actions = app.process_frame(&status);
        assert!(
            !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "pump timer expiry should NOT produce immediate SendFrame"
        );
        assert_eq!(
            app.queued_command_count(),
            1,
            "pump timer expiry should queue the toggle-off command"
        );

        // Ready frame should dequeue and send the pump toggle-off command
        let ready_actions = app.process_frame(&ready_frame(0x03));
        assert!(
            ready_actions
                .iter()
                .any(|a| matches!(a, AppAction::SendFrame(_))),
            "Ready frame should dequeue and send the pump toggle-off"
        );
        assert_eq!(app.queued_command_count(), 0);
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
        app.process_frame(&ready_frame(0x03));

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
        app.process_frame(&ready_frame(0x03));

        // Advance past timeout (5s) but don't change status
        clock.advance_ms(6_000);

        // Same status arrives → pump still off → timeout triggers retry
        // Bug 6 fix: retry is QUEUED for next Ready window, not sent immediately
        let actions = app.process_frame(&status_frame());
        assert!(
            !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "retry should NOT produce immediate SendFrame"
        );
        assert_eq!(app.queued_command_count(), 1, "retry should be queued");
        assert!(app.total_retries() > 0);

        // Ready frame dequeues and sends the retry
        let ready_actions = app.process_frame(&ready_frame(0x03));
        assert!(
            ready_actions
                .iter()
                .any(|a| matches!(a, AppAction::SendFrame(_))),
            "Ready frame should dequeue and send the retry command"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    #[test]
    fn test_registered_ignores_new_client_query() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Already registered — NewClientQuery should be ignored
        let actions = app.process_frame(&new_client_query_frame());
        assert!(app.is_registered(), "should stay registered");
        assert_eq!(app.client_id(), Some(0x03), "client ID should be preserved");
        assert!(actions.is_empty(), "no actions from ignored NewClientQuery");

        // Receiving another NewClientQuery still ignored
        let actions = app.process_frame(&new_client_query_frame());
        assert!(app.is_registered());
        assert!(actions.is_empty());
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
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), 2);

        // Second Ready → send pump2
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), 1);

        // Third Ready → send pump3
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), 0);
    }

    #[test]
    fn test_command_queue_cap() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Fill the queue with non-deduplicating commands.
        // ToggleItem deduplicates (cancels duplicates), SetTemperature replaces.
        // ConfigurationRequest and other commands are just appended.
        for _ in 0..MAX_COMMAND_QUEUE {
            app.on_mqtt_command(Command::ConfigurationRequest);
        }
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 0);

        // Next command should be dropped
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 1);

        // Queue another — also dropped
        app.on_mqtt_command(Command::ConfigurationRequest);
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 2);

        // Drain one via Ready, then queue should accept again
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE - 1);

        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        assert_eq!(app.queued_command_count(), MAX_COMMAND_QUEUE);
        assert_eq!(app.total_dropped(), 2); // no new drops
    }

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

    /// Helper: check if a byte slice contains a subsequence.
    fn contains_sequence(haystack: &[u8], needle: &[u8]) -> bool {
        if needle.len() > haystack.len() {
            return false;
        }
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
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

        // Get initial status and CTS to keep CTS loss timer happy
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x03));

        // Advance past probe interval. Keep sending CTS to prevent CTS loss
        // from firing (the probe test is about status staleness, not CTS loss).
        clock.advance_ms(6_000);
        app.process_frame(&ready_frame(0x03)); // keep CTS alive
        let actions = app.tick();

        let frames = collect_sent_frames(&actions);
        assert!(
            !frames.is_empty(),
            "stale probe should send at least one frame"
        );

        // The probe should be a NothingToSend: msg_type=[client_id, 0xBF], payload=[0x07]
        let expected = {
            let (mt, payload) = Command::NothingToSend { client_id: 0x03 }
                .encode()
                .expect("encode should succeed");
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
        // Get initial CTS to keep CTS loss timer happy
        app.process_frame(&ready_frame(0x03));

        // Advance to 6s → first probe. Send CTS to prevent CTS loss.
        clock.advance_ms(6_000);
        app.process_frame(&ready_frame(0x03)); // keep CTS alive
        let actions1 = app.tick();
        assert!(
            collect_sent_frames(&actions1).iter().any(|f| !f.is_empty()),
            "first probe should fire after 5s+"
        );

        // Advance only 3s → no probe yet (interval is 5s)
        clock.advance_ms(3_000);
        app.process_frame(&ready_frame(0x03)); // keep CTS alive
        let actions2 = app.tick();
        let frames2 = collect_sent_frames(&actions2);
        assert!(
            frames2.is_empty(),
            "no probe should fire at 3s after last probe"
        );

        // Advance to 5s total since last probe → second probe
        clock.advance_ms(2_000);
        app.process_frame(&ready_frame(0x03)); // keep CTS alive
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

    /// VAL-BM-010: SpaApp cancels pump timer on Off status — no toggle-off SendFrame.
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

        // Now pump turns off externally (someone pressed the physical button).
        // process_frame ticks pump timers; the timer sees pump is Off and cancels.
        let status_off = status_frame(); // pump 1 = Off (default)
        let _actions = app.process_frame(&status_off);

        // Advance past the timer duration — the cancelled timer must NOT fire.
        clock.advance_ms(61_000);

        // Feed another status with pump still off — no auto-off SendFrame should appear.
        let actions = app.process_frame(&status_off);
        let has_send_frame = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            !has_send_frame,
            "no SendFrame should appear after pump timer was cancelled by external Off"
        );
    }

    /// WiFi reconnection lifecycle test (VAL-PL-004)
    #[test]
    fn test_wifi_reconnection_lifecycle() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);

        // Phase 1: Normal operation — process a few status updates with CTS
        for _ in 0..3 {
            let actions = app.process_frame(&status_frame());
            assert!(
                actions
                    .iter()
                    .any(|a| matches!(a, AppAction::PublishState { .. })),
                "normal status should produce PublishState"
            );
            app.process_frame(&ready_frame(0x03));
            clock.advance_ms(1_000);
        }
        assert!(!app.is_stale());
        assert_eq!(app.frames_received(), 3);

        // Phase 2: Bus silence — advance to 6s (CTS loss fires at 5s)
        clock.advance_ms(6_000);
        let actions = app.tick();
        // CTS loss fires first (5s no CTS), which is the primary recovery
        let has_cts_loss = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { message, .. } if message == "cts_loss"));
        assert!(has_cts_loss, "CTS loss should fire at 5s of bus silence");
        assert!(!app.is_registered(), "CTS loss should reset registration");

        // Phase 3: Continue silence — stale detection still fires at 30s as fallback
        // (status time was set during Phase 1, so advance to 30s total)
        clock.advance_ms(25_000); // total ~31s since last status
        let actions = app.tick();
        let has_stale_alert = actions.iter().any(|a| {
            matches!(
                a,
                AppAction::PublishAlert { message, .. } if message == "spa_communication_lost"
            )
        });
        assert!(
            has_stale_alert,
            "stale alert should also fire at 30s as fallback"
        );
        assert!(app.is_stale(), "should be stale after 30s silence");

        // Phase 4: Communication resumes — re-register first
        app.process_frame(&new_client_query_frame());
        app.process_frame(&client_id_assignment_frame(0x03));
        assert!(app.is_registered(), "should re-register after stale");

        // Phase 5: Status arrives → stale recovery
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

    /// Helper: create a Ready frame with a specific client ID.
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
        let actions = app.process_frame(&ready_frame(0x03));
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
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), 2);

        // Another status — no additional dequeue
        app.process_frame(&status_frame());
        assert_eq!(app.queued_command_count(), 2);

        // Ready 2 → dequeue second
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), 1);

        // Ready 3 → dequeue third
        app.process_frame(&ready_frame(0x03));
        assert_eq!(app.queued_command_count(), 0);
    }

    /// VAL-PROTO-005: Ready frame with registered client ID also dequeues commands.
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
        let actions = app.process_frame(&ready_frame(0x03));
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
        let actions = app.process_frame(&ready_frame(0x03));
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
        let actions = app.process_frame(&ready_frame(0x03));
        let has_send = actions.iter().any(|a| matches!(a, AppAction::SendFrame(_)));
        assert!(
            has_send,
            "Ready should dequeue command even without prior status"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    /// Rapid duplicate toggles cancel out (press, press again → no-op).
    #[test]
    fn test_toggle_duplicate_cancels_out() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        // First toggle: queued
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Second toggle for same item: cancels the first
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 0);
    }

    /// Different toggle items are queued independently.
    #[test]
    fn test_toggle_different_items_not_deduplicated() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2));
        assert_eq!(app.queued_command_count(), 2);
    }

    /// Toggle deduplication only cancels matching items, not cross-contaminating.
    #[test]
    fn test_toggle_dedup_does_not_affect_other_commands() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        app.on_mqtt_command(Command::SetTemperature(100));
        assert_eq!(app.queued_command_count(), 2);

        // Duplicate toggle cancels only the toggle, not the set-temp
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Remaining command is the set-temp
        match app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump2)) {
            _ => {}
        }
        assert_eq!(app.queued_command_count(), 2);
    }

    /// Triple rapid press: press-press-press results in one toggle queued.
    #[test]
    fn test_triple_rapid_press_results_in_one_toggle() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x03);
        app.process_frame(&status_frame());

        // Press 1: queue
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Press 2: cancel
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 0);

        // Press 3: queue again
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);
    }

    /// CTS filtering: registered app ignores Ready frames on wrong channel.
    #[test]
    fn test_cts_filtered_by_client_id() {
        let (_clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x11);
        app.process_frame(&status_frame());

        // Queue a command
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // CTS on wrong channel (0x10 = display panel) → ignored
        let actions = app.process_frame(&ready_frame(0x10));
        assert!(
            !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "CTS on wrong channel should be ignored"
        );
        assert_eq!(
            app.queued_command_count(),
            1,
            "command should remain queued"
        );

        // CTS on another wrong channel (0x12) → ignored
        let actions = app.process_frame(&ready_frame(0x12));
        assert!(
            !actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "CTS on other channel should be ignored"
        );
        assert_eq!(app.queued_command_count(), 1);

        // CTS on our channel (0x11) → command dequeued
        let actions = app.process_frame(&ready_frame(0x11));
        assert!(
            actions.iter().any(|a| matches!(a, AppAction::SendFrame(_))),
            "CTS on our channel should dequeue command"
        );
        assert_eq!(app.queued_command_count(), 0);
    }

    /// CTS loss detection: no CTS on our channel for 5s triggers re-registration.
    #[test]
    fn test_cts_loss_detection_resets_registration() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x11);

        // Get initial CTS and status to establish normal operation
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x11));
        assert!(app.is_registered());
        assert!(!app.is_stale());

        // Queue a command to verify it gets cleared on CTS loss
        app.on_mqtt_command(Command::ToggleItem(ToggleItem::Pump1));
        assert_eq!(app.queued_command_count(), 1);

        // Continue receiving status frames (spa is alive, sending FFAF)
        // but NOT receiving CTS on our channel (simulating spa reboot).
        clock.advance_ms(2_000);
        app.process_frame(&status_frame());
        let actions = app.tick();
        // Should NOT trigger CTS loss yet (only 2s)
        assert!(
            !actions.iter().any(
                |a| matches!(a, AppAction::PublishAlert { message, .. } if message == "cts_loss")
            ),
            "should not trigger CTS loss at 2s"
        );
        assert!(app.is_registered());

        // CTS on wrong channel should NOT reset the timer (spa sends CTS on 0x10 after reboot)
        app.process_frame(&ready_frame(0x10));

        // Advance past 5s threshold since last CTS on our channel
        clock.advance_ms(4_000);
        app.process_frame(&status_frame()); // status still coming in
        let actions = app.tick();

        // Now CTS loss should fire
        let has_cts_loss = actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { message, .. } if message == "cts_loss"));
        assert!(has_cts_loss, "should trigger CTS loss after 5s");
        assert!(!app.is_registered(), "CTS loss should reset registration");
        assert_eq!(
            app.queued_command_count(),
            0,
            "CTS loss should clear command queue"
        );
    }

    /// CTS loss does NOT fire when still receiving CTS on our channel.
    #[test]
    fn test_cts_loss_no_false_positive() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x11);

        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x11));

        // Advance 4s and send another CTS — timer resets
        clock.advance_ms(4_000);
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x11));

        // Advance another 4s — still within 5s of last CTS
        clock.advance_ms(4_000);
        app.process_frame(&status_frame());
        let actions = app.tick();

        let no_cts_loss = !actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { message, .. } if message == "cts_loss"));
        assert!(
            no_cts_loss,
            "should NOT trigger CTS loss when CTS is regular"
        );
        assert!(app.is_registered());
    }

    /// After CTS loss recovery, re-registration works and CTS tracking resumes.
    #[test]
    fn test_cts_loss_recovery_and_reregistration() {
        let (clock, app) = make_app_with_clock();
        let mut app = app;
        app.force_registered(0x11);

        // Normal operation
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x11));

        // CTS loss — advance past threshold
        clock.advance_ms(6_000);
        app.process_frame(&status_frame());
        let actions = app.tick();
        assert!(actions.iter().any(
            |a| matches!(a, AppAction::PublishAlert { message, .. } if message == "cts_loss")
        ));
        assert!(!app.is_registered());

        // Re-register
        app.process_frame(&new_client_query_frame());
        let _actions = app.process_frame(&client_id_assignment_frame(0x11));
        assert!(app.is_registered());
        assert_eq!(app.client_id(), Some(0x11));

        // Receiving CTS on our channel resumes normal operation
        app.process_frame(&status_frame());
        app.process_frame(&ready_frame(0x11));

        // Advance 3s — no CTS loss (timer was reset by new CTS)
        clock.advance_ms(3_000);
        let actions = app.tick();
        let no_cts_loss = !actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { message, .. } if message == "cts_loss"));
        assert!(
            no_cts_loss,
            "should not trigger CTS loss after re-registration with active CTS"
        );
    }
}
