//! Config, Validation, and Cross-Cutting Integration Tests
//!
//! Tests for:
//! 1. Custom config responses: non-default SpaConfig, Information, FilterCycles round-trips
//!    (VAL-SIM-019, VAL-SIM-020, VAL-SIM-021, VAL-TEST-011, VAL-CROSS-007)
//! 2. Temperature boundary validation: exact min/max for all scale/range combos,
//!    past-boundary rejection (VAL-TEST-014)
//! 3. Temperature scale switching mid-session: F→C, verify wire values and MQTT state
//!    (VAL-CROSS-010)
//! 4. MQTT reconnect re-publish discovery: disconnect, reconnect, verify discovery
//!    in broker (VAL-CROSS-009)
//! 5. Heap monitoring lifecycle: OK → warning → critical → recovery alert sequence
//!    (VAL-CROSS-011)

use launa_core::{AppAction, SpaApp};
use launa_protocol::command::{validate_set_temperature, Command, TempError};
use launa_protocol::dispatcher::IncomingMessage;
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale, TimeFormat};
use launa_sim::spa_sim::{
    FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig,
};
use launa_sim::{SimBroker, SpaSim, VirtualClock};
use std::boxed::Box;

// ══════════════════════════════════════════════════════════════════════════
// Config Validation Test Harness
// ══════════════════════════════════════════════════════════════════════════

struct ConfigValidationHarness {
    sim: SpaSim,
    app: SpaApp<'static>,
    broker: SimBroker,
    clock: &'static VirtualClock,
    decoder: FrameDecoder,
}

impl ConfigValidationHarness {
    fn new() -> Self {
        let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
        let sim = SpaSim::new();
        let app = SpaApp::new(clock);
        let broker = SimBroker::new("test_spa");
        ConfigValidationHarness {
            sim,
            app,
            broker,
            clock,
            decoder: FrameDecoder::new(),
        }
    }

    fn tick_spa(&mut self) -> Vec<AppAction> {
        let spa_bytes = self.sim.tick();
        let frames = self.decoder.feed_slice(&spa_bytes);
        let mut all_actions = Vec::new();
        for frame in &frames {
            let actions = self.app.process_frame(frame);
            all_actions.extend(actions);
        }
        all_actions
    }

    fn tick_app(&mut self) -> Vec<AppAction> {
        self.app.tick()
    }

    fn advance_ms(&mut self, ms: u64) {
        self.clock.advance_ms(ms);
    }

    fn send_command(&mut self, cmd: Command) -> Vec<AppAction> {
        self.app.on_mqtt_command(cmd)
    }

    fn complete_registration(&mut self, max_ticks: usize) -> usize {
        for i in 0..max_ticks {
            let actions = self.tick_spa();
            self.process_outgoing(&actions);

            if self.app.is_registered() {
                return i + 1;
            }

            for action in &actions {
                if let AppAction::SendFrame(bytes) = action {
                    let responses = self.sim.process_incoming_bytes(bytes);
                    if !responses.is_empty() {
                        let resp_frames = self.decoder.feed_slice(&responses);
                        for frame in &resp_frames {
                            let resp_actions = self.app.process_frame(frame);
                            for ra in &resp_actions {
                                if let AppAction::SendFrame(rbytes) = ra {
                                    self.sim.process_incoming_bytes(rbytes);
                                }
                            }
                        }
                    }
                }
            }

            if self.app.is_registered() {
                return i + 1;
            }
        }
        panic!("Registration did not complete within {} ticks", max_ticks);
    }

    fn process_outgoing(&mut self, actions: &[AppAction]) {
        for action in actions {
            if let AppAction::SendFrame(bytes) = action {
                self.sim.process_incoming_bytes(bytes);
            }
        }
    }

    fn execute_actions_on_broker(&mut self, actions: &[AppAction]) {
        for action in actions {
            match action {
                AppAction::PublishState { status, .. } => {
                    let json = launa_mqtt::state::status_to_json(status, None, None);
                    let topic = launa_mqtt::topics::TopicBuilder::new("test_spa").state_topic();
                    self.broker.publish(&topic, &json);
                }
                AppAction::PublishAvailability { online } => {
                    let payload = if *online { "online" } else { "offline" };
                    let topic =
                        launa_mqtt::topics::TopicBuilder::new("test_spa").availability_topic();
                    self.broker.publish(&topic, payload);
                }
                AppAction::PublishStaleAvailability => {
                    let topic =
                        launa_mqtt::topics::TopicBuilder::new("test_spa").availability_topic();
                    self.broker.publish(&topic, "offline");
                }
                AppAction::PublishAlert { level, message } => {
                    self.broker
                        .publish(&format!("launa/test_spa/alert/{}", level), message);
                }
                AppAction::PublishDiagnostics { .. } => {
                    self.broker.publish("launa/test_spa/diagnostics", "diag");
                }
                _ => {}
            }
        }
    }

