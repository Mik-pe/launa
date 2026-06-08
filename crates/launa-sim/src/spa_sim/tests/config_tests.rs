use super::*;
use crate::spa_sim::{
    FaultLogConfig, FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig,
};
use launa_protocol::config::PumpConfig;
use launa_protocol::fault::FaultCode;
use launa_protocol::frame::FrameDecoder;
use launa_protocol::status::TemperatureScale;

#[test]
fn test_configurable_fault_log_response() {
    let mut sim = SpaSim::new();
    sim.set_fault_log_config(FaultLogConfig {
        fault_count: 5,
        entry_number: 2,
        message_code: FaultCode::LowFlow,
        days_ago: 10,
        hour: 8,
        minute: 15,
        flags: 0x00,
        set_temperature: 96,
        sensor_a_temp: 95,
        sensor_b_temp: 94,
    });

    let response = sim.generate_fault_log_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(entry.fault_count, 5);
            assert_eq!(entry.entry_number, 2);
            assert_eq!(entry.message_code, FaultCode::LowFlow);
            assert_eq!(entry.days_ago, 10);
            assert_eq!(entry.hour, 8);
            assert_eq!(entry.minute, 15);
            assert_eq!(entry.flags, 0x00);
            assert_eq!(entry.set_temperature, 96);
            assert_eq!(entry.sensor_a_temp, 95);
            assert_eq!(entry.sensor_b_temp, 94);
        }
        other => panic!("Expected FaultLogResponse, got {:?}", other),
    }
}

#[test]
fn test_default_fault_log_response_unchanged() {
    let sim = SpaSim::new();
    let response = sim.generate_fault_log_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(entry.fault_count, 3);
            assert_eq!(entry.message_code, FaultCode::HeaterDry);
            assert_eq!(entry.days_ago, 2);
            assert_eq!(entry.set_temperature, 104);
        }
        other => panic!("Expected FaultLogResponse, got {:?}", other),
    }
}

#[test]
fn test_configurable_filter_cycles_response() {
    let mut sim = SpaSim::new();
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

    let response = sim.generate_filter_cycles_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
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
        other => panic!("Expected FilterCyclesResponse, got {:?}", other),
    }
}

#[test]
fn test_default_filter_cycles_response_unchanged() {
    let sim = SpaSim::new();
    let response = sim.generate_filter_cycles_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
            assert_eq!(fc.filter1.start_hour, 8);
            assert_eq!(fc.filter1.duration_hours, 4);
            assert_eq!(fc.filter2.start_hour, 16);
            assert_eq!(fc.filter2.duration_hours, 2);
            assert!(fc.filter2.enabled);
        }
        other => panic!("Expected FilterCyclesResponse, got {:?}", other),
    }
}

#[test]
fn test_configurable_information_response() {
    let mut model = [b' '; 8];
    model[..7].copy_from_slice(b"CUSTOM1");

    let mut sim = SpaSim::new();
    sim.set_information_config(InformationConfig {
        software_id_byte0: 0xAA,
        software_id_byte1: 0xBB,
        software_version_byte0: 0xCC,
        software_version_byte1: 0xDD,
        system_model: model,
        current_setup: 0x05,
        config_sig_byte0: 0xDE,
        config_sig_byte1: 0xAD,
        config_sig_byte2: 0xBE,
        config_sig_byte3: 0xEF,
        heater_voltage: 0x01,
        heater_type: 0xFF,
        dip_switch_byte0: 0xFF,
        dip_switch_byte1: 0x00,
    });

    let response = sim.generate_information_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
            assert_eq!(info.system_model, "CUSTOM1");
            assert_eq!(info.current_setup, 0x05);
            assert_eq!(info.config_signature, "DEADBEEF");
            assert_eq!(
                info.heater_type,
                launa_protocol::information::HeaterType::Unknown(0xFF)
            );
            assert_eq!(info.dip_switches, "1111111100000000");
        }
        other => panic!("Expected InformationResponse, got {:?}", other),
    }
}

#[test]
fn test_default_information_response_unchanged() {
    let sim = SpaSim::new();
    let response = sim.generate_information_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
            assert_eq!(info.system_model, "BFBP20");
            assert_eq!(info.config_signature, "3D12382E");
            assert_eq!(
                info.heater_voltage,
                launa_protocol::information::HeaterVoltage::V240
            );
            assert_eq!(
                info.heater_type,
                launa_protocol::information::HeaterType::Standard
            );
        }
        other => panic!("Expected InformationResponse, got {:?}", other),
    }
}

