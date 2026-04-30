//! Convert a `StatusUpdate` into a JSON string for the Home Assistant state topic.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use launa_protocol::status::{HeatingMode, PumpState, StatusUpdate, TempRange, TemperatureScale};

use crate::escape::escape_json_string;

/// Serialize a `StatusUpdate` into a JSON string suitable for publishing
/// to the Home Assistant state topic.
///
/// The JSON field names match the `value_template` patterns used in
/// the discovery configuration so that HA can extract each value.
///
/// This implementation builds JSON manually (no serde) so it works in no_std.
pub fn status_to_json(
    status: &StatusUpdate,
    last_fault: Option<&str>,
    firmware_version: Option<&str>,
    sniff_mode: bool,
    wifi_rssi: Option<i32>,
    registration_state: &str,
) -> String {
    let current_temp = match status.current_temp {
        Some(t) => format!("{}", t.raw_value()),
        None => String::from("null"),
    };

    let is_heating = if status.is_heating { "true" } else { "false" };

    // Generate pump fields in a loop
    let mut pump_parts = Vec::new();
    for (i, pump) in status.pumps.iter().enumerate() {
        let val = if matches!(pump, PumpState::Low | PumpState::High) {
            "true"
        } else {
            "false"
        };
        pump_parts.push(alloc::format!("\"pump{}_on\":{}", i + 1, val));
    }
    let pump_fields = pump_parts.join(",");

    // Generate light fields in a loop
    let mut light_parts = Vec::new();
    for (i, on) in status.lights.iter().enumerate() {
        light_parts.push(alloc::format!("\"light{}\":{}", i + 1, on));
    }
    let light_fields = light_parts.join(",");

    let heating_mode = match status.heating_mode {
        HeatingMode::Ready => "ready",
        HeatingMode::Rest => "rest",
        HeatingMode::ReadyInRest => "ready_in_rest",
        _ => "unknown",
    };

    let temp_range = match status.temp_range {
        TempRange::High => "high",
        TempRange::Low => "low",
        _ => "unknown",
    };

    let temp_scale = match status.temperature_scale {
        TemperatureScale::Fahrenheit => "fahrenheit",
        TemperatureScale::Celsius => "celsius",
        _ => "unknown",
    };

    let time_format = match status.time_format {
        launa_protocol::status::TimeFormat::Hour12 => "12h",
        launa_protocol::status::TimeFormat::Hour24 => "24h",
    };

    let firmware_ver = match firmware_version {
        Some(v) => alloc::format!("\"{}\"", escape_json_string(v)),
        None => alloc::string::String::from("null"),
    };

    // Combine pump and light fields
    let device_fields = [pump_fields.as_str(), light_fields.as_str()].join(",");

    format!(
        "{{\"current_temp\":{},\"set_temp\":{},\"is_heating\":{},{},\"blower\":{},\"circ_pump\":{},\"mister\":{},\"hold_mode\":{},\"heating_mode\":\"{}\",\"temp_range\":\"{}\",\"temp_scale\":\"{}\",\"time_format\":\"{}\",\"hour\":{},\"minute\":{},\"notification_type\":{},\"panel_locked\":{},\"settings_lock\":{},\"m8_cycle_time\":{},\"last_fault\":{},\"firmware_version\":{},\"sniff_mode\":{},\"wifi_rssi\":{},\"registration_state\":\"{}\"}}",
        current_temp,
        status.set_temp,
        is_heating,
        device_fields,
        status.blower,
        status.circ_pump,
        status.mister,
        status.is_hold,
        heating_mode,
        temp_range,
        temp_scale,
        time_format,
        status.hour,
        status.minute,
        status.notification_type,
        status.panel_locked,
        status.settings_lock,
        status.m8_cycle_time,
        match last_fault {
            Some(f) => alloc::format!("\"{}\"", escape_json_string(f)),
            None => alloc::string::String::from("null"),
        },
        firmware_ver,
        sniff_mode,
        wifi_rssi.map_or(String::from("null"), |r| format!("{}", r)),
        registration_state,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use launa_protocol::status::{
        HeatingMode, PumpState, StatusUpdate, TempRange, TemperatureScale, TimeFormat,
    };
    use launa_protocol::Temperature;
    use serde_json;

    fn sample_status() -> StatusUpdate {
        StatusUpdate {
            current_temp: Some(Temperature::fahrenheit(100.0)),
            set_temp: Temperature::fahrenheit(104.0),
            hour: 14,
            minute: 30,
            heating_mode: HeatingMode::Ready,
            temperature_scale: TemperatureScale::Fahrenheit,
            time_format: TimeFormat::Hour24,
            filter_mode: 0,
            is_heating: true,
            temp_range: TempRange::High,
            pumps: [
                PumpState::Low,
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
            ],
            circ_pump: false,
            blower: false,
            mister: false,
            lights: [true, false, false, false],
            is_priming: false,
            is_hold: false,
            notification_type: 0,
            panel_locked: false,
            settings_lock: false,
            m8_cycle_time: 0,
            sensor_a_temp: Some(Temperature::fahrenheit(98.0)),
            sensor_b_temp: None,
            hold_timer_minutes: None,
        }
    }

    #[test]
    fn test_status_to_json_all_fields() {
        let status = sample_status();
        let json_str = status_to_json(&status, None, None, false, None, "registered");

        // Verify it is valid JSON by parsing it back
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("output should be valid JSON");

        // Verify all fields
        assert_eq!(parsed["current_temp"], 100.0);
        assert_eq!(parsed["set_temp"], 104.0);
        assert_eq!(parsed["is_heating"], true);
        assert_eq!(parsed["pump1_on"], true);
        assert_eq!(parsed["pump2_on"], false);
        assert_eq!(parsed["pump3_on"], false);
        assert_eq!(parsed["pump4_on"], false);
        assert_eq!(parsed["pump5_on"], false);
        assert_eq!(parsed["pump6_on"], false);
        assert_eq!(parsed["light1"], true);
        assert_eq!(parsed["light2"], false);
        assert_eq!(parsed["light3"], false);
        assert_eq!(parsed["light4"], false);
        assert_eq!(parsed["blower"], false);
        assert_eq!(parsed["circ_pump"], false);
        assert_eq!(parsed["mister"], false);
        assert_eq!(parsed["hold_mode"], false);
        assert_eq!(parsed["heating_mode"], "ready");
        assert_eq!(parsed["temp_range"], "high");
        assert_eq!(parsed["temp_scale"], "fahrenheit");
        assert_eq!(parsed["time_format"], "24h");
        assert_eq!(parsed["hour"], 14);
        assert_eq!(parsed["minute"], 30);
        assert!(parsed["last_fault"].is_null());
        assert!(parsed["firmware_version"].is_null());
        // New fields
        assert_eq!(parsed["notification_type"], 0);
        assert_eq!(parsed["panel_locked"], false);
        assert_eq!(parsed["settings_lock"], false);
        assert_eq!(parsed["m8_cycle_time"], 0);
        assert_eq!(parsed["registration_state"], "registered");
    }

    #[test]
    fn test_status_to_json_new_fields_set() {
        let mut status = sample_status();
        status.notification_type = 4;
        status.panel_locked = true;
        status.settings_lock = true;
        status.m8_cycle_time = 30;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["notification_type"], 4);
        assert_eq!(parsed["panel_locked"], true);
        assert_eq!(parsed["settings_lock"], true);
        assert_eq!(parsed["m8_cycle_time"], 30);
    }

    #[test]
    fn test_status_to_json_registration_state_unregistered() {
        let status = sample_status();
        let json_str = status_to_json(&status, None, None, false, None, "waiting_for_assignment");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["registration_state"], "waiting_for_assignment");
    }

    #[test]
    fn test_status_to_json_null_temp() {
        let mut status = sample_status();
        status.current_temp = None;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed["current_temp"].is_null());
    }

    #[test]
    fn test_status_to_json_heating_modes() {
        for (mode, expected) in [
            (HeatingMode::Ready, "ready"),
            (HeatingMode::Rest, "rest"),
            (HeatingMode::ReadyInRest, "ready_in_rest"),
        ] {
            let mut status = sample_status();
            status.heating_mode = mode;
            let json_str = status_to_json(&status, None, None, false, None, "registered");
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            assert_eq!(parsed["heating_mode"], expected);
        }
    }

    #[test]
    fn test_status_to_json_pump_states() {
        let mut status = sample_status();
        status.pumps[0] = PumpState::High;
        status.pumps[1] = PumpState::Low;
        status.pumps[2] = PumpState::Off;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["pump1_on"], true);
        assert_eq!(parsed["pump2_on"], true);
        assert_eq!(parsed["pump3_on"], false);
    }

    #[test]
    fn test_status_to_json_celsius_scale() {
        let mut status = sample_status();
        status.temperature_scale = TemperatureScale::Celsius;
        status.temp_range = TempRange::Low;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["temp_scale"], "celsius");
        assert_eq!(parsed["temp_range"], "low");
    }

    #[test]
    fn test_status_to_json_is_heating_false() {
        let mut status = sample_status();
        status.is_heating = false;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["is_heating"], false);
    }

    #[test]
    fn test_status_to_json_with_fault() {
        let status = sample_status();
        let json_str = status_to_json(
            &status,
            Some("HeaterDry: code 27"),
            None,
            false,
            None,
            "registered",
        );
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "HeaterDry: code 27");
    }

    #[test]
    fn test_status_to_json_with_firmware_version() {
        let status = sample_status();
        let json_str = status_to_json(&status, None, Some("1.2.3"), false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["firmware_version"], "1.2.3");
    }

    #[test]
    fn test_escape_json_string_backslash() {
        // Backslash in a fault string must be escaped
        let status = sample_status();
        let json_str = status_to_json(
            &status,
            Some("Fault:\\path\\to\\issue"),
            None,
            false,
            None,
            "registered",
        );
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "Fault:\\path\\to\\issue");
    }

    #[test]
    fn test_escape_json_string_newline_tab() {
        // Newline and tab must be escaped
        let status = sample_status();
        let json_str = status_to_json(
            &status,
            Some("Line1\nLine2\tTabbed"),
            None,
            false,
            None,
            "registered",
        );
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "Line1\nLine2\tTabbed");
    }

    #[test]
    fn test_escape_json_string_control_chars() {
        // Control characters (0x00-0x1F) must be escaped as \uXXXX
        let status = sample_status();
        let fault = alloc::format!("Bad{}char", '\x07'); // BEL character
        let json_str = status_to_json(&status, Some(&fault), None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "Bad\u{0007}char");
    }

    #[test]
    fn test_escape_json_string_carriage_return() {
        let status = sample_status();
        let json_str = status_to_json(
            &status,
            Some("Line1\rLine2"),
            None,
            false,
            None,
            "registered",
        );
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "Line1\rLine2");
    }

    #[test]
    fn test_escape_json_string_all_special() {
        // A string containing all escapable characters at once
        let status = sample_status();
        let fault = alloc::format!("a\\b\"c\nd\re\tf{}g", '\x01');
        let json_str = status_to_json(&status, Some(&fault), None, false, None, "registered");
        // The main assertion: the JSON must parse successfully
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            parsed["last_fault"],
            alloc::format!("a\\b\"c\nd\re\tf\u{0001}g")
        );
    }

    #[test]
    fn test_escape_json_string_firmware_with_special_chars() {
        // Firmware version field also gets proper escaping
        let status = sample_status();
        let json_str = status_to_json(&status, None, Some("v1.0\nbeta"), false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["firmware_version"], "v1.0\nbeta");
    }

    #[test]
    fn test_escape_json_string_quote_in_fault() {
        // Double-quote in a fault string must be escaped (existing behaviour)
        let status = sample_status();
        let json_str = status_to_json(
            &status,
            Some("Heater \"dry\" fire"),
            None,
            false,
            None,
            "registered",
        );
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "Heater \"dry\" fire");
    }

    #[test]
    fn test_escape_json_string_null_char() {
        // Null byte must be escaped as \u0000
        let status = sample_status();
        let fault = alloc::format!("before{}after", '\x00');
        let json_str = status_to_json(&status, Some(&fault), None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["last_fault"], "before\u{0000}after");
    }

    #[test]
    fn test_status_to_json_all_pumps_on() {
        let mut status = sample_status();
        status.pumps = [
            PumpState::Low,
            PumpState::Low,
            PumpState::High,
            PumpState::High,
            PumpState::Low,
            PumpState::High,
        ];
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["pump1_on"], true);
        assert_eq!(parsed["pump2_on"], true);
        assert_eq!(parsed["pump3_on"], true);
        assert_eq!(parsed["pump4_on"], true);
        assert_eq!(parsed["pump5_on"], true);
        assert_eq!(parsed["pump6_on"], true);
    }

    #[test]
    fn test_status_to_json_pump5_pump6_on() {
        let mut status = sample_status();
        status.pumps[0] = PumpState::Off; // Turn off pump1 (was Low in sample)
        status.pumps[4] = PumpState::High;
        status.pumps[5] = PumpState::Low;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["pump5_on"], true);
        assert_eq!(parsed["pump6_on"], true);
        // First 4 pumps should be off
        assert_eq!(parsed["pump1_on"], false);
        assert_eq!(parsed["pump2_on"], false);
        assert_eq!(parsed["pump3_on"], false);
        assert_eq!(parsed["pump4_on"], false);
    }

    #[test]
    fn test_status_to_json_hold_mode_active() {
        let mut status = sample_status();
        status.is_hold = true;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["hold_mode"], true);
    }

    #[test]
    fn test_status_to_json_all_pumps_off_with_heating() {
        let mut status = sample_status();
        status.pumps = [PumpState::Off; 6];
        status.is_heating = true; // Heating flag can still be set
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["pump1_on"], false);
        assert_eq!(parsed["is_heating"], true);
    }

    #[test]
    fn test_status_to_json_mister_on() {
        let mut status = sample_status();
        status.mister = true;
        let json_str = status_to_json(&status, None, None, false, None, "registered");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["mister"], true);
    }
}