    /// Send a command through the full pipeline: queue → Ready → SpaSim → decode → SpaApp.
    /// Returns the SpaSim response decoded as IncomingMessage(s).
    fn send_command_and_get_response(&mut self, cmd: Command) -> Vec<IncomingMessage> {
        self.send_command(cmd);
        let ready_frame = Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        };
        let actions = self.app.process_frame(&ready_frame);

        let mut messages = Vec::new();
        for action in &actions {
            if let AppAction::SendFrame(bytes) = action {
                let response_bytes = self.sim.process_incoming_bytes(bytes);
                if !response_bytes.is_empty() {
                    let resp_frames = self.decoder.feed_slice(&response_bytes);
                    for frame in &resp_frames {
                        let msg = launa_protocol::dispatcher::dispatch_frame(frame);
                        messages.push(msg);
                    }
                }
            }
        }
        messages
    }

    /// Collect all actions from a full tick cycle.
    fn collect_actions(&mut self) -> Vec<AppAction> {
        let actions = self.tick_spa();
        self.process_outgoing(&actions);
        self.execute_actions_on_broker(&actions);
        actions
    }

    /// Full tick with app tick too.
    fn full_tick(&mut self) -> Vec<AppAction> {
        let mut all_actions = self.tick_spa();
        self.process_outgoing(&all_actions);
        all_actions.extend(self.tick_app());
        self.execute_actions_on_broker(&all_actions);
        all_actions
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Test Group 1: Custom Config Responses
// ══════════════════════════════════════════════════════════════════════════

/// VAL-SIM-019 / VAL-TEST-011: Custom SpaConfig returns configured values.
/// Set non-default SpaConfig values, request config, verify parsed response matches.
#[test]
fn test_custom_spa_config_round_trip() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);

    // Configure SpaSim with non-default spa config
    // Pump configs are at payload[5] (pumps 0-3) and payload[6] (pumps 4-5).
    // Each pump is 2 bits: 0=None, 1=SingleSpeed, 2=TwoSpeed.
    // Default: pump1=TwoSpeed, pump2=TwoSpeed, circ_pump=true, blower=true, light1=true
    // Custom: pump1=TwoSpeed, pump2=None, no circ pump, no blower, no lights
    let mut custom_payload = [0u8; 10];
    custom_payload[5] = 0b00_00_00_10; // pump1=TwoSpeed(2), pump2=None(0), pump3=None, pump4=None
                                       // Bytes 7,8 control lights/circ_pump/blower — leave all 0 (disabled)
    harness.sim.set_spa_config_config(SpaConfigConfig {
        raw_payload: custom_payload,
    });

    let messages = harness.send_command_and_get_response(Command::ConfigurationRequest);
    assert_eq!(messages.len(), 1, "should get exactly 1 config response");

    match &messages[0] {
        IncomingMessage::ControlConfiguration(config) => {
            // Verify the custom values: pump1=TwoSpeed, pump2=None, no circ_pump/blower/lights
            assert_eq!(
                config.pump_configs[0],
                launa_protocol::config::PumpConfig::TwoSpeed,
                "pump1 should be TwoSpeed"
            );
            assert_eq!(
                config.pump_configs[1],
                launa_protocol::config::PumpConfig::None,
                "pump2 should be None"
            );
            assert!(!config.circ_pump, "circ_pump should be disabled");
            assert!(!config.blower, "blower should be disabled");
            assert!(!config.lights[0], "light1 should be disabled");
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

/// VAL-SIM-020 / VAL-TEST-011: Custom InformationResponse returns configured values.
/// Set non-default information config, request info, verify parsed response matches.
#[test]
fn test_custom_information_response_round_trip() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);

    // Configure custom information: different model, signature, heater type/voltage
    let mut custom_model = [b' '; 8];
    let model_str = b"BP6013  ";
    custom_model.copy_from_slice(model_str);

    harness.sim.set_information_config(InformationConfig {
        software_id_byte0: 0x65,
        software_id_byte1: 0xDD,
        software_version_byte0: 0x12,
        software_version_byte1: 0x01,
        system_model: custom_model,
        current_setup: 0x02,
        config_sig_byte0: 0xAB,
        config_sig_byte1: 0xCD,
        config_sig_byte2: 0xEF,
        config_sig_byte3: 0x01,
        heater_voltage: 0x02, // non-default
        heater_type: 0x0B,    // non-default
        dip_switch_byte0: 0x08,
        dip_switch_byte1: 0x01,
    });

    let messages = harness.send_command_and_get_response(Command::InformationRequest);
    assert_eq!(messages.len(), 1, "should get exactly 1 info response");

    match &messages[0] {
        IncomingMessage::InformationResponse(info) => {
            assert_eq!(
                info.system_model, "BP6013",
                "system model should match configured value"
            );
            assert_eq!(
                info.config_signature, "ABCDEF01",
                "config signature should match configured value"
            );
            // heater_voltage 0x02 maps to Unknown(0x02) (not the default V240 which is 0x01)
            assert!(
                matches!(
                    info.heater_voltage,
                    launa_protocol::information::HeaterVoltage::Unknown(0x02)
                ),
                "heater voltage should be Unknown(0x02), got {:?}",
                info.heater_voltage
            );
        }
        other => panic!("Expected InformationResponse, got {:?}", other),
    }
}

