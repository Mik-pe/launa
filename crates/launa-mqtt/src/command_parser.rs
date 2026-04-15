//! Parse incoming MQTT command messages into `Command` variants.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use launa_protocol::command::{Command, TempError, ToggleItem, ABSOLUTE_MAX_TEMP_F};
use launa_protocol::status::{TempRange, TemperatureScale};

/// Recognized command subtopics. Anything not in this list is silently rejected.
const ALLOWED_SUBTOPICS: &[&str] = &[
    "pump1",
    "pump2",
    "pump3",
    "pump4",
    "pump5",
    "pump6",
    "pump1_timer",
    "pump2_timer",
    "pump3_timer",
    "pump4_timer",
    "pump5_timer",
    "pump6_timer",
    "light1",
    "light2",
    "blower",
    "heat_mode",
    "temp_range",
    "hold_mode",
    "set_temperature",
];

/// Result of parsing a command, including temperature validation status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseResult {
    /// A valid command was parsed.
    Valid(Command),
    /// The temperature was validated but out of the safe range.
    TemperatureOutOfRange {
        raw_value: u8,
        error: launa_protocol::command::TempError,
    },
    /// The subtopic is not in the allowlist.
    UnknownSubtopic(String),
    /// The payload could not be parsed.
    InvalidPayload(String),
    /// A pump timer command: start pump N for M minutes.
    TimerPump { minutes: u32, pump_index: u8 },
}

/// Parse an incoming MQTT message into a `Command` with allowlist enforcement.
///
/// `command_topic_base` is the base command topic, e.g. `"launa/test_spa/command"`.
/// `topic` is the full topic the message arrived on.
/// `payload` is the raw MQTT payload bytes.
///
/// Returns a `ParseResult` indicating whether the command is valid, rejected,
/// or had a temperature validation error.
pub fn parse_command(command_topic_base: &str, topic: &str, payload: &[u8]) -> ParseResult {
    // topic must be exactly command_topic_base + "/" + subcommand
    if !topic.starts_with(command_topic_base) {
        return ParseResult::UnknownSubtopic(topic.to_string());
    }

    let suffix = &topic[command_topic_base.len()..];

    // Must start with '/'
    if !suffix.starts_with('/') {
        return ParseResult::UnknownSubtopic(topic.to_string());
    }

    let subtopic = &suffix[1..];

    // Check allowlist first
    if !ALLOWED_SUBTOPICS.contains(&subtopic) {
        return ParseResult::UnknownSubtopic(subtopic.to_string());
    }

    let payload_str = match core::str::from_utf8(payload) {
        Ok(s) => s,
        Err(_) => return ParseResult::InvalidPayload("non-utf8 payload".to_string()),
    };

    match subtopic {
        s if s.starts_with("pump") && s.ends_with("_timer") => {
            // "pump<N>_timer" → parse N and delegate
            let num_str = &s[4..s.len() - 6]; // strip "pump" prefix and "_timer" suffix
            let idx: u8 = match num_str.parse() {
                Ok(n) if n >= 1 && n <= 6 => n,
                _ => return ParseResult::UnknownSubtopic(subtopic.to_string()),
            };
            parse_pump_timer(payload_str, idx)
        }
        s if s.starts_with("pump") => {
            // "pump<N>" → parse N and map to ToggleItem via from_pump_index
            let num_str = &s[4..];
            let idx: usize = match num_str.parse() {
                Ok(n) if n >= 1 && n <= 6 => n,
                _ => return ParseResult::UnknownSubtopic(subtopic.to_string()),
            };
            if let Some(item) = ToggleItem::from_pump_index(idx - 1) {
                parse_toggle(payload_str, item)
            } else {
                ParseResult::UnknownSubtopic(subtopic.to_string())
            }
        }
        s if s.starts_with("light") => {
            // "light<N>" → parse N and map to ToggleItem via from_light_index
            let num_str = &s[5..];
            let idx: usize = match num_str.parse() {
                Ok(n) if n >= 1 && n <= 2 => n,
                _ => return ParseResult::UnknownSubtopic(subtopic.to_string()),
            };
            if let Some(item) = ToggleItem::from_light_index(idx - 1) {
                parse_toggle(payload_str, item)
            } else {
                ParseResult::UnknownSubtopic(subtopic.to_string())
            }
        }
        "blower" => parse_toggle(payload_str, ToggleItem::Blower),
        "heat_mode" => parse_toggle(payload_str, ToggleItem::HeatingMode),
        "temp_range" => parse_toggle(payload_str, ToggleItem::TemperatureRange),
        "hold_mode" => parse_toggle(payload_str, ToggleItem::HoldMode),
        "set_temperature" => parse_set_temperature(payload_str),
        _ => ParseResult::UnknownSubtopic(subtopic.to_string()),
    }
}

