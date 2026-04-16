//! Spa controller logic extracted from the ESP32 main loop.
//!
//! This is the core firmware logic with zero hardware dependencies. It can be
//! tested end-to-end on desktop by feeding it simulated RS-485 bytes from a
//! `SpaSim` and observing the emitted `ControllerEvent`s.

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::frame::FrameDecoder;
use launa_protocol::registration::{RegistrationAction, RegistrationStateMachine};
use launa_protocol::status::{PumpState, StatusUpdate};

/// Events emitted by the controller for the caller to act on.
///
/// The caller (ESP32 main loop or test harness) is responsible for:
/// - Publishing state to MQTT on `StatusUpdate`
/// - Writing registration responses to the UART on `RegistrationAction`
/// - Writing pump-off commands to the UART on `PumpExpired`
/// - Sending queued commands on `Ready`
#[derive(Debug, Clone, PartialEq)]
pub enum ControllerEvent {
    /// A new status update was received from the spa.
    StatusUpdate(StatusUpdate),

    /// Registration completed successfully.
    Registered { client_id: u8 },

    /// The controller needs to send registration protocol bytes.
    /// The caller should write the returned bytes to the transport.
    RegistrationSend { bytes: Vec<u8> },

    /// A pump timer expired and an auto-off command needs to be sent.
    PumpExpired(Command),

    /// The spa signaled the RS-485 bus is free for commands.
    Ready,

    /// An unhandled/unknown message was received.
    UnknownMessage,
}

/// Pump timer that uses a virtual clock instead of real time.
///
/// In the real firmware this uses `std::time::Instant`, but for simulation
/// we track elapsed time via `tick()` calls.
struct PumpTimer {
    pump: ToggleItem,
    started_at: Option<u64>, // tick when started
    duration_secs: u64,      // duration in simulated seconds
}

const DEFAULT_PUMP_DURATION_SECS: u64 = 20 * 60; // 20 minutes

impl PumpTimer {
    fn new(pump: ToggleItem) -> Self {
        PumpTimer {
            pump,
            started_at: None,
            duration_secs: DEFAULT_PUMP_DURATION_SECS,
        }
    }

    fn start(&mut self, at_tick: u64) {
        self.started_at = Some(at_tick);
    }

    fn cancel(&mut self) {
        self.started_at = None;
    }

    fn tick(&mut self, now: u64, pump_state: PumpState) -> Option<Command> {
        if let Some(started_at) = self.started_at {
            let is_on = matches!(pump_state, PumpState::Low | PumpState::High);
            if !is_on {
                self.started_at = None;
                return None;
            }
            if now.saturating_sub(started_at) >= self.duration_secs {
                self.started_at = None;
                return Some(Command::ToggleItem(self.pump));
            }
        }
        None
    }

    fn is_running(&self) -> bool {
        self.started_at.is_some()
    }
}

/// Manages pump timers for all pumps.
struct PumpTimerManager {
    timers: [PumpTimer; 6],
}

impl PumpTimerManager {
    fn new() -> Self {
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

    fn tick_all(&mut self, now: u64, pumps: &[PumpState; 6]) -> Vec<Command> {
        let mut commands = Vec::new();
        for (i, timer) in self.timers.iter_mut().enumerate() {
            if let Some(cmd) = timer.tick(now, pumps[i]) {
                commands.push(cmd);
            }
        }
        commands
    }
}

/// The core spa controller logic.
///
/// Feed it raw RS-485 bytes via `process_bytes()` and it will return a list of
/// `ControllerEvent`s. The caller is responsible for acting on those events
/// (writing to UART, publishing to MQTT, etc.).
pub struct SpaController {
    frame_decoder: FrameDecoder,
    registration: RegistrationStateMachine,
    pump_timers: PumpTimerManager,
    last_status: Option<StatusUpdate>,
    tick_count: u64,
}

impl SpaController {
    pub fn new() -> Self {
        SpaController {
            frame_decoder: FrameDecoder::new(),
            registration: RegistrationStateMachine::new(),
            pump_timers: PumpTimerManager::new(),
            last_status: None,
            tick_count: 0,
        }
    }

    /// Whether the controller has completed registration with the spa.
    pub fn is_registered(&self) -> bool {
        self.registration.is_registered()
    }

    /// The assigned client ID, if registered.
    pub fn client_id(&self) -> Option<u8> {
        self.registration.client_id()
    }

    /// The last received status update, if any.
    pub fn last_status(&self) -> Option<&StatusUpdate> {
        self.last_status.as_ref()
    }