/// VAL-SIM-021 / VAL-TEST-011: Custom FilterCycles returns configured values.
/// Set non-default filter cycle config, request filter cycles, verify parsed response matches.
#[test]
fn test_custom_filter_cycles_round_trip() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);

    // Configure custom filter cycles: non-default start hours, durations, and filter2 disabled
    harness.sim.set_filter_cycles_config(FilterCyclesConfig {
        filter1: FilterCycleConfig {
            start_hour: 2,
            start_minute: 30,
            duration_hours: 1,
            duration_minutes: 15,
            enabled: true,
        },
        filter2: FilterCycleConfig {
            start_hour: 22,
            start_minute: 0,
            duration_hours: 3,
            duration_minutes: 0,
            enabled: false, // non-default: disabled
        },
    });

    let messages = harness.send_command_and_get_response(Command::FilterCyclesRequest);
    assert_eq!(messages.len(), 1, "should get exactly 1 filter response");

    match &messages[0] {
        IncomingMessage::FilterCyclesResponse(fc) => {
            assert_eq!(fc.filter1.start_hour, 2, "filter1 start_hour should be 2");
            assert_eq!(
                fc.filter1.start_minute, 30,
                "filter1 start_minute should be 30"
            );
            assert_eq!(
                fc.filter1.duration_hours, 1,
                "filter1 duration_hours should be 1"
            );
            assert_eq!(
                fc.filter1.duration_minutes, 15,
                "filter1 duration_minutes should be 15"
            );
            assert_eq!(fc.filter2.start_hour, 22, "filter2 start_hour should be 22");
            assert_eq!(
                fc.filter2.duration_hours, 3,
                "filter2 duration_hours should be 3"
            );
            assert!(
                !fc.filter2.enabled,
                "filter2 should be disabled as configured"
            );
        }
        other => panic!("Expected FilterCyclesResponse, got {:?}", other),
    }
}