/// Convenience function that returns `Some(Command)` only for valid commands,
/// silently filtering out unknown subtopics and invalid payloads.
/// This preserves backward compatibility with existing callers.
pub fn parse_command_ok(command_topic_base: &str, topic: &str, payload: &[u8]) -> Option<Command> {
    match parse_command(command_topic_base, topic, payload) {
        ParseResult::Valid(cmd) => Some(cmd),
        _ => None,
    }
}

/// Parse a set-temperature command with hard upper-limit validation.
///
/// Without scale/range context we cannot do full range validation, but we
/// enforce the absolute maximum (108°F wire value) as a safety backstop to
/// prevent accidental `SetTemperature(255)` being sent to the spa.
/// Zero is accepted as a valid wire value ("no temp set").
fn parse_set_temperature(payload: &str) -> ParseResult {
    let temp: u8 = match payload.parse() {
        Ok(t) => t,
        Err(_) => return ParseResult::InvalidPayload(format!("not a number: {:?}", payload)),
    };

    if temp > ABSOLUTE_MAX_TEMP_F {
        return ParseResult::TemperatureOutOfRange {
            raw_value: temp,
            error: TempError::AboveAbsoluteLimit,
        };
    }

    ParseResult::Valid(Command::SetTemperature(temp))
}

/// Parse a set-temperature command with full temperature range validation.
pub fn parse_set_temperature_validated(
    payload: &str,
    scale: TemperatureScale,
    range: TempRange,
) -> ParseResult {
    let temp: u8 = match payload.parse() {
        Ok(t) => t,
        Err(_) => return ParseResult::InvalidPayload(format!("not a number: {:?}", payload)),
    };

    match launa_protocol::command::validate_set_temperature(temp, scale, range) {
        Ok(_) => ParseResult::Valid(Command::SetTemperature(temp)),
        Err(e) => ParseResult::TemperatureOutOfRange {
            raw_value: temp,
            error: e,
        },
    }
}

fn parse_toggle(payload: &str, item: ToggleItem) -> ParseResult {
    match payload {
        "true" | "false" => ParseResult::Valid(Command::ToggleItem(item)),
        _ => ParseResult::InvalidPayload(format!("expected 'true' or 'false', got: {:?}", payload)),
    }
}

