//! Convert a `StatusUpdate` into a JSON string for the Home Assistant state topic.

extern crate alloc;

use alloc::string::String;
use alloc::format;
use launa_protocol::status::{StatusUpdate, HeatingMode, TemperatureScale, TempRange, PumpState};

/// Serialize a `StatusUpdate` into a JSON string suitable for publishing
/// to the Home Assistant state topic.
///
/// The JSON field names match the `value_template` patterns used in
/// the discovery configuration so that HA can extract each value.
///
/// This implementation builds JSON manually (no serde) so it works in no_std.
pub fn status_to_json(status: &StatusUpdate) -> String {
    let current_temp = match status.current_temp {
        Some(t) => format!("{}", t),
        None => String::from("null"),
    };

    let is_heating = if status.is_heating { "true" } else { "false" };
    let pump1_on = if matches!(status.pump1, PumpState::Low | PumpState::High) { "true" } else { "false" };
    let pump2_on = if matches!(status.pump2, PumpState::Low | PumpState::High) { "true" } else { "false" };
    let pump3_on = if matches!(status.pump3, PumpState::Low | PumpState::High) { "true" } else { "false" };

    let heating_mode = match status.heating_mode {
        HeatingMode::Ready => "ready",
        HeatingMode::Rest => "rest",
        HeatingMode::ReadyInRest => "ready_in_rest",
    };

    let temp_range = match status.temp_range {
        TempRange::High => "high",
        TempRange::Low => "low",
    };

    let temp_scale = match status.temperature_scale {
        TemperatureScale::Fahrenheit => "fahrenheit",
        TemperatureScale::Celsius => "celsius",
    };

    format!(
        "{{\"current_temp\":{},\"set_temp\":{},\"is_heating\":{},\"pump1_on\":{},\"pump2_on\":{},\"pump3_on\":{},\"light1\":{},\"blower\":{},\"circ_pump\":{},\"mister\":{},\"hold_mode\":{},\"heating_mode\":\"{}\",\"temp_range\":\"{}\",\"temp_scale\":\"{}\",\"hour\":{},\"minute\":{},\"last_fault\":null}}",
        current_temp,
        status.set_temp,
        is_heating,
        pump1_on,
        pump2_on,
        pump3_on,
        status.light1,
        status.blower,
        status.circ_pump,
        status.mister,
        status.is_hold,
        heating_mode,
        temp_range,
        temp_scale,
        status.hour,
        status.minute
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use launa_protocol::status::{
        StatusUpdate, HeatingMode, TemperatureScale, TempRange, PumpState, TimeFormat,
    };
    use serde_json;

    fn sample_status() -> StatusUpdate {
        StatusUpdate {
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
            pump1: PumpState::Low,
            pump2: PumpState::Off,
            pump3: PumpState::Off,
            circ_pump: false,
            blower: false,
            mister: false,
            light1: true,
            is_priming: false,
            is_hold: false,
        }
    }

    #[test]
    fn test_status_to_json_all_fields() {
        let status = sample_status();
        let json_str = status_to_json(&status);

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
        assert_eq!(parsed["light1"], true);
        assert_eq!(parsed["blower"], false);
        assert_eq!(parsed["circ_pump"], false);
        assert_eq!(parsed["mister"], false);
        assert_eq!(parsed["hold_mode"], false);
        assert_eq!(parsed["heating_mode"], "ready");
        assert_eq!(parsed["temp_range"], "high");
        assert_eq!(parsed["temp_scale"], "fahrenheit");
        assert_eq!(parsed["hour"], 14);
        assert_eq!(parsed["minute"], 30);
        assert!(parsed["last_fault"].is_null());
    }

    #[test]
    fn test_status_to_json_null_temp() {
        let mut status = sample_status();
        status.current_temp = None;
        let json_str = status_to_json(&status);
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
            let json_str = status_to_json(&status);
            let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            assert_eq!(parsed["heating_mode"], expected);
        }
    }

    #[test]
    fn test_status_to_json_pump_states() {
        let mut status = sample_status();
        status.pump1 = PumpState::High;
        status.pump2 = PumpState::Low;
        status.pump3 = PumpState::Off;
        let json_str = status_to_json(&status);
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
        let json_str = status_to_json(&status);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["temp_scale"], "celsius");
        assert_eq!(parsed["temp_range"], "low");
    }

    #[test]
    fn test_status_to_json_is_heating_false() {
        let mut status = sample_status();
        status.is_heating = false;
        let json_str = status_to_json(&status);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["is_heating"], false);
    }
}