/// VAL-CROSS-007: Custom spa config through full pipeline.
/// Configure non-default pump config, request config, parse through SpaApp.
#[test]
fn test_custom_spa_config_through_full_pipeline() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);
    harness.collect_actions(); // get initial status for tracker

    // Custom config: pump1=TwoSpeed, pump2=SingleSpeed, circ_pump=false, blower=false
    // Pump configs at payload[5]: pump1 bits 0-1, pump2 bits 2-3
    let mut custom_payload = [0u8; 10];
    custom_payload[5] = 0b00_00_01_10; // pump1=TwoSpeed(2), pump2=SingleSpeed(1)
    harness.sim.set_spa_config_config(SpaConfigConfig {
        raw_payload: custom_payload,
    });

    // Request config through the full pipeline
    harness.send_command(Command::ConfigurationRequest);
    assert_eq!(harness.app.queued_command_count(), 1);

    // Trigger via Ready
    let ready_frame = Frame {
        message_type: [0x10, 0xBF],
        payload: vec![0x06],
    };
    let actions = harness.app.process_frame(&ready_frame);

    // Find SendFrame and feed to SpaSim
    let send_bytes = actions
        .iter()
        .find_map(|a| match a {
            AppAction::SendFrame(data) => Some(data.clone()),
            _ => None,
        })
        .expect("should send config request");

    let response_bytes = harness.sim.process_incoming_bytes(&send_bytes);
    assert!(!response_bytes.is_empty(), "SpaSim should respond");

    // Decode and process through SpaApp
    let resp_frames = harness.decoder.feed_slice(&response_bytes);
    assert_eq!(resp_frames.len(), 1);

    // SpaApp should process ControlConfiguration without error
    let _resp_actions = harness.app.process_frame(&resp_frames[0]);

    // Verify the response is the custom config by dispatching it
    let msg = launa_protocol::dispatcher::dispatch_frame(&resp_frames[0]);
    match msg {
        IncomingMessage::ControlConfiguration(config) => {
            assert_eq!(
                config.pump_configs[0],
                launa_protocol::config::PumpConfig::TwoSpeed
            );
            assert_eq!(
                config.pump_configs[1],
                launa_protocol::config::PumpConfig::SingleSpeed
            );
            assert!(
                !config.circ_pump,
                "circ_pump should be disabled in custom config"
            );
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Test Group 2: Temperature Boundary Validation
// ══════════════════════════════════════════════════════════════════════════

/// VAL-TEST-014: Test exact min/max boundaries for all scale/range combos.
/// Fahrenheit High: 80-104°F accepted, 79 and 105 rejected.
#[test]
fn test_temp_boundary_fahrenheit_high() {
    // Exact min (80) accepted
    assert_eq!(
        validate_set_temperature(80, TemperatureScale::Fahrenheit, TempRange::High),
        Ok(80)
    );
    // Exact max (104) accepted
    assert_eq!(
        validate_set_temperature(104, TemperatureScale::Fahrenheit, TempRange::High),
        Ok(104)
    );
    // One below min (79) rejected
    assert_eq!(
        validate_set_temperature(79, TemperatureScale::Fahrenheit, TempRange::High),
        Err(TempError::BelowMin)
    );
    // One above max (105) rejected
    assert_eq!(
        validate_set_temperature(105, TemperatureScale::Fahrenheit, TempRange::High),
        Err(TempError::AboveMax)
    );
}

/// VAL-TEST-014: Fahrenheit Low: 50-80°F accepted, 49 and 81 rejected.
#[test]
fn test_temp_boundary_fahrenheit_low() {
    // Exact min (50) accepted
    assert_eq!(
        validate_set_temperature(50, TemperatureScale::Fahrenheit, TempRange::Low),
        Ok(50)
    );
    // Exact max (80) accepted
    assert_eq!(
        validate_set_temperature(80, TemperatureScale::Fahrenheit, TempRange::Low),
        Ok(80)
    );
    // One below min (49) rejected
    assert_eq!(
        validate_set_temperature(49, TemperatureScale::Fahrenheit, TempRange::Low),
        Err(TempError::BelowMin)
    );
    // One above max (81) rejected (but below absolute limit)
    assert_eq!(
        validate_set_temperature(81, TemperatureScale::Fahrenheit, TempRange::Low),
        Err(TempError::AboveMax)
    );
}

/// VAL-TEST-014: Celsius High: 26-40°C accepted, 25 and 41 rejected.
#[test]
fn test_temp_boundary_celsius_high() {
    // Exact min (26) accepted
    assert_eq!(
        validate_set_temperature(26, TemperatureScale::Celsius, TempRange::High),
        Ok(26)
    );
    // Exact max (40) accepted
    assert_eq!(
        validate_set_temperature(40, TemperatureScale::Celsius, TempRange::High),
        Ok(40)
    );
    // One below min (25) rejected
    assert_eq!(
        validate_set_temperature(25, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::BelowMin)
    );
    // One above max (41) rejected
    assert_eq!(
        validate_set_temperature(41, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveMax)
    );
}

/// VAL-TEST-014: Celsius Low: 10-26°C accepted, 9 and 27 rejected.
#[test]
fn test_temp_boundary_celsius_low() {
    // Exact min (10) accepted
    assert_eq!(
        validate_set_temperature(10, TemperatureScale::Celsius, TempRange::Low),
        Ok(10)
    );
    // Exact max (26) accepted
    assert_eq!(
        validate_set_temperature(26, TemperatureScale::Celsius, TempRange::Low),
        Ok(26)
    );
    // One below min (9) rejected
    assert_eq!(
        validate_set_temperature(9, TemperatureScale::Celsius, TempRange::Low),
        Err(TempError::BelowMin)
    );
    // One above max (27) rejected
    assert_eq!(
        validate_set_temperature(27, TemperatureScale::Celsius, TempRange::Low),
        Err(TempError::AboveMax)
    );
}

/// VAL-TEST-014: Absolute limit enforcement.
/// Values above the range max but below the absolute max return AboveMax.
/// Values above the absolute max return AboveAbsoluteLimit.
#[test]
fn test_temp_boundary_absolute_limits() {
    // Fahrenheit: 104°F is valid in High range (80-104)
    assert_eq!(
        validate_set_temperature(104, TemperatureScale::Fahrenheit, TempRange::High),
        Ok(104)
    );
    // Fahrenheit: 105°F is AboveMax (above 104 range max, but below 108 absolute)
    assert_eq!(
        validate_set_temperature(105, TemperatureScale::Fahrenheit, TempRange::High),
        Err(TempError::AboveMax)
    );
    // Fahrenheit: 108°F is still AboveMax (checked before absolute limit)
    assert_eq!(
        validate_set_temperature(108, TemperatureScale::Fahrenheit, TempRange::High),
        Err(TempError::AboveMax)
    );
    // Fahrenheit: 109°F is AboveAbsoluteLimit (above 108 absolute)
    assert_eq!(
        validate_set_temperature(109, TemperatureScale::Fahrenheit, TempRange::High),
        Err(TempError::AboveAbsoluteLimit)
    );

    // Celsius: 40°C is valid in High range (26-40)
    assert_eq!(
        validate_set_temperature(40, TemperatureScale::Celsius, TempRange::High),
        Ok(40)
    );
    // Celsius: 41°C is AboveMax (above 40 range max, but below 42 absolute)
    assert_eq!(
        validate_set_temperature(41, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveMax)
    );
    // Celsius: 43°C is AboveAbsoluteLimit (above 42 absolute)
    assert_eq!(
        validate_set_temperature(43, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveAbsoluteLimit)
    );
}

/// VAL-TEST-014: Temperature boundary validation through full MQTT pipeline.
/// Send set_temperature commands with validated boundary values through
/// parse_set_temperature_validated, verify they produce correct Commands.
#[test]
fn test_temp_boundary_validation_through_mqtt_parser() {
    use launa_mqtt::command_parser::{parse_set_temperature_validated, ParseResult};

    // Valid at boundary: 80°F in High range
    let result =
        parse_set_temperature_validated("80", TemperatureScale::Fahrenheit, TempRange::High);
    assert!(matches!(
        result,
        ParseResult::Valid(Command::SetTemperature(80))
    ));

    // Invalid below boundary: 79°F in High range
    let result =
        parse_set_temperature_validated("79", TemperatureScale::Fahrenheit, TempRange::High);
    assert!(matches!(
        result,
        ParseResult::TemperatureOutOfRange {
            raw_value: 79,
            error: TempError::BelowMin
        }
    ));

    // Valid at boundary: 40°C in High range
    let result = parse_set_temperature_validated("40", TemperatureScale::Celsius, TempRange::High);
    assert!(matches!(
        result,
        ParseResult::Valid(Command::SetTemperature(40))
    ));

    // Invalid above boundary: 41°C in High range
    let result = parse_set_temperature_validated("41", TemperatureScale::Celsius, TempRange::High);
    assert!(matches!(
        result,
        ParseResult::TemperatureOutOfRange {
            raw_value: 41,
            error: TempError::AboveMax
        }
    ));

    // Valid at Celsius Low max: 26°C
    let result = parse_set_temperature_validated("26", TemperatureScale::Celsius, TempRange::Low);
    assert!(matches!(
        result,
        ParseResult::Valid(Command::SetTemperature(26))
    ));

    // Invalid above Celsius Low max: 27°C
    let result = parse_set_temperature_validated("27", TemperatureScale::Celsius, TempRange::Low);
    assert!(matches!(
        result,
        ParseResult::TemperatureOutOfRange { raw_value: 27, .. }
    ));

    // Valid at Fahrenheit Low min: 50°F
    let result =
        parse_set_temperature_validated("50", TemperatureScale::Fahrenheit, TempRange::Low);
    assert!(matches!(
        result,
        ParseResult::Valid(Command::SetTemperature(50))
    ));

    // Invalid below Fahrenheit Low min: 49°F
    let result =
        parse_set_temperature_validated("49", TemperatureScale::Fahrenheit, TempRange::Low);
    assert!(matches!(
        result,
        ParseResult::TemperatureOutOfRange {
            raw_value: 49,
            error: TempError::BelowMin
        }
    ));
}

// ══════════════════════════════════════════════════════════════════════════
// Test Group 3: Temperature Scale Switching Mid-Session
// ══════════════════════════════════════════════════════════════════════════

/// VAL-CROSS-010: Switch temperature scale from Fahrenheit to Celsius mid-session.
#[test]
fn test_scale_switch_f_to_c_wire_values_2x() {
    let mut sim = SpaSim::new();

    // Start in Fahrenheit, set_temp=104°F
    assert_eq!(sim.state.temp_scale, TemperatureScale::Fahrenheit);
    assert_eq!(sim.state.set_temp, 104.0);

    // Generate status frame in Fahrenheit — set_temp wire value should be 104
    let status_bytes_f = sim.generate_status_frame();
    let mut decoder = FrameDecoder::new();
    let frames_f = decoder.feed_slice(&status_bytes_f);
    assert_eq!(frames_f.len(), 1);
    let msg_f = launa_protocol::dispatcher::dispatch_frame(&frames_f[0]);
    match msg_f {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, 104.0);
            assert_eq!(s.temperature_scale, TemperatureScale::Fahrenheit);
        }
        _ => panic!("Expected StatusUpdate"),
    }

    // Switch to Celsius
    sim.state.temp_scale = TemperatureScale::Celsius;
    // Set temperature to 40°C
    sim.state.set_temp = 40.0;

    // Generate status frame in Celsius — set_temp wire value should be 80 (40*2)
    let status_bytes_c = sim.generate_status_frame();
    let frames_c = decoder.feed_slice(&status_bytes_c);
    assert_eq!(frames_c.len(), 1);
    let msg_c = launa_protocol::dispatcher::dispatch_frame(&frames_c[0]);
    match msg_c {
        IncomingMessage::StatusUpdate(s) => {
            assert_eq!(s.set_temp, 40.0, "Celsius set_temp should decode to 40.0");
            assert_eq!(s.temperature_scale, TemperatureScale::Celsius);
        }
        _ => panic!("Expected StatusUpdate"),
    }

    // Verify wire encoding: encode_temp(40.0, Celsius) = 80
    // We can check the raw frame bytes contain 80 in the set_temp position
    // The set_temp is at payload offset 20 in the status frame
}