fn parse_pump_timer(payload: &str, pump_index: u8) -> ParseResult {
    match payload.parse::<u32>() {
        Ok(minutes) if minutes > 0 && minutes <= 120 => ParseResult::TimerPump {
            minutes,
            pump_index,
        },
        Ok(minutes) => {
            ParseResult::InvalidPayload(format!("timer minutes must be 1-120, got: {}", minutes))
        }
        Err(_) => ParseResult::InvalidPayload(format!("not a number: {:?}", payload)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD_BASE: &str = "launa/test_spa_001/command";

    #[test]
    fn test_parse_pump1_on() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::Pump1))
        );
    }

    #[test]
    fn test_parse_pump1_off() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", b"false");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::Pump1))
        );
    }

    #[test]
    fn test_parse_pump2() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump2", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::Pump2))
        );
    }

    #[test]
    fn test_parse_pump3() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump3", b"false");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::Pump3))
        );
    }

    #[test]
    fn test_parse_light1() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/light1", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::Light1))
        );
    }

    #[test]
    fn test_parse_blower() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/blower", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::Blower))
        );
    }

    #[test]
    fn test_parse_heat_mode() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/heat_mode", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::HeatingMode))
        );
    }

    #[test]
    fn test_parse_temp_range() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/temp_range", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::TemperatureRange))
        );
    }

    #[test]
    fn test_parse_hold_mode() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/hold_mode", b"true");
        assert_eq!(
            result,
            ParseResult::Valid(Command::ToggleItem(ToggleItem::HoldMode))
        );
    }

    #[test]
    fn test_parse_set_temperature() {
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/set_temperature",
            b"104",
        );
        assert_eq!(result, ParseResult::Valid(Command::SetTemperature(104)));
    }

    #[test]
    fn test_parse_set_temperature_low() {
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/set_temperature",
            b"80",
        );
        assert_eq!(result, ParseResult::Valid(Command::SetTemperature(80)));
    }

    #[test]
    fn test_parse_unknown_subtopic() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/unknown", b"true");
        assert!(matches!(result, ParseResult::UnknownSubtopic(_)));
    }

    #[test]
    fn test_parse_wrong_base() {
        let result = parse_command(
            "launa/other_spa/command",
            "launa/test_spa_001/command/pump1",
            b"true",
        );
        assert!(matches!(result, ParseResult::UnknownSubtopic(_)));
    }

    #[test]
    fn test_parse_invalid_toggle_payload() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", b"on");
        assert!(matches!(result, ParseResult::InvalidPayload(_)));
    }

    #[test]
    fn test_parse_invalid_temperature() {
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/set_temperature",
            b"abc",
        );
        assert!(matches!(result, ParseResult::InvalidPayload(_)));
    }

    #[test]
    fn test_parse_state_topic_not_command() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/state", b"true");
        assert!(matches!(result, ParseResult::UnknownSubtopic(_)));
    }

    #[test]
    fn test_parse_exact_base_topic_no_subtopic() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command", b"true");
        assert!(matches!(result, ParseResult::UnknownSubtopic(_)));
    }

    // --- New tests for allowlist and validation ---

    #[test]
    fn test_parse_non_utf8_payload_rejected() {
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", &[0xFF, 0xFE]);
        assert!(matches!(result, ParseResult::InvalidPayload(_)));
    }

    #[test]
    fn test_allowlist_rejects_garbage_subtopic() {
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/random_garbage",
            b"true",
        );
        assert!(matches!(result, ParseResult::UnknownSubtopic(_)));
    }

    #[test]
    fn test_parse_set_temperature_validated_ok() {
        let result =
            parse_set_temperature_validated("100", TemperatureScale::Fahrenheit, TempRange::High);
        assert_eq!(result, ParseResult::Valid(Command::SetTemperature(100)));
    }

    #[test]
    fn test_parse_set_temperature_validated_out_of_range() {
        let result =
            parse_set_temperature_validated("50", TemperatureScale::Fahrenheit, TempRange::High);
        assert!(matches!(result, ParseResult::TemperatureOutOfRange { .. }));
    }

    #[test]
    fn test_parse_set_temperature_validated_above_absolute() {
        let result =
            parse_set_temperature_validated("200", TemperatureScale::Fahrenheit, TempRange::High);
        assert!(matches!(
            result,
            ParseResult::TemperatureOutOfRange {
                raw_value: 200,
                error: launa_protocol::command::TempError::AboveAbsoluteLimit
            }
        ));
    }

    #[test]
    fn test_backward_compat_parse_command_ok() {
        // parse_command_ok returns None for unknown/invalid, Some for valid
        assert_eq!(
            parse_command_ok(CMD_BASE, "launa/test_spa_001/command/pump1", b"true"),
            Some(Command::ToggleItem(ToggleItem::Pump1)),
        );
        assert_eq!(
            parse_command_ok(CMD_BASE, "launa/test_spa_001/command/unknown", b"true"),
            None,
        );
        assert_eq!(
            parse_command_ok(CMD_BASE, "launa/test_spa_001/command/pump1", b"garbage"),
            None,
        );
    }

    // --- Absolute max temperature gating tests ---

    #[test]
    fn test_parse_set_temperature_above_absolute_max() {
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/set_temperature",
            b"200",
        );
        assert_eq!(
            result,
            ParseResult::TemperatureOutOfRange {
                raw_value: 200,
                error: TempError::AboveAbsoluteLimit,
            }
        );
    }

    #[test]
    fn test_parse_set_temperature_at_max() {
        // 108 = ABSOLUTE_MAX_TEMP_F, should be accepted
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/set_temperature",
            b"108",
        );
        assert_eq!(result, ParseResult::Valid(Command::SetTemperature(108)));
    }

    #[test]
    fn test_parse_set_temperature_just_above_max() {
        // 109 > ABSOLUTE_MAX_TEMP_F, should be rejected
        let result = parse_command(
            CMD_BASE,
            "launa/test_spa_001/command/set_temperature",
            b"109",
        );
        assert_eq!(
            result,
            ParseResult::TemperatureOutOfRange {
                raw_value: 109,
                error: TempError::AboveAbsoluteLimit,
            }
        );
    }

    #[test]
    fn test_parse_set_temperature_zero() {
        // 0 is valid wire value ("no temp set")
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/set_temperature", b"0");
        assert_eq!(result, ParseResult::Valid(Command::SetTemperature(0)));
    }
}