    /// Feed raw RS-485 bytes and process all decoded frames.
    ///
    /// Returns events the caller should act on. Call this with bytes
    /// read from the `Transport::read()` call.
    pub fn process_bytes(&mut self, bytes: &[u8]) -> Vec<ControllerEvent> {
        let frames = self.frame_decoder.feed_slice(bytes);
        let mut events = Vec::new();

        for frame in &frames {
            let message = dispatch_frame(frame);

            // Handle registration
            if !self.registration.is_registered() {
                let action = self
                    .registration
                    .process(frame.message_type, &frame.payload);
                match action {
                    RegistrationAction::SendIdRequest => {
                        let encoded = launa_protocol::frame::FrameEncoder::encode(
                            [0xFE, 0xBF],
                            &[0x01, 0x02, 0xF1, 0x73],
                        )
                        .unwrap();
                        events.push(ControllerEvent::RegistrationSend { bytes: encoded });
                    }
                    RegistrationAction::SendIdAck { client_id } => {
                        let encoded =
                            launa_protocol::frame::FrameEncoder::encode([client_id, 0xBF], &[0x03])
                                .unwrap();
                        events.push(ControllerEvent::Registered { client_id });
                        events.push(ControllerEvent::RegistrationSend { bytes: encoded });
                    }
                    RegistrationAction::None => {}
                }
                continue;
            }

            // Handle incoming messages
            match message {
                IncomingMessage::StatusUpdate(status) => {
                    // Tick pump timers
                    self.tick_count += 1;
                    let expired = self.pump_timers.tick_all(self.tick_count, &status.pumps);
                    for cmd in expired {
                        events.push(ControllerEvent::PumpExpired(cmd));
                    }

                    self.last_status = Some(status.clone());
                    events.push(ControllerEvent::StatusUpdate(status));
                }
                IncomingMessage::Ready => {
                    events.push(ControllerEvent::Ready);
                }
                IncomingMessage::NewClientQuery => {
                    // Re-registration query; could happen if spa resets
                }
                _ => {
                    events.push(ControllerEvent::UnknownMessage);
                }
            }
        }

        events
    }

    /// Encode a command for sending to the spa.
    ///
    /// Returns `None` if not registered. Otherwise returns the encoded bytes
    /// to write to the transport.
    pub fn encode_command(&self, cmd: &Command) -> Option<Vec<u8>> {
        // Used as a registration guard: returns None if not registered
        let _ = self.registration.client_id()?;

        let (msg_type, payload) = cmd.encode();
        Some(launa_protocol::frame::FrameEncoder::encode(msg_type, &payload).ok()?)
    }

    /// Start a pump timer (for P1 mode auto-off after 20 minutes).
    /// Call this when the user enables P1 mode via MQTT.
    pub fn start_pump_timer(&mut self, pump: ToggleItem) {
        let idx = match pump {
            ToggleItem::Pump1 => 0,
            ToggleItem::Pump2 => 1,
            ToggleItem::Pump3 => 2,
            ToggleItem::Pump4 => 3,
            ToggleItem::Pump5 => 4,
            ToggleItem::Pump6 => 5,
            _ => return,
        };
        if idx < self.pump_timers.timers.len() {
            self.pump_timers.timers[idx].start(self.tick_count);
        }
    }

    /// Cancel a pump timer.
    pub fn cancel_pump_timer(&mut self, pump: ToggleItem) {
        let idx = match pump {
            ToggleItem::Pump1 => 0,
            ToggleItem::Pump2 => 1,
            ToggleItem::Pump3 => 2,
            ToggleItem::Pump4 => 3,
            ToggleItem::Pump5 => 4,
            ToggleItem::Pump6 => 5,
            _ => return,
        };
        if idx < self.pump_timers.timers.len() {
            self.pump_timers.timers[idx].cancel();
        }
    }

    /// Check if a pump timer is running.
    pub fn is_pump_timer_running(&self, pump: ToggleItem) -> bool {
        let idx = match pump {
            ToggleItem::Pump1 => 0,
            ToggleItem::Pump2 => 1,
            ToggleItem::Pump3 => 2,
            ToggleItem::Pump4 => 3,
            ToggleItem::Pump5 => 4,
            ToggleItem::Pump6 => 5,
            _ => return false,
        };
        idx < self.pump_timers.timers.len() && self.pump_timers.timers[idx].is_running()
    }

    /// Force the controller into a registered state.
    ///
    /// Intended for tests that need to bypass the normal registration flow.
    pub fn force_registered(&mut self, client_id: u8) {
        self.registration.process([0xFE, 0xBF], &[0x00]);
        self.registration.process([0xFE, 0xBF], &[0x02, client_id]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controller_new() {
        let c = SpaController::new();
        assert!(!c.is_registered());
        assert!(c.last_status().is_none());
    }

    #[test]
    fn test_encode_command_not_registered() {
        let c = SpaController::new();
        let cmd = Command::ToggleItem(ToggleItem::Pump1);
        assert!(c.encode_command(&cmd).is_none());
    }

    #[test]
    fn test_encode_command_registered() {
        let mut c = SpaController::new();
        // Simulate registration
        c.registration.process([0xFE, 0xBF], &[0x00]);
        c.registration.process([0xFE, 0xBF], &[0x02, 0x05]);

        let cmd = Command::ToggleItem(ToggleItem::Pump1);
        let encoded = c.encode_command(&cmd);
        assert!(encoded.is_some());
        assert!(!encoded.unwrap().is_empty());
    }
}