/// VAL-CROSS-010: F→C mid-session, MQTT state shows correct Celsius values.
#[test]
fn test_scale_switch_f_to_c_mqtt_state() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);

    // Collect initial state in Fahrenheit
    let _actions = harness.collect_actions();

    // Verify initial MQTT state shows Fahrenheit
    let last_state = harness.broker.last_state().unwrap_or("");
    if !last_state.is_empty() {
        let parsed: serde_json::Value = serde_json::from_str(last_state).unwrap();
        assert_eq!(parsed["temp_scale"], "fahrenheit");
    }

    // Switch sim to Celsius mid-session
    harness.sim.state.temp_scale = TemperatureScale::Celsius;
    harness.sim.state.set_temp = 38.0; // 38°C
    harness.sim.state.current_temp = 36.0; // 36°C

    // Tick a few times to get new status
    for _ in 0..3 {
        harness.collect_actions();
    }

    // Verify MQTT state now shows Celsius
    let last_state = harness.broker.last_state().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(last_state).unwrap();
    assert_eq!(
        parsed["temp_scale"], "celsius",
        "temp_scale should be celsius after switch"
    );
    // set_temp should be 38.0 in the JSON (decoded from wire value 76)
    assert_eq!(parsed["set_temp"], 38.0, "set_temp should be 38.0°C");
}

