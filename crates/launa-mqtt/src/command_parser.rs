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
    "light3",
    "light4",
    "blower",
    "mister",
    "circulation_pump",
    "aux1",
    "aux2",
    "heat_mode",
    "temp_range",
    "hold_mode",
    "soak_mode",
    "normal_operation",
    "clear_notification",
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
                Ok(n) if n >= 1 && n <= 4 => n,
                _ => return ParseResult::UnknownSubtopic(subtopic.to_string()),
            };
            if let Some(item) = ToggleItem::from_light_index(idx - 1) {
                parse_toggle(payload_str, item)
            } else {
                ParseResult::UnknownSubtopic(subtopic.to_string())
            }
        }
        "blower" => parse_toggle(payload_str, ToggleItem::Blower),
        "mister" => parse_toggle(payload_str, ToggleItem::Mister),
        "circulation_pump" => parse_toggle(payload_str, ToggleItem::CirculationPump),
        "aux1" => parse_toggle(payload_str, ToggleItem::Aux1),
        "aux2" => parse_toggle(payload_str, ToggleItem::Aux2),
        "heat_mode" => parse_toggle(payload_str, ToggleItem::HeatingMode),
        "temp_range" => parse_toggle(payload_str, ToggleItem::TemperatureRange),
        "hold_mode" => parse_toggle(payload_str, ToggleItem::HoldMode),
        "soak_mode" => parse_toggle(payload_str, ToggleItem::SoakMode),
        "normal_operation" => parse_toggle(payload_str, ToggleItem::NormalOperation),
        "clear_notification" => parse_toggle(payload_str, ToggleItem::ClearNotification),
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
/// Zero is rejected as an invalid temperature (no spa supports 0°F / 0°C).
fn parse_set_temperature(payload: &str) -> ParseResult {
    let temp: u8 = match payload.parse() {
        Ok(t) => t,
        Err(_) => return ParseResult::InvalidPayload(format!("not a number: {:?}", payload)),
    };

    if temp == 0 {
        return ParseResult::TemperatureOutOfRange {
            raw_value: 0,
            error: TempError::BelowMin,
        };
    }

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

    /// Each toggle subtopic + payload should produce the corresponding ToggleItem.
    /// Both "true" and "false" payloads are accepted for toggle commands.
    #[test]
    fn test_toggle_commands() {
        let cases: &[(&str, &ToggleItem)] = &[
            ("pump1", &ToggleItem::Pump1),
            ("pump2", &ToggleItem::Pump2),
            ("pump3", &ToggleItem::Pump3),
            ("pump4", &ToggleItem::Pump4),
            ("pump5", &ToggleItem::Pump5),
            ("pump6", &ToggleItem::Pump6),
            ("light1", &ToggleItem::Light1),
            ("light2", &ToggleItem::Light2),
            ("light3", &ToggleItem::Light3),
            ("light4", &ToggleItem::Light4),
            ("blower", &ToggleItem::Blower),
            ("mister", &ToggleItem::Mister),
            ("circulation_pump", &ToggleItem::CirculationPump),
            ("aux1", &ToggleItem::Aux1),
            ("aux2", &ToggleItem::Aux2),
            ("heat_mode", &ToggleItem::HeatingMode),
            ("temp_range", &ToggleItem::TemperatureRange),
            ("hold_mode", &ToggleItem::HoldMode),
            ("soak_mode", &ToggleItem::SoakMode),
            ("normal_operation", &ToggleItem::NormalOperation),
            ("clear_notification", &ToggleItem::ClearNotification),
        ];

        for (i, (subtopic, expected_item)) in cases.iter().enumerate() {
            let topic = format!("{}/{}", CMD_BASE, subtopic);

            let result = parse_command(CMD_BASE, &topic, b"true");
            assert_eq!(
                result,
                ParseResult::Valid(Command::ToggleItem(**expected_item)),
                "case {i}: true payload for subtopic '{subtopic}'"
            );

            let result = parse_command(CMD_BASE, &topic, b"false");
            assert_eq!(
                result,
                ParseResult::Valid(Command::ToggleItem(**expected_item)),
                "case {i}: false payload for subtopic '{subtopic}'"
            );
        }
    }

    /// Pump timer commands: valid minute ranges and boundary values.
    #[test]
    fn test_pump_timer_commands() {
        let valid_cases: &[(&str, u32, u8)] = &[
            ("pump1_timer", 15, 1),
            ("pump6_timer", 20, 6),
            ("pump2_timer", 120, 2), // upper boundary
            ("pump1_timer", 1, 1),   // lower boundary
        ];

        for (i, (subtopic, minutes, pump_index)) in valid_cases.iter().enumerate() {
            let topic = format!("{}/{}", CMD_BASE, subtopic);
            let payload = format!("{}", minutes);
            let result = parse_command(CMD_BASE, &topic, payload.as_bytes());
            assert_eq!(
                result,
                ParseResult::TimerPump {
                    minutes: *minutes,
                    pump_index: *pump_index,
                },
                "case {i}: valid timer for {subtopic}={minutes}"
            );
        }

        // Invalid minute values
        let invalid_cases: &[(&str, &[u8])] = &[
            ("pump1_timer", b"0"),   // zero minutes
            ("pump3_timer", b"121"), // over max
            ("pump1_timer", b"abc"), // non-numeric
        ];

        for (i, (subtopic, payload)) in invalid_cases.iter().enumerate() {
            let topic = format!("{}/{}", CMD_BASE, subtopic);
            let result = parse_command(CMD_BASE, &topic, *payload);
            assert!(
                matches!(result, ParseResult::InvalidPayload(_)),
                "case {i}: invalid timer payload for {subtopic}: expected InvalidPayload, got {:?}",
                result
            );
        }
    }

    /// Temperature commands: valid values, boundary conditions, and error cases.
    #[test]
    fn test_set_temperature() {
        let valid_cases: &[(u8, u8)] = &[
            (104, 104),
            (80, 80),
            (108, 108), // ABSOLUTE_MAX_TEMP_F
            (1, 1),     // minimum non-zero
            (50, 50),   // mid-range
        ];

        for (i, (input, expected)) in valid_cases.iter().enumerate() {
            let topic = format!("{}/set_temperature", CMD_BASE);
            let payload = format!("{}", input);
            let result = parse_command(CMD_BASE, &topic, payload.as_bytes());
            assert_eq!(
                result,
                ParseResult::Valid(Command::SetTemperature(*expected)),
                "case {i}: set_temperature={input}"
            );
        }

        let out_of_range_cases: &[(u8, TempError)] = &[
            (0, TempError::BelowMin),
            (109, TempError::AboveAbsoluteLimit), // just above max
            (200, TempError::AboveAbsoluteLimit), // well above max
            (255, TempError::AboveAbsoluteLimit), // u8 max
        ];

        for (i, (input, expected_error)) in out_of_range_cases.iter().enumerate() {
            let topic = format!("{}/set_temperature", CMD_BASE);
            let payload = format!("{}", input);
            let result = parse_command(CMD_BASE, &topic, payload.as_bytes());
            assert_eq!(
                result,
                ParseResult::TemperatureOutOfRange {
                    raw_value: *input,
                    error: *expected_error,
                },
                "case {i}: out-of-range set_temperature={input}"
            );
        }

        // Non-numeric payload
        let topic = format!("{}/set_temperature", CMD_BASE);
        let result = parse_command(CMD_BASE, &topic, b"abc");
        assert!(
            matches!(result, ParseResult::InvalidPayload(_)),
            "non-numeric temperature: expected InvalidPayload, got {:?}",
            result
        );
    }

    /// Temperature validation with scale and range context via parse_set_temperature_validated.
    #[test]
    fn test_set_temperature_validated() {
        let valid_cases: &[(&str, TemperatureScale, TempRange, u8)] = &[
            ("100", TemperatureScale::Fahrenheit, TempRange::High, 100),
            ("35", TemperatureScale::Celsius, TempRange::High, 35),
            ("20", TemperatureScale::Celsius, TempRange::Low, 20),
        ];

        for (i, (payload, scale, range, expected)) in valid_cases.iter().enumerate() {
            let result = parse_set_temperature_validated(payload, *scale, *range);
            assert_eq!(
                result,
                ParseResult::Valid(Command::SetTemperature(*expected)),
                "case {i}: validated temp={payload} {scale:?}/{range:?}"
            );
        }

        let error_cases: &[(&str, TemperatureScale, TempRange)] = &[
            ("50", TemperatureScale::Fahrenheit, TempRange::High), // below range
            ("200", TemperatureScale::Fahrenheit, TempRange::High), // above absolute
            ("0", TemperatureScale::Celsius, TempRange::High),     // zero rejected
        ];

        for (i, (payload, scale, range)) in error_cases.iter().enumerate() {
            let result = parse_set_temperature_validated(payload, *scale, *range);
            assert!(
                matches!(result, ParseResult::TemperatureOutOfRange { .. }),
                "case {i}: validated error temp={payload} {scale:?}/{range:?}: expected TemperatureOutOfRange, got {:?}",
                result
            );
        }

        // Verify exact error variant for above-absolute case
        let result =
            parse_set_temperature_validated("200", TemperatureScale::Fahrenheit, TempRange::High);
        assert!(matches!(
            result,
            ParseResult::TemperatureOutOfRange {
                raw_value: 200,
                error: TempError::AboveAbsoluteLimit
            }
        ));

        // Verify exact error variant for celsius zero
        let result =
            parse_set_temperature_validated("0", TemperatureScale::Celsius, TempRange::High);
        assert!(matches!(
            result,
            ParseResult::TemperatureOutOfRange {
                raw_value: 0,
                error: TempError::BelowMin
            }
        ));
    }

    /// Unknown subtopics and topic-mismatch errors.
    #[test]
    fn test_unknown_subtopic_errors() {
        let cases: &[(&str, &str, &[u8])] = &[
            // Unknown subtopic
            (CMD_BASE, "launa/test_spa_001/command/unknown", b"true"),
            // Garbage subtopic
            (
                CMD_BASE,
                "launa/test_spa_001/command/random_garbage",
                b"true",
            ),
            // Wrong base topic
            (
                "launa/other_spa/command",
                "launa/test_spa_001/command/pump1",
                b"true",
            ),
            // State topic (not command)
            (CMD_BASE, "launa/test_spa_001/state", b"true"),
            // Exact base topic with no subtopic
            (CMD_BASE, "launa/test_spa_001/command", b"true"),
            // Trailing slash in base causes mismatch
            (
                "launa/test_spa_001/command/",
                "launa/test_spa_001/command/pump1",
                b"true",
            ),
            // Case-sensitive: uppercase rejected
            (CMD_BASE, "launa/test_spa_001/command/PUMP1", b"true"),
            // pump7 not in allowlist
            (CMD_BASE, "launa/test_spa_001/command/pump7", b"true"),
        ];

        for (i, (base, topic, payload)) in cases.iter().enumerate() {
            let result = parse_command(base, topic, *payload);
            assert!(
                matches!(result, ParseResult::UnknownSubtopic(_)),
                "case {i}: topic '{topic}' with base '{base}': expected UnknownSubtopic, got {:?}",
                result
            );
        }
    }

    /// Invalid payload edge cases: non-UTF8, invalid toggle payloads.
    #[test]
    fn test_invalid_payload_errors() {
        // Non-UTF8 payload
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", &[0xFF, 0xFE]);
        assert!(
            matches!(result, ParseResult::InvalidPayload(_)),
            "non-utf8: expected InvalidPayload, got {:?}",
            result
        );

        // Invalid toggle payload (not "true" or "false")
        let toggle_subtopics: &[&str] = &[
            "pump1",
            "light1",
            "blower",
            "mister",
            "heat_mode",
            "hold_mode",
        ];
        for subtopic in toggle_subtopics {
            let topic = format!("{}/{}", CMD_BASE, subtopic);
            let result = parse_command(CMD_BASE, &topic, b"on");
            assert!(
                matches!(result, ParseResult::InvalidPayload(_)),
                "invalid toggle payload for {subtopic}: expected InvalidPayload, got {:?}",
                result
            );
        }
    }

    /// Backward-compatible parse_command_ok returns Some for valid, None otherwise.
    #[test]
    fn test_backward_compat_parse_command_ok() {
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
}