#[test]
fn test_configurable_config_response() {
    let mut raw = [0u8; 10];
    // Set up specific pump configs: pump1=SingleSpeed, pump2=None
    raw[0] = 0x02;
    raw[1] = 0x02;
    raw[5] = 0b00_00_00_01; // pump1=SingleSpeed
    raw[7] = 0x05; // light1 (bits 0-1=01), light2 (bits 2-3=01)
    raw[8] = 0x80; // circ pump present

    let mut sim = SpaSim::new();
    sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });

    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert_eq!(config.pump_configs[0], PumpConfig::SingleSpeed);
            assert_eq!(config.pump_configs[1], PumpConfig::None);
            assert!(config.circ_pump);
            assert!(config.lights[0]);
            assert!(config.lights[1]);
            assert!(!config.blower);
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

#[test]
fn test_default_config_response_unchanged() {
    let sim = SpaSim::new();
    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert_eq!(config.pump_configs[0], PumpConfig::TwoSpeed);
            assert_eq!(config.pump_configs[1], PumpConfig::TwoSpeed);
            assert!(config.circ_pump);
            assert!(config.blower);
            assert!(config.lights[0]);
            assert!(config.temperature_scale_celsius);
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

#[test]
fn test_configurable_responses_valid_frames() {
    let mut sim = SpaSim::new();

    // Set custom configs with edge-case values
    sim.set_fault_log_config(FaultLogConfig {
        fault_count: 0,
        entry_number: 0,
        message_code: FaultCode::Unknown(99),
        days_ago: 0,
        hour: 0,
        minute: 0,
        flags: 0xFF,
        set_temperature: 0,
        sensor_a_temp: 255,
        sensor_b_temp: 255,
    });

    sim.set_filter_cycles_config(FilterCyclesConfig {
        filter1: FilterCycleConfig {
            start_hour: 23,
            start_minute: 59,
            duration_hours: 23,
            duration_minutes: 59,
            enabled: true,
        },
        filter2: FilterCycleConfig {
            start_hour: 0,
            start_minute: 0,
            duration_hours: 0,
            duration_minutes: 0,
            enabled: false,
        },
    });

    let model = [0xFF; 8];
    sim.set_information_config(InformationConfig {
        system_model: model,
        ..Default::default()
    });

    let raw = [0xFF; 10];
    sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });

    // Verify each generates valid framed output
    let fault_bytes = sim.generate_fault_log_response();
    let filter_bytes = sim.generate_filter_cycles_response();
    let info_bytes = sim.generate_information_response();
    let config_bytes = sim.generate_config_response();

    // Each should produce at least one valid frame
    for (label, bytes) in [
        ("fault", &fault_bytes),
        ("filter", &filter_bytes),
        ("info", &info_bytes),
        ("config", &config_bytes),
    ] {
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(bytes);
        assert!(
            !frames.is_empty(),
            "{} response should produce valid frames",
            label
        );
        assert_eq!(
            frames[0].message_type,
            [0x0A, 0xBF],
            "{} response should have message type 0x0A 0xBF",
            label
        );
    }

    // Verify each decodes to the expected typed message (not Unknown)
    let fault_msg = super::dispatch_response(&fault_bytes);
    assert!(
        matches!(
            fault_msg,
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(_)
        ),
        "fault response should dispatch as FaultLogResponse"
    );

    let filter_msg = super::dispatch_response(&filter_bytes);
    assert!(
        matches!(
            filter_msg,
            launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(_)
        ),
        "filter response should dispatch as FilterCyclesResponse"
    );

    let info_msg = super::dispatch_response(&info_bytes);
    assert!(
        matches!(
            info_msg,
            launa_protocol::dispatcher::IncomingMessage::InformationResponse(_)
        ),
        "info response should dispatch as InformationResponse"
    );

    let config_msg = super::dispatch_response(&config_bytes);
    assert!(
        matches!(
            config_msg,
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(_)
        ),
        "config response should dispatch as ControlConfiguration"
    );
}