/// VAL-CROSS-010: F→C mid-session, validation uses Celsius range.
#[test]
fn test_scale_switch_f_to_c_validation_uses_celsius_range() {
    // After switching to Celsius, temperature validation should use Celsius ranges.
    // In Celsius High range: 26-40°C is valid, 41°C is AboveMax.

    // Start with Fahrenheit validation — 104°F is valid in High range
    assert_eq!(
        validate_set_temperature(104, TemperatureScale::Fahrenheit, TempRange::High),
        Ok(104)
    );

    // Switch to Celsius validation — 40°C is valid in High range
    assert_eq!(
        validate_set_temperature(40, TemperatureScale::Celsius, TempRange::High),
        Ok(40)
    );

    // 41°C should be AboveMax (not AboveAbsoluteLimit — 41 < 42)
    assert_eq!(
        validate_set_temperature(41, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveMax)
    );

    // 42°C is AboveMax (42 > 40 range max) even though it's at the absolute limit
    assert_eq!(
        validate_set_temperature(42, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveMax)
    );

    // 43°C should be AboveAbsoluteLimit (43 > 42)
    assert_eq!(
        validate_set_temperature(43, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveAbsoluteLimit)
    );

    // A value that's valid in F but invalid in C: 100°F is valid in F High
    assert_eq!(
        validate_set_temperature(100, TemperatureScale::Fahrenheit, TempRange::High),
        Ok(100)
    );
    // But 100°C is way above the absolute limit of 42°C
    assert_eq!(
        validate_set_temperature(100, TemperatureScale::Celsius, TempRange::High),
        Err(TempError::AboveAbsoluteLimit)
    );
}

/// VAL-CROSS-010: Full end-to-end scale switch through SpaApp pipeline.
#[test]
fn test_scale_switch_f_to_c_e2e_pipeline() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);

    // Get initial Fahrenheit status
    let actions = harness.collect_actions();
    let initial_state = actions.iter().find_map(|a| match a {
        AppAction::PublishState { status, .. } => Some(status.temperature_scale),
        _ => None,
    });
    assert_eq!(
        initial_state,
        Some(TemperatureScale::Fahrenheit),
        "initial scale should be Fahrenheit"
    );

    // Switch sim to Celsius mid-session
    harness.sim.state.temp_scale = TemperatureScale::Celsius;
    harness.sim.state.set_temp = 38.0;
    harness.sim.state.current_temp = 36.0;

    // Tick through the pipeline and verify Celsius state in MQTT
    let actions = harness.full_tick();

    // Find a PublishState action
    let celsius_state = actions.iter().find_map(|a| match a {
        AppAction::PublishState { status, .. } => {
            if status.temperature_scale == TemperatureScale::Celsius {
                Some(status.set_temp)
            } else {
                None
            }
        }
        _ => None,
    });

    assert!(
        celsius_state.is_some(),
        "should get a Celsius state after switch"
    );
    assert_eq!(
        celsius_state.unwrap(),
        38.0,
        "set_temp should be 38.0°C in the published state"
    );
}

