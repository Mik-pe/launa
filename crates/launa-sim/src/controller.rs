//! Spa controller logic extracted from the ESP32 main loop.
//!
//! **Deprecated**: This module contains `SpaController`, a simplified protocol
//! handler that diverges from the real firmware logic. New code should use
//! `SpaApp` from the `launa-core` crate instead, which is the actual extracted
//! firmware logic used in production.
//!
//! Migration guide:
//! - Replace `SpaController::new()` with `SpaApp::new(clock)`
//! - Replace `process_bytes()` with `process_frame()` + `tick()`
//! - Replace `ControllerEvent` handling with `AppAction` handling
//! - See `launa-integration-tests` for examples of `SpaApp` usage
//!
//! This module is kept for backward compatibility with existing sim tests.

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::config::SpaConfig;
use launa_protocol::dispatcher::{dispatch_frame, IncomingMessage};
use launa_protocol::fault::FaultLogEntry;
use launa_protocol::filter::FilterCycles;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::information::InformationResponse;
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

    /// A fault log response was received from the spa.
    FaultLogResponse(FaultLogEntry),

    /// A filter cycles response was received from the spa.
    FilterCyclesResponse(FilterCycles),

    /// An information response was received from the spa.
    InformationResponse(InformationResponse),

    /// A configuration response was received from the spa.
    ConfigurationResponse(SpaConfig),

    /// A control configuration response was received from the spa.
    ControlConfiguration(SpaConfig),

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
/// **Deprecated**: Use `launa_core::SpaApp` instead, which is the real extracted
/// firmware logic. `SpaController` is a simplified version that diverges from
/// production behavior. See the module-level documentation for migration guidance.
///
/// Feed it raw RS-485 bytes via `process_bytes()` and it will return a list of
/// `ControllerEvent`s. The caller is responsible for acting on those events
/// (writing to UART, publishing to MQTT, etc.).
#[deprecated(
    since = "0.2.0",
    note = "Use `launa_core::SpaApp` instead. SpaController is a simplified version that diverges from production logic. See module docs for migration guide."
)]
pub struct SpaController {
    frame_decoder: FrameDecoder,
    registration: RegistrationStateMachine,
    pump_timers: PumpTimerManager,
    last_status: Option<StatusUpdate>,
    last_fault_log: Option<FaultLogEntry>,
    last_filter_cycles: Option<FilterCycles>,
    last_information: Option<InformationResponse>,
    last_config: Option<SpaConfig>,
    last_control_config: Option<SpaConfig>,
    tick_count: u64,
}