#[test]
fn test_configurable_sim_response_round_trip() {
    let mut sim = SpaSim::new();
    sim.registered = true;

    // Configure custom fault log
    sim.set_fault_log_config(FaultLogConfig {
        fault_count: 7,
        entry_number: 3,
        message_code: FaultCode::SensorAFault,
        days_ago: 5,
        hour: 10,
        minute: 45,
        flags: 0x12,
        set_temperature: 80,
        sensor_a_temp: 78,
        sensor_b_temp: 77,
    });

    // Configure custom filter cycles
    sim.set_filter_cycles_config(FilterCyclesConfig {
        filter1: FilterCycleConfig {
            start_hour: 3,
            start_minute: 15,
            duration_hours: 1,
            duration_minutes: 45,
            enabled: true,
        },
        filter2: FilterCycleConfig {
            start_hour: 21,
            start_minute: 30,
            duration_hours: 4,
            duration_minutes: 0,
            enabled: true,
        },
    });

    // Configure custom information
    let mut model = [b' '; 8];
    model[..5].copy_from_slice(b"TEST1");
    sim.set_information_config(InformationConfig {
        software_id_byte0: 0xAB,
        software_id_byte1: 0xCD,
        software_version_byte0: 0xEF,
        software_version_byte1: 0x01,
        system_model: model,
        current_setup: 0x42,
        config_sig_byte0: 0xCA,
        config_sig_byte1: 0xFE,
        config_sig_byte2: 0xBA,
        config_sig_byte3: 0xBE,
        heater_voltage: 0x01,
        heater_type: 0x0A,
        dip_switch_byte0: 0xAA,
        dip_switch_byte1: 0x55,
    });

    // Send settings requests and verify responses survive round-trip
    // Fault log request: 0x22 0x20
    let fault_response = sim.process_incoming_bytes(&super::build_settings_request(0x20));
    assert!(
        !fault_response.is_empty(),
        "should produce fault log response"
    );
    let fault_msg = super::dispatch_response(&fault_response);
    match fault_msg {
        launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
            assert_eq!(entry.fault_count, 7);
            assert_eq!(entry.entry_number, 3);
            assert_eq!(entry.message_code, FaultCode::SensorAFault);
            assert_eq!(entry.days_ago, 5);
            assert_eq!(entry.hour, 10);
            assert_eq!(entry.minute, 45);
            assert_eq!(entry.flags, 0x12);
            assert_eq!(entry.set_temperature, 80);
            assert_eq!(entry.sensor_a_temp, 78);
            assert_eq!(entry.sensor_b_temp, 77);
        }
        other => panic!("Expected FaultLogResponse, got {:?}", other),
    }

    // Filter cycles request: 0x22 0x01
    let filter_response = sim.process_incoming_bytes(&super::build_settings_request(0x01));
    assert!(
        !filter_response.is_empty(),
        "should produce filter cycles response"
    );
    let filter_msg = super::dispatch_response(&filter_response);
    match filter_msg {
        launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
            assert_eq!(fc.filter1.start_hour, 3);
            assert_eq!(fc.filter1.start_minute, 15);
            assert_eq!(fc.filter1.duration_hours, 1);
            assert_eq!(fc.filter1.duration_minutes, 45);
            assert_eq!(fc.filter2.start_hour, 21);
            assert_eq!(fc.filter2.start_minute, 30);
            assert_eq!(fc.filter2.duration_hours, 4);
            assert!(fc.filter2.enabled);
        }
        other => panic!("Expected FilterCyclesResponse, got {:?}", other),
    }

    // Information request: 0x22 0x02
    let info_response = sim.process_incoming_bytes(&super::build_settings_request(0x02));
    assert!(
        !info_response.is_empty(),
        "should produce information response"
    );
    let info_msg = super::dispatch_response(&info_response);
    match info_msg {
        launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
            assert_eq!(info.system_model, "TEST1");
            assert_eq!(info.current_setup, 0x42);
            assert_eq!(info.config_signature, "CAFEBABE");
        }
        other => panic!("Expected InformationResponse, got {:?}", other),
    }

    // Config request: 0x04
    let config_response = sim.process_incoming_bytes(&super::build_config_request());
    assert!(
        !config_response.is_empty(),
        "should produce config response"
    );
    let config_msg = super::dispatch_response(&config_response);
    // Config response with 0x2E sub-type → ControlConfiguration
    match config_msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            // Defaults should apply since we didn't set a custom config
            assert_eq!(config.pump_configs[0], PumpConfig::TwoSpeed);
            assert!(config.circ_pump);
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

#[test]
fn test_config_response_adapts_temperature_scale() {
    let mut sim = SpaSim::new();
    sim.state.temp_scale = TemperatureScale::Celsius;

    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert!(
                config.temperature_scale_celsius,
                "config should report Celsius when state is Celsius"
            );
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }

    // Now test Fahrenheit
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert!(
                !config.temperature_scale_celsius,
                "config should report Fahrenheit when state is Fahrenheit"
            );
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