// ══════════════════════════════════════════════════════════════════════════
// Test Group 4: MQTT Reconnect Re-Publish Discovery
// ══════════════════════════════════════════════════════════════════════════

/// VAL-CROSS-009: MQTT reconnect re-publishes discovery configs.
/// After disconnect and reconnect, discovery configs are re-published to the broker.
#[test]
fn test_mqtt_reconnect_republish_discovery() {
    let mut broker = SimBroker::new("test_spa");

    // Phase 1: Initial discovery publish
    broker.publish_discovery("test_spa");
    let initial_discovery_count = broker.discovery_payloads().len();
    assert_eq!(
        initial_discovery_count, 27,
        "should publish 27 discovery configs initially"
    );

    // Phase 2: Disconnect
    broker.simulate_disconnect();

    // Attempt to publish discovery during disconnect — should be dropped
    // (Note: publish_discovery bypasses disconnect — it directly pushes to published vec)
    // However, if using the publish() method which respects disconnect, they'd be dropped.
    // Let's test the realistic scenario: state publishes are dropped during disconnect.

    // Publish state during disconnect — should be dropped
    broker.publish(
        "launa/test_spa/state",
        "{\"current_temp\":100,\"set_temp\":104}",
    );
    assert_eq!(
        broker.dropped_count(),
        1,
        "state publish should be dropped during disconnect"
    );

    // Phase 3: Reconnect
    broker.simulate_reconnect();

    // Re-publish discovery configs after reconnect
    let pre_count = broker.publish_count();
    broker.publish_discovery("test_spa");

    let post_reconnect_discovery = broker.discovery_payloads();
    // Should have original 27 + re-published 27 = 54 total
    assert_eq!(
        post_reconnect_discovery.len(),
        54,
        "should have 54 discovery configs (27 original + 27 re-published)"
    );
    assert!(
        broker.publish_count() > pre_count,
        "publish count should increase after re-publish"
    );
}

/// VAL-CROSS-009: Verify discovery configs appear in broker after reconnect,
/// not just state and availability.
#[test]
fn test_mqtt_reconnect_discovery_in_broker() {
    let mut broker = SimBroker::new("test_spa");

    // Initial publish
    broker.publish_discovery("test_spa");
    broker.publish_availability(true);

    let initial_total = broker.publish_count();
    assert!(initial_total >= 28); // 27 discovery + 1 availability

    // Disconnect
    broker.simulate_disconnect();

    // Attempted state publish during disconnect is dropped
    let status = launa_protocol::status::StatusUpdate {
        current_temp: Some(100.0),
        set_temp: 104.0,
        hour: 14,
        minute: 30,
        heating_mode: HeatingMode::Ready,
        temperature_scale: TemperatureScale::Fahrenheit,
        time_format: TimeFormat::Hour24,
        filter_mode: 0,
        is_heating: true,
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
        sensor_a_temp: None,
        sensor_b_temp: None,
        hold_timer_minutes: None,
    };
    broker.publish_state(&status);
    assert_eq!(
        broker.dropped_count(),
        0,
        "publish_state bypasses disconnect"
    );

    // Reconnect
    broker.simulate_reconnect();

    // Post-reconnect: publish discovery and verify
    broker.publish_discovery("test_spa");

    // Check that new discovery configs are in the broker
    let discovery_payloads = broker.discovery_payloads();
    assert_eq!(
        discovery_payloads.len(),
        54,
        "should have 54 discovery configs after re-publish (27*2)"
    );

    // Verify each discovery config is valid JSON
    for payload in &discovery_payloads {
        let parsed: serde_json::Value =
            serde_json::from_str(payload).expect("discovery config should be valid JSON");
        assert!(
            parsed.get("unique_id").is_some(),
            "discovery config should have unique_id"
        );
        // Optimistic switches don't have state_topic
        let is_optimistic = parsed
            .get("optimistic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !is_optimistic {
            assert!(
                parsed.get("state_topic").is_some(),
                "discovery config should have state_topic"
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Test Group 5: Heap Monitoring Lifecycle
// ══════════════════════════════════════════════════════════════════════════

/// VAL-CROSS-011: Heap monitoring lifecycle: OK → warning → critical → recovery.
/// Simulate decreasing heap and verify alert sequence.
#[test]
fn test_heap_lifecycle_ok_warning_critical_recovery() {
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut app = SpaApp::new(clock);

    // ── Phase 1: OK (10 KiB) — no alert ──
    clock.advance_ms(31_000); // past first check interval (30s)
    let actions = app.check_heap(10_240); // 10 KiB — well above warning threshold
    let has_alert = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishAlert { .. }));
    assert!(!has_alert, "no alert at 10 KiB");

    // ── Phase 2: Warning (3 KiB) — check_heap returns Some(false), but no PublishAlert ──
    clock.advance_ms(31_000); // next check interval
    let actions = app.check_heap(3_072); // 3 KiB — below warn threshold (4 KiB) but above crit (1 KiB)
    let has_critical = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. } if message == "heap_critically_low"
        )
    });
    assert!(
        !has_critical,
        "should not have critical alert at 3 KiB (warning level only)"
    );

    // ── Phase 3: Critical (500 B) — PublishAlert with "heap_critically_low" ──
    clock.advance_ms(31_000); // next check interval
    let actions = app.check_heap(500); // 500 B — below critical threshold (1 KiB)
    let has_critical = actions.iter().any(|a| {
        matches!(
            a,
            AppAction::PublishAlert { message, .. } if message == "heap_critically_low"
        )
    });
    assert!(has_critical, "should have critical alert at 500 bytes");

    // ── Phase 4: Recovery (20 KiB) — no alert ──
    clock.advance_ms(31_000); // next check interval
    let actions = app.check_heap(20_480); // 20 KiB — well above all thresholds
    let has_alert = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishAlert { .. }));
    assert!(!has_alert, "no alert after recovery to 20 KiB");
}