impl SpaController {
    pub fn new() -> Self {
        SpaController {
            frame_decoder: FrameDecoder::new(),
            registration: RegistrationStateMachine::new(),
            pump_timers: PumpTimerManager::new(),
            last_status: None,
            last_fault_log: None,
            last_filter_cycles: None,
            last_information: None,
            last_config: None,
            last_control_config: None,
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

    /// The last received fault log entry, if any.
    pub fn last_fault_log(&self) -> Option<&FaultLogEntry> {
        self.last_fault_log.as_ref()
    }

    /// The last received filter cycles data, if any.
    pub fn last_filter_cycles(&self) -> Option<&FilterCycles> {
        self.last_filter_cycles.as_ref()
    }

    /// The last received information response, if any.
    pub fn last_information(&self) -> Option<&InformationResponse> {
        self.last_information.as_ref()
    }

    /// The last received configuration response, if any.
    pub fn last_config(&self) -> Option<&SpaConfig> {
        self.last_config.as_ref()
    }

    /// The last received control configuration response, if any.
    pub fn last_control_config(&self) -> Option<&SpaConfig> {
        self.last_control_config.as_ref()
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
                IncomingMessage::FaultLogResponse(entry) => {
                    self.last_fault_log = Some(entry.clone());
                    events.push(ControllerEvent::FaultLogResponse(entry));
                }
                IncomingMessage::FilterCyclesResponse(cycles) => {
                    self.last_filter_cycles = Some(cycles.clone());
                    events.push(ControllerEvent::FilterCyclesResponse(cycles));
                }
                IncomingMessage::InformationResponse(info) => {
                    self.last_information = Some(info.clone());
                    events.push(ControllerEvent::InformationResponse(info));
                }
                IncomingMessage::ConfigurationResponse(config) => {
                    self.last_config = Some(config.clone());
                    events.push(ControllerEvent::ConfigurationResponse(config));
                }
                IncomingMessage::ControlConfiguration(config) => {
                    self.last_control_config = Some(config.clone());
                    events.push(ControllerEvent::ControlConfiguration(config));
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
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::{FaultLogConfig, FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaSim};
    use launa_protocol::fault::FaultCode;
    use launa_protocol::frame::FrameEncoder;

    /// Helper: create a registered SpaController for testing.
    fn make_registered_controller() -> SpaController {
        let mut c = SpaController::new();
        c.force_registered(0x05);
        c
    }

    /// Helper: create a SpaSim that has completed client registration.
    fn make_registered_sim() -> SpaSim {
        let mut sim = SpaSim::new();
        // Complete registration by processing an ID request frame
        let id_request = FrameEncoder::encode([0xFE, 0xBF], &[0x01]).unwrap();
        sim.process_incoming_bytes(&id_request);
        // Sim now has registered=true and client_id set
        sim
    }

    // -- Basic controller tests --

    #[test]
    fn test_controller_new() {
        let c = SpaController::new();
        assert!(!c.is_registered());
        assert!(c.last_status().is_none());
        assert!(c.last_fault_log().is_none());
        assert!(c.last_filter_cycles().is_none());
        assert!(c.last_information().is_none());
        assert!(c.last_config().is_none());
        assert!(c.last_control_config().is_none());
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

    // -- VAL-MQTT-025: SpaController emits event for FaultLogResponse --

    #[test]
    fn test_controller_handles_fault_log_response() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Generate fault log response bytes from SpaSim
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        // Feed response bytes to controller
        let events = c.process_bytes(&response_bytes);

        // Should have exactly one FaultLogResponse event
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], ControllerEvent::FaultLogResponse(_)),
            "expected FaultLogResponse, got {:?}",
            events[0]
        );

        // Accessor should return the stored entry
        let entry = c.last_fault_log().expect("should have stored fault log");
        assert_eq!(entry.fault_count, 3);
        assert_eq!(entry.message_code, FaultCode::HeaterDry);
    }

    // -- VAL-MQTT-026: SpaController emits event for FilterCyclesResponse --

    #[test]
    fn test_controller_handles_filter_cycles_response() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Generate filter cycles response bytes from SpaSim
        let (mt, payload) = Command::FilterCyclesRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        // Feed response bytes to controller
        let events = c.process_bytes(&response_bytes);

        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], ControllerEvent::FilterCyclesResponse(_)),
            "expected FilterCyclesResponse, got {:?}",
            events[0]
        );

        let fc = c
            .last_filter_cycles()
            .expect("should have stored filter cycles");
        assert_eq!(fc.filter1.start_hour, 8);
        assert_eq!(fc.filter1.duration_hours, 4);
        assert_eq!(fc.filter2.start_hour, 16);
        assert!(fc.filter2.enabled);
    }

    // -- VAL-MQTT-027: SpaController emits event for InformationResponse --

    #[test]
    fn test_controller_handles_information_response() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Generate information response bytes from SpaSim
        let (mt, payload) = Command::InformationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        // Feed response bytes to controller
        let events = c.process_bytes(&response_bytes);

        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], ControllerEvent::InformationResponse(_)),
            "expected InformationResponse, got {:?}",
            events[0]
        );

        let info = c
            .last_information()
            .expect("should have stored information");
        assert_eq!(info.system_model, "BFBP20");
        assert_eq!(info.config_signature, "3D12382E");
    }

    // -- VAL-MQTT-028: SpaController emits event for ConfigurationResponse --

    #[test]
    fn test_controller_handles_configuration_response() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Generate configuration response bytes from SpaSim
        let (mt, payload) = Command::ConfigurationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        // Feed response bytes to controller
        let events = c.process_bytes(&response_bytes);

        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], ControllerEvent::ControlConfiguration(_)),
            "expected ControlConfiguration, got {:?}",
            events[0]
        );

        let config = c
            .last_control_config()
            .expect("should have stored control config");
        assert!(config.circ_pump);
        assert!(config.blower);
    }

    // -- VAL-MQTT-029: SpaController preserves all response data without loss --

    #[test]
    fn test_controller_response_data_fidelity() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Configure custom fault log data
        sim.set_fault_log_config(FaultLogConfig {
            fault_count: 7,
            entry_number: 3,
            message_code: FaultCode::WaterTooHot,
            days_ago: 5,
            hour: 10,
            minute: 45,
            flags: 0x12,
            set_temperature: 106,
            sensor_a_temp: 108,
            sensor_b_temp: 107,
        });

        // Generate and process fault log response
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        let events = c.process_bytes(&response_bytes);
        assert_eq!(events.len(), 1);

        // Verify ALL fields preserved without loss
        let entry = c.last_fault_log().unwrap();
        assert_eq!(entry.fault_count, 7);
        assert_eq!(entry.entry_number, 3);
        assert_eq!(entry.message_code, FaultCode::WaterTooHot);
        assert_eq!(entry.days_ago, 5);
        assert_eq!(entry.hour, 10);
        assert_eq!(entry.minute, 45);
        assert_eq!(entry.flags, 0x12);
        assert_eq!(entry.set_temperature, 106);
        assert_eq!(entry.sensor_a_temp, 108);
        assert_eq!(entry.sensor_b_temp, 107);

        // Also verify the event carries the same data
        if let ControllerEvent::FaultLogResponse(event_entry) = &events[0] {
            assert_eq!(event_entry, entry);
        } else {
            panic!("expected FaultLogResponse event");
        }
    }

    #[test]
    fn test_controller_filter_cycles_data_fidelity() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Configure custom filter cycles data
        sim.set_filter_cycles_config(FilterCyclesConfig {
            filter1: FilterCycleConfig {
                start_hour: 6,
                start_minute: 30,
                duration_hours: 2,
                duration_minutes: 15,
                enabled: true,
            },
            filter2: FilterCycleConfig {
                start_hour: 18,
                start_minute: 45,
                duration_hours: 1,
                duration_minutes: 30,
                enabled: false,
            },
        });

        let (mt, payload) = Command::FilterCyclesRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        c.process_bytes(&response_bytes);

        let fc = c.last_filter_cycles().unwrap();
        assert_eq!(fc.filter1.start_hour, 6);
        assert_eq!(fc.filter1.start_minute, 30);
        assert_eq!(fc.filter1.duration_hours, 2);
        assert_eq!(fc.filter1.duration_minutes, 15);
        assert!(fc.filter1.enabled);

        assert_eq!(fc.filter2.start_hour, 18);
        assert_eq!(fc.filter2.start_minute, 45);
        assert_eq!(fc.filter2.duration_hours, 1);
        assert_eq!(fc.filter2.duration_minutes, 30);
        assert!(!fc.filter2.enabled);
    }

    #[test]
    fn test_controller_information_data_fidelity() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // Configure custom information response
        let mut model = [b' '; 8];
        let model_str = b"CUSTOM20";
        model.copy_from_slice(model_str);
        sim.set_information_config(InformationConfig {
            software_id_byte0: 0xAB,
            software_id_byte1: 0xCD,
            software_version_byte0: 0x22,
            software_version_byte1: 0x01,
            system_model: model,
            current_setup: 0x03,
            config_sig_byte0: 0xAA,
            config_sig_byte1: 0xBB,
            config_sig_byte2: 0xCC,
            config_sig_byte3: 0xDD,
            heater_voltage: 0x01,
            heater_type: 0x0A,
            dip_switch_byte0: 0xFF,
            dip_switch_byte1: 0x00,
        });

        let (mt, payload) = Command::InformationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let response_bytes = sim.process_frame(&request_frames[0]).unwrap();

        c.process_bytes(&response_bytes);

        let info = c.last_information().unwrap();
        assert_eq!(info.system_model, "CUSTOM20");
        assert_eq!(info.current_setup, 0x03);
        assert_eq!(info.config_signature, "AABBCCDD");
        assert_eq!(
            info.heater_voltage,
            launa_protocol::information::HeaterVoltage::V240
        );
        assert_eq!(
            info.heater_type,
            launa_protocol::information::HeaterType::Standard
        );
        assert_eq!(info.dip_switches, "1111111100000000");
    }

    // -- VAL-MQTT-030: SpaController handles mixed StatusUpdate and response events --

    #[test]
    fn test_controller_mixed_status_and_response() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Generate a full tick (status + ready) and a fault log response
        let tick_bytes = sim.tick();
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let fault_bytes = sim.process_frame(&request_frames[0]).unwrap();

        // Concatenate all bytes
        let mut all_bytes = Vec::new();
        all_bytes.extend_from_slice(&tick_bytes);
        all_bytes.extend_from_slice(&fault_bytes);

        let events = c.process_bytes(&all_bytes);

        // Should have: StatusUpdate, Ready, FaultLogResponse
        assert!(
            events.len() >= 3,
            "expected at least 3 events, got {}: {:?}",
            events.len(),
            events
        );

        let has_status = events
            .iter()
            .any(|e| matches!(e, ControllerEvent::StatusUpdate(_)));
        let has_ready = events.iter().any(|e| matches!(e, ControllerEvent::Ready));
        let has_fault = events
            .iter()
            .any(|e| matches!(e, ControllerEvent::FaultLogResponse(_)));

        assert!(has_status, "should have StatusUpdate event");
        assert!(has_ready, "should have Ready event");
        assert!(has_fault, "should have FaultLogResponse event");

        // Both status and fault log should be stored
        assert!(c.last_status().is_some());
        assert!(c.last_fault_log().is_some());
    }

    // -- VAL-MQTT-031: Response handling does not interfere with registration --

    #[test]
    fn test_response_frames_during_registration() {
        let mut c = SpaController::new(); // NOT registered
        let mut sim = make_registered_sim();

        // Generate fault log response bytes
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let fault_bytes = sim.process_frame(&request_frames[0]).unwrap();

        // While NOT registered, feed registration query + fault log response bytes
        let reg_query = sim.generate_registration_query();
        let mut all_bytes = Vec::new();
        all_bytes.extend_from_slice(&reg_query);
        all_bytes.extend_from_slice(&fault_bytes);

        let events = c.process_bytes(&all_bytes);

        // Should handle registration (SendIdRequest event) and skip response processing
        // since the controller is not registered yet
        let has_reg_send = events
            .iter()
            .any(|e| matches!(e, ControllerEvent::RegistrationSend { .. }));
        assert!(has_reg_send, "should have registration event");

        // After processing, controller should not be registered (we only got the query)
        assert!(!c.is_registered());

        // Fault log should not be stored (was processed during registration phase)
        assert!(
            c.last_fault_log().is_none(),
            "fault log should not be stored during registration"
        );
    }

    #[test]
    fn test_response_frames_after_registration_preserved() {
        // Register controller, then process a config response — should work
        let mut c = SpaController::new();
        let mut sim = make_registered_sim();

        // Step 1: Simulate registration flow via raw bytes
        // Send NewClientQuery
        let query_bytes = sim.generate_registration_query();
        let events1 = c.process_bytes(&query_bytes);
        assert!(events1
            .iter()
            .any(|e| matches!(e, ControllerEvent::RegistrationSend { .. })));

        // Send ClientIdAssignment
        let id_request = FrameEncoder::encode([0xFE, 0xBF], &[0x01]).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&id_request);
        let assignment_bytes = sim.process_frame(&request_frames[0]).unwrap();
        let events2 = c.process_bytes(&assignment_bytes);

        // Should be registered now
        assert!(events2
            .iter()
            .any(|e| matches!(e, ControllerEvent::Registered { .. })));
        assert!(c.is_registered());

        // Step 2: Now process a configuration response — should succeed
        let (mt, payload) = Command::ConfigurationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let config_bytes = sim.process_frame(&request_frames[0]).unwrap();

        let events3 = c.process_bytes(&config_bytes);
        assert_eq!(events3.len(), 1);
        assert!(
            matches!(events3[0], ControllerEvent::ControlConfiguration(_)),
            "expected ControlConfiguration after registration"
        );
        assert!(c.last_control_config().is_some());
    }

    // -- VAL-MQTT-032: Response handling does not interfere with pump timers --

    #[test]
    fn test_response_handling_preserves_pump_timers() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Turn on Pump1 and start its timer
        sim.state.pumps[0] = launa_protocol::status::PumpState::Low;
        c.start_pump_timer(ToggleItem::Pump1);
        assert!(c.is_pump_timer_running(ToggleItem::Pump1));

        // Generate a status frame (which updates pump state tracking)
        let tick_bytes = sim.tick();
        c.process_bytes(&tick_bytes);

        // Timer should still be running after status update
        assert!(
            c.is_pump_timer_running(ToggleItem::Pump1),
            "pump timer should still be running after status update"
        );

        // Now generate a fault log response and process it
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let fault_bytes = sim.process_frame(&request_frames[0]).unwrap();

        c.process_bytes(&fault_bytes);

        // Pump timer should still be running — response processing doesn't affect it
        assert!(
            c.is_pump_timer_running(ToggleItem::Pump1),
            "pump timer should not be affected by fault log response"
        );

        // Verify fault log was still stored (response was handled)
        assert!(c.last_fault_log().is_some());
    }

    // -- VAL-MQTT-033: SpaController exposes last received response data --

    #[test]
    fn test_controller_exposes_last_responses() {
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();

        // Initially all accessors return None
        assert!(c.last_fault_log().is_none());
        assert!(c.last_filter_cycles().is_none());
        assert!(c.last_information().is_none());
        assert!(c.last_config().is_none());
        assert!(c.last_control_config().is_none());

        // Process a fault log response
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let fault_bytes = sim.process_frame(&request_frames[0]).unwrap();
        c.process_bytes(&fault_bytes);
        assert!(c.last_fault_log().is_some());

        // Process a filter cycles response
        let (mt, payload) = Command::FilterCyclesRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let filter_bytes = sim.process_frame(&request_frames[0]).unwrap();
        c.process_bytes(&filter_bytes);
        assert!(c.last_filter_cycles().is_some());

        // Process an information response
        let (mt, payload) = Command::InformationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let info_bytes = sim.process_frame(&request_frames[0]).unwrap();
        c.process_bytes(&info_bytes);
        assert!(c.last_information().is_some());

        // Process a configuration response
        let (mt, payload) = Command::ConfigurationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let config_bytes = sim.process_frame(&request_frames[0]).unwrap();
        c.process_bytes(&config_bytes);
        assert!(c.last_control_config().is_some());

        // All accessors should now return Some
        assert!(c.last_fault_log().is_some());
        assert!(c.last_filter_cycles().is_some());
        assert!(c.last_information().is_some());
        assert!(c.last_control_config().is_some());
    }

    #[test]
    fn test_controller_last_response_updates_on_new_data() {
        // Verify that receiving a second response replaces the first
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();

        // First fault log
        sim.set_fault_log_config(FaultLogConfig {
            fault_count: 1,
            ..FaultLogConfig::default()
        });
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();
        let request_frames = decoder.feed_slice(&request_encoded);
        let fault_bytes = sim.process_frame(&request_frames[0]).unwrap();
        c.process_bytes(&fault_bytes);
        assert_eq!(c.last_fault_log().unwrap().fault_count, 1);

        // Second fault log with different data
        sim.set_fault_log_config(FaultLogConfig {
            fault_count: 9,
            ..FaultLogConfig::default()
        });
        let (mt, payload) = Command::FaultLogRequest { entry: 0xFF }.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let fault_bytes = sim.process_frame(&request_frames[0]).unwrap();
        c.process_bytes(&fault_bytes);

        // Accessor should return the latest data
        assert_eq!(c.last_fault_log().unwrap().fault_count, 9);
    }

    #[test]
    fn test_controller_control_configuration_separate_from_config() {
        // Verify ControlConfiguration (0x2E) and ConfigurationResponse (0x94)
        // are stored in separate accessors
        let mut c = make_registered_controller();
        let mut sim = SpaSim::new();
        let mut decoder = launa_protocol::frame::FrameDecoder::new();

        // Request config (generates 0x2E ControlConfiguration)
        let (mt, payload) = Command::ConfigurationRequest.encode();
        let request_encoded = FrameEncoder::encode(mt, &payload).unwrap();
        let request_frames = decoder.feed_slice(&request_encoded);
        let config_bytes = sim.process_frame(&request_frames[0]).unwrap();
        let events = c.process_bytes(&config_bytes);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            ControllerEvent::ControlConfiguration(_)
        ));
        assert!(c.last_control_config().is_some());
        assert!(
            c.last_config().is_none(),
            "config should be None — only control_config was received"
        );
    }

    // ── Pump timer tests for pumps 4-6 and simultaneous operation (VAL-PL-011) ──

    /// Verify that PumpTimerManager supports all 6 pumps by starting timers
    /// for pumps 4, 5, and 6 individually.
    #[test]
    fn test_pump_timer_pump4_individual() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start pump 4 timer
        c.start_pump_timer(ToggleItem::Pump4);
        assert!(c.is_pump_timer_running(ToggleItem::Pump4));

        // Turn pump 4 on in sim state
        sim.state.pumps[3] = launa_protocol::status::PumpState::Low;

        // Generate status frame and process
        let tick_bytes = sim.tick();
        c.process_bytes(&tick_bytes);

        // Timer should still be running (not expired — default 20 min, only 1 tick)
        assert!(
            c.is_pump_timer_running(ToggleItem::Pump4),
            "pump 4 timer should still be running after 1 tick"
        );
    }

    #[test]
    fn test_pump_timer_pump5_individual() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start pump 5 timer
        c.start_pump_timer(ToggleItem::Pump5);
        assert!(c.is_pump_timer_running(ToggleItem::Pump5));

        // Turn pump 5 on in sim state
        sim.state.pumps[4] = launa_protocol::status::PumpState::High;

        let tick_bytes = sim.tick();
        c.process_bytes(&tick_bytes);

        assert!(
            c.is_pump_timer_running(ToggleItem::Pump5),
            "pump 5 timer should still be running after 1 tick"
        );
    }

    #[test]
    fn test_pump_timer_pump6_individual() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start pump 6 timer
        c.start_pump_timer(ToggleItem::Pump6);
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        // Turn pump 6 on in sim state
        sim.state.pumps[5] = launa_protocol::status::PumpState::Low;

        let tick_bytes = sim.tick();
        c.process_bytes(&tick_bytes);

        assert!(
            c.is_pump_timer_running(ToggleItem::Pump6),
            "pump 6 timer should still be running after 1 tick"
        );
    }

    #[test]
    fn test_pump_timer_cancel_pump4() {
        let mut c = make_registered_controller();

        // Start pump 4 timer
        c.start_pump_timer(ToggleItem::Pump4);
        assert!(c.is_pump_timer_running(ToggleItem::Pump4));

        // Cancel the timer
        c.cancel_pump_timer(ToggleItem::Pump4);
        assert!(
            !c.is_pump_timer_running(ToggleItem::Pump4),
            "pump 4 timer should not be running after cancel"
        );
    }

    #[test]
    fn test_pump_timer_cancel_pump5() {
        let mut c = make_registered_controller();

        c.start_pump_timer(ToggleItem::Pump5);
        assert!(c.is_pump_timer_running(ToggleItem::Pump5));

        c.cancel_pump_timer(ToggleItem::Pump5);
        assert!(!c.is_pump_timer_running(ToggleItem::Pump5));
    }

    #[test]
    fn test_pump_timer_cancel_pump6() {
        let mut c = make_registered_controller();

        c.start_pump_timer(ToggleItem::Pump6);
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        c.cancel_pump_timer(ToggleItem::Pump6);
        assert!(!c.is_pump_timer_running(ToggleItem::Pump6));
    }

    /// Test 3 simultaneous pump timers (pumps 4, 5, 6) all running at once.
    /// Each should track independently.
    #[test]
    fn test_pump_timer_simultaneous_pumps_4_5_6() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start timers for pumps 4, 5, and 6 simultaneously
        c.start_pump_timer(ToggleItem::Pump4);
        c.start_pump_timer(ToggleItem::Pump5);
        c.start_pump_timer(ToggleItem::Pump6);

        assert!(c.is_pump_timer_running(ToggleItem::Pump4));
        assert!(c.is_pump_timer_running(ToggleItem::Pump5));
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        // Turn all three pumps on in sim state
        sim.state.pumps[3] = launa_protocol::status::PumpState::Low;
        sim.state.pumps[4] = launa_protocol::status::PumpState::High;
        sim.state.pumps[5] = launa_protocol::status::PumpState::Low;

        // Process a status frame
        let tick_bytes = sim.tick();
        let events = c.process_bytes(&tick_bytes);

        // After 1 tick, none should have expired (default 20 min)
        assert!(c.is_pump_timer_running(ToggleItem::Pump4));
        assert!(c.is_pump_timer_running(ToggleItem::Pump5));
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        // No PumpExpired events
        let expired_count = events
            .iter()
            .filter(|e| matches!(e, ControllerEvent::PumpExpired(_)))
            .count();
        assert_eq!(expired_count, 0, "no timers should expire after 1 tick");
    }

    /// Test that pump 4 timer expires after 20 "ticks" (default duration).
    /// Each status frame ticks the pump timer by 1. Default duration is 1200 ticks (20 min).
    #[test]
    fn test_pump_timer_pump4_expires() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        c.start_pump_timer(ToggleItem::Pump4);
        sim.state.pumps[3] = launa_protocol::status::PumpState::Low;

        // Tick 1199 times — timer should not expire yet (default 20 min = 1200 ticks)
        for _ in 0..1199 {
            let tick_bytes = sim.tick();
            c.process_bytes(&tick_bytes);
        }
        assert!(
            c.is_pump_timer_running(ToggleItem::Pump4),
            "pump 4 timer should still be running after 1199 ticks"
        );

        // Tick once more — timer should expire
        let tick_bytes = sim.tick();
        let events = c.process_bytes(&tick_bytes);

        let has_expired = events.iter().any(|e| {
            matches!(
                e,
                ControllerEvent::PumpExpired(Command::ToggleItem(ToggleItem::Pump4))
            )
        });
        assert!(has_expired, "pump 4 timer should expire at tick 1200");
        assert!(!c.is_pump_timer_running(ToggleItem::Pump4));
    }

    /// Test that pump 5 timer expires after the expected duration.
    #[test]
    fn test_pump_timer_pump5_expires() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        c.start_pump_timer(ToggleItem::Pump5);
        sim.state.pumps[4] = launa_protocol::status::PumpState::Low;

        // Tick 1199 times
        for _ in 0..1199 {
            let tick_bytes = sim.tick();
            c.process_bytes(&tick_bytes);
        }
        assert!(c.is_pump_timer_running(ToggleItem::Pump5));

        // Tick once more — should expire
        let tick_bytes = sim.tick();
        let events = c.process_bytes(&tick_bytes);

        let has_expired = events.iter().any(|e| {
            matches!(
                e,
                ControllerEvent::PumpExpired(Command::ToggleItem(ToggleItem::Pump5))
            )
        });
        assert!(has_expired, "pump 5 timer should expire at tick 1200");
        assert!(!c.is_pump_timer_running(ToggleItem::Pump5));
    }

    /// Test that pump 6 timer expires after the expected duration.
    #[test]
    fn test_pump_timer_pump6_expires() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        c.start_pump_timer(ToggleItem::Pump6);
        sim.state.pumps[5] = launa_protocol::status::PumpState::Low;

        // Tick 1199 times
        for _ in 0..1199 {
            let tick_bytes = sim.tick();
            c.process_bytes(&tick_bytes);
        }
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        // Tick once more — should expire
        let tick_bytes = sim.tick();
        let events = c.process_bytes(&tick_bytes);

        let has_expired = events.iter().any(|e| {
            matches!(
                e,
                ControllerEvent::PumpExpired(Command::ToggleItem(ToggleItem::Pump6))
            )
        });
        assert!(has_expired, "pump 6 timer should expire at tick 1200");
        assert!(!c.is_pump_timer_running(ToggleItem::Pump6));
    }

    /// Test pump timer auto-cancellation when pump is turned off externally.
    /// After cancellation, advancing past the duration should NOT fire.
    #[test]
    fn test_pump_timer_cancel_on_external_off_pump5() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start pump 5 timer
        c.start_pump_timer(ToggleItem::Pump5);
        sim.state.pumps[4] = launa_protocol::status::PumpState::Low;

        // 5 ticks — timer running normally
        for _ in 0..5 {
            let tick_bytes = sim.tick();
            c.process_bytes(&tick_bytes);
        }
        assert!(c.is_pump_timer_running(ToggleItem::Pump5));

        // Pump 5 turns off externally
        sim.state.pumps[4] = launa_protocol::status::PumpState::Off;
        let tick_bytes = sim.tick();
        c.process_bytes(&tick_bytes);

        // Timer should be cancelled
        assert!(
            !c.is_pump_timer_running(ToggleItem::Pump5),
            "pump 5 timer should be cancelled when pump turns off externally"
        );

        // Advance well past the duration — should NOT re-fire
        for _ in 0..1300 {
            let tick_bytes = sim.tick();
            let events = c.process_bytes(&tick_bytes);
            let has_expired = events.iter().any(|e| {
                matches!(
                    e,
                    ControllerEvent::PumpExpired(Command::ToggleItem(ToggleItem::Pump5))
                )
            });
            assert!(!has_expired, "cancelled pump 5 timer should never fire");
        }
    }

    /// Test restart: cancel a timer then restart it — should fire at new duration.
    #[test]
    fn test_pump_timer_restart_pump6() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start and cancel
        c.start_pump_timer(ToggleItem::Pump6);
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));
        c.cancel_pump_timer(ToggleItem::Pump6);
        assert!(!c.is_pump_timer_running(ToggleItem::Pump6));

        // Restart
        c.start_pump_timer(ToggleItem::Pump6);
        assert!(
            c.is_pump_timer_running(ToggleItem::Pump6),
            "pump 6 timer should be running after restart"
        );

        // Turn pump on
        sim.state.pumps[5] = launa_protocol::status::PumpState::Low;

        // Advance past duration (1200 ticks) — should fire at new start time
        for _ in 0..1199 {
            let tick_bytes = sim.tick();
            c.process_bytes(&tick_bytes);
        }
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        let tick_bytes = sim.tick();
        let events = c.process_bytes(&tick_bytes);
        let has_expired = events.iter().any(|e| {
            matches!(
                e,
                ControllerEvent::PumpExpired(Command::ToggleItem(ToggleItem::Pump6))
            )
        });
        assert!(
            has_expired,
            "restarted pump 6 timer should fire at new duration"
        );
    }

    /// Test that simultaneous timers for pumps 1, 4, and 6 all fire independently.
    #[test]
    fn test_pump_timer_simultaneous_independent_fire() {
        let mut c = make_registered_controller();
        let mut sim = make_registered_sim();

        // Start timers for pumps 1, 4, and 6
        c.start_pump_timer(ToggleItem::Pump1);
        c.start_pump_timer(ToggleItem::Pump4);
        c.start_pump_timer(ToggleItem::Pump6);

        // Turn all three pumps on
        sim.state.pumps[0] = launa_protocol::status::PumpState::Low;
        sim.state.pumps[3] = launa_protocol::status::PumpState::Low;
        sim.state.pumps[5] = launa_protocol::status::PumpState::Low;

        // Tick 1199 times
        for _ in 0..1199 {
            let tick_bytes = sim.tick();
            c.process_bytes(&tick_bytes);
        }

        // All should still be running
        assert!(c.is_pump_timer_running(ToggleItem::Pump1));
        assert!(c.is_pump_timer_running(ToggleItem::Pump4));
        assert!(c.is_pump_timer_running(ToggleItem::Pump6));

        // Tick once more — all should expire
        let tick_bytes = sim.tick();
        let events = c.process_bytes(&tick_bytes);

        let expired_pumps: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ControllerEvent::PumpExpired(Command::ToggleItem(item)) => Some(*item),
                _ => None,
            })
            .collect();

        assert_eq!(
            expired_pumps.len(),
            3,
            "all 3 timers should fire simultaneously"
        );
        assert!(expired_pumps.contains(&ToggleItem::Pump1));
        assert!(expired_pumps.contains(&ToggleItem::Pump4));
        assert!(expired_pumps.contains(&ToggleItem::Pump6));

        // None should be running after expiry
        assert!(!c.is_pump_timer_running(ToggleItem::Pump1));
        assert!(!c.is_pump_timer_running(ToggleItem::Pump4));
        assert!(!c.is_pump_timer_running(ToggleItem::Pump6));
    }
}