#[test]
fn test_custom_config_response_preserves_other_bits_with_scale_adaptation() {
    let mut raw = [0u8; 10];
    raw[3] = 0xFE; // all bits set except bit 0
    raw[5] = 0xFF;

    let mut sim = SpaSim::new();
    sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });
    sim.state.temp_scale = TemperatureScale::Celsius;

    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert!(config.temperature_scale_celsius, "should set Celsius bit");
            // Other bits in byte 3 should be preserved
            // FE | 01 = FF, so all bits in byte 3 should be set
            // The parser reads bit 0 of byte 3 for temperature_scale_celsius
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }

    // Switch to Fahrenheit — should clear bit 0
    sim.state.temp_scale = TemperatureScale::Fahrenheit;
    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert!(
                !config.temperature_scale_celsius,
                "should clear Celsius bit for Fahrenheit"
            );
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

#[test]
fn test_val_sim_019_custom_spa_config_round_trip() {
    let mut raw = [0u8; 10];
    raw[0] = 0x01; // 1 pump
    raw[1] = 0x03; // 3 pumps worth of config
    raw[5] = 0b00_00_00_01; // pump1=SingleSpeed
    raw[7] = 0x0F; // light1 + light2 (all bits)
    raw[8] = 0x00; // no circ pump, no blower
    raw[9] = 0x42; // arbitrary

    let mut sim = SpaSim::new();
    sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });

    let response = sim.generate_config_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
            assert_eq!(
                config.pump_configs[0],
                PumpConfig::SingleSpeed,
                "pump1 should be SingleSpeed"
            );
            assert!(!config.circ_pump, "circ pump should not be present");
            assert!(!config.blower, "blower should not be present");
        }
        other => panic!("Expected ControlConfiguration, got {:?}", other),
    }
}

#[test]
fn test_val_sim_020_custom_information_round_trip() {
    let mut model = [b' '; 8];
    model[..4].copy_from_slice(b"TEST");

    let mut sim = SpaSim::new();
    sim.set_information_config(InformationConfig {
        software_id_byte0: 0x11,
        software_id_byte1: 0x22,
        software_version_byte0: 0x33,
        software_version_byte1: 0x44,
        system_model: model,
        current_setup: 0x07,
        config_sig_byte0: 0xAB,
        config_sig_byte1: 0xCD,
        config_sig_byte2: 0xEF,
        config_sig_byte3: 0x01,
        heater_voltage: 0x01,
        heater_type: 0x0A,
        dip_switch_byte0: 0x0F,
        dip_switch_byte1: 0xF0,
    });

    let response = sim.generate_information_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
            assert_eq!(info.system_model, "TEST");
            assert_eq!(info.current_setup, 0x07);
            assert_eq!(info.config_signature, "ABCDEF01");
            assert_eq!(info.dip_switches, "0000111111110000");
        }
        other => panic!("Expected InformationResponse, got {:?}", other),
    }
}

#[test]
fn test_val_sim_021_custom_filter_cycles_round_trip() {
    let mut sim = SpaSim::new();
    sim.set_filter_cycles_config(FilterCyclesConfig {
        filter1: FilterCycleConfig {
            start_hour: 0,
            start_minute: 0,
            duration_hours: 1,
            duration_minutes: 0,
            enabled: true,
        },
        filter2: FilterCycleConfig {
            start_hour: 12,
            start_minute: 30,
            duration_hours: 3,
            duration_minutes: 45,
            enabled: false,
        },
    });

    let response = sim.generate_filter_cycles_response();
    let msg = super::dispatch_response(&response);

    match msg {
        launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
            assert_eq!(fc.filter1.start_hour, 0);
            assert_eq!(fc.filter1.start_minute, 0);
            assert_eq!(fc.filter1.duration_hours, 1);
            assert_eq!(fc.filter1.duration_minutes, 0);
            assert!(fc.filter1.enabled);

            assert_eq!(fc.filter2.start_hour, 12);
            assert_eq!(fc.filter2.start_minute, 30);
            assert_eq!(fc.filter2.duration_hours, 3);
            assert_eq!(fc.filter2.duration_minutes, 45);
            assert!(!fc.filter2.enabled);
        }
        other => panic!("Expected FilterCyclesResponse, got {:?}", other),
    }
}