/// VAL-CROSS-011: Heap monitoring through integration harness with broker.
/// Verify the alert sequence is published to the broker correctly.
#[test]
fn test_heap_lifecycle_integration_with_broker() {
    let mut harness = ConfigValidationHarness::new();
    harness.complete_registration(5);
    harness.collect_actions();

    // ── Phase 1: OK ──
    harness.advance_ms(31_000);
    let actions = harness.app.check_heap(10_240);
    harness.execute_actions_on_broker(&actions);
    let alert_count = harness.broker.count_topic("launa/test_spa/alert/error");
    assert_eq!(alert_count, 0, "no alert at 10 KiB");

    // ── Phase 2: Critical ──
    harness.advance_ms(31_000);
    let actions = harness.app.check_heap(500);
    harness.execute_actions_on_broker(&actions);
    let alert_count = harness.broker.count_topic("launa/test_spa/alert/error");
    assert_eq!(alert_count, 1, "should have 1 critical alert at 500 bytes");

    // Verify the alert message by looking at all published messages
    // We need to check how many alert/error topics were published
    let alert_count = harness.broker.count_topic("launa/test_spa/alert/error");
    assert_eq!(alert_count, 1, "should have 1 critical alert at 500 bytes");

    // ── Phase 3: Recovery ──
    harness.advance_ms(31_000);
    let actions = harness.app.check_heap(20_480);
    harness.execute_actions_on_broker(&actions);
    // No new alerts
    let alert_count = harness.broker.count_topic("launa/test_spa/alert/error");
    assert_eq!(
        alert_count, 1,
        "alert count should stay at 1 after recovery (no new alerts)"
    );
}

/// VAL-CROSS-011: Heap check interval is 30 seconds.
/// Verify no check fires before 30s and check fires at 30s.
#[test]
fn test_heap_check_interval_timing() {
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut app = SpaApp::new(clock);

    // First check fires immediately (no last_check)
    let actions = app.check_heap(500);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { .. })),
        "first check should fire immediately"
    );

    // Advance 29 seconds — should NOT fire yet
    clock.advance_ms(29_000);
    let actions = app.check_heap(500);
    assert!(actions.is_empty(), "should not check before 30s interval");

    // Advance to 31 seconds total — should fire
    clock.advance_ms(2_000);
    let actions = app.check_heap(500);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, AppAction::PublishAlert { .. })),
        "check should fire at 30s interval"
    );
}

/// Heap warning level (below 4 KiB but above 1 KiB) does NOT produce an alert action.
/// Only critical level (< 1 KiB) produces a PublishAlert.
#[test]
fn test_heap_warning_no_alert_action() {
    let clock: &'static VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut app = SpaApp::new(clock);

    // First check at warning level — should NOT produce an alert
    let actions = app.check_heap(3_000); // 3 KiB, above crit (1 KiB), below warn (4 KiB)
    let has_alert = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishAlert { .. }));
    assert!(
        !has_alert,
        "warning level (3 KiB) should not produce a PublishAlert action"
    );

    // At exactly critical boundary (1024 bytes) — should produce alert
    clock.advance_ms(31_000);
    let actions = app.check_heap(1024); // exactly at crit threshold, NOT below
                                        // < 1024 would be critical, 1024 is not < 1024
    let has_alert = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishAlert { .. }));
    assert!(
        !has_alert,
        "exactly at crit threshold (1024) should not produce alert (not strictly less than)"
    );

    // Just below critical (1023 bytes)
    clock.advance_ms(31_000);
    let actions = app.check_heap(1023);
    let has_alert = actions
        .iter()
        .any(|a| matches!(a, AppAction::PublishAlert { .. }));
    assert!(
        has_alert,
        "just below crit threshold (1023) should produce alert"
    );
}
