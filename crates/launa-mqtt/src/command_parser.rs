//! Parse incoming MQTT command messages into `Command` variants.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use launa_protocol::command::{self, Command, TempError, ToggleItem, ABSOLUTE_MAX_TEMP_F};
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
    "set_time",
    "set_preference",
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
                Ok(n) if (1..=6).contains(&n) => n,
                _ => return ParseResult::UnknownSubtopic(subtopic.to_string()),
            };
            parse_pump_timer(payload_str, idx)
        }
        s if s.starts_with("pump") => {
            // "pump<N>" → parse N and map to ToggleItem via from_pump_index
            let num_str = &s[4..];
            let idx: usize = match num_str.parse() {
                Ok(n) if (1..=6).contains(&n) => n,
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
                Ok(n) if (1..=4).contains(&n) => n,
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
        "set_time" => parse_set_time(payload_str),
        "set_preference" => parse_set_preference(payload_str),
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
    let t = match payload.parse::<f32>() {
        Ok(t) => t,
        Err(_) => return ParseResult::InvalidPayload(format!("not a number: {:?}", payload)),
    };

    if t.is_nan() || !(0.0..=255.0).contains(&t) {
        return ParseResult::InvalidPayload(format!(
            "temperature out of representable range: {:?}",
            payload
        ));
    }

    let temp: u8 = (t + 0.5) as u8;

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
    // Accept any non-empty payload — the toggle command always toggles
    // regardless of the value. HA sends "true"/"false", the web GUI may
    // send select values like "rest" or "low" for heat_mode/temp_range.
    if payload.is_empty() {
        ParseResult::InvalidPayload("empty payload".to_string())
    } else {
        ParseResult::Valid(Command::ToggleItem(item))
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

/// Parse a set-preference command. Accepts JSON `{"code":C,"value":V}` or named
/// preferences like `{"name":"clock_mode","value":"24h"}`.
fn parse_set_preference(payload: &str) -> ParseResult {
    let trimmed = payload.trim();
    if !trimmed.starts_with('{') {
        return ParseResult::InvalidPayload(alloc::format!(
            "invalid set_preference payload, expected JSON: {:?}",
            payload
        ));
    }

    // Try named preference first: {"name":"clock_mode","value":"24h"}
    if let Some(name) = extract_json_string(trimmed, "name") {
        let value_str = extract_json_string(trimmed, "value").unwrap_or_default();
        let code = match name.as_str() {
            "reminders" => command::preference::REMINDERS,
            "temperature_scale" => command::preference::TEMPERATURE_SCALE,
            "clock_mode" => command::preference::CLOCK_MODE,
            "cleanup_cycle" => command::preference::CLEANUP_CYCLE,
            "m8_ai" => command::preference::M8_AI,
            _ => {
                return ParseResult::InvalidPayload(alloc::format!(
                    "unknown preference name: {:?}",
                    name
                ))
            }
        };
        let value: u8 = match value_str.as_str() {
            "12h" | "fahrenheit" | "off" | "no" => 0,
            "24h" | "celsius" | "on" | "yes" => 1,
            _ => match value_str.parse::<u8>() {
                Ok(v) => v,
                Err(_) => {
                    return ParseResult::InvalidPayload(alloc::format!(
                        "invalid preference value: {:?}",
                        value_str
                    ))
                }
            },
        };
        return ParseResult::Valid(Command::SetPreference { code, value });
    }

    // Fallback: {"code":C,"value":V}
    let code = extract_json_number(trimmed, "code");
    let value = extract_json_number(trimmed, "value");
    match (code, value) {
        (Some(c), Some(v)) => ParseResult::Valid(Command::SetPreference { code: c, value: v }),
        _ => ParseResult::InvalidPayload(alloc::format!(
            "invalid set_preference JSON: {:?}",
            payload
        )),
    }
}

/// Extract a string value from a simple JSON object by key name.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let pattern = alloc::format!("\"{}\":", key);
    let start = json.find(&pattern)?;
    let rest = &json[start + pattern.len()..];
    let rest = rest.trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let inner = &rest[1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

/// Parse a set-time command. Accepts `HH:MM` or JSON `{"hour":H,"minute":M}`.
/// The 24h flag is derived from the hour value (>= 13 implies 24h).
fn parse_set_time(payload: &str) -> ParseResult {
    let trimmed = payload.trim();

    // Try JSON: {"hour":H,"minute":M} or {"hour":H,"minute":M,"is_24h":bool}
    if trimmed.starts_with('{') {
        let hour = extract_json_number(trimmed, "hour");
        let minute = extract_json_number(trimmed, "minute");
        let is_24h = extract_json_bool(trimmed, "is_24h").unwrap_or(false);
        match (hour, minute) {
            (Some(h), Some(m)) => return validate_time(h, m, is_24h),
            _ => {
                return ParseResult::InvalidPayload(alloc::format!(
                    "invalid set_time JSON: {:?}",
                    payload
                ))
            }
        }
    }

    // Try HH:MM format
    let parts: Vec<&str> = trimmed.split(':').collect();
    if parts.len() == 2 {
        if let (Ok(h), Ok(m)) = (parts[0].trim().parse::<u8>(), parts[1].trim().parse::<u8>()) {
            return validate_time(h, m, false);
        }
    }

    ParseResult::InvalidPayload(alloc::format!(
        "invalid set_time payload, expected HH:MM or JSON: {:?}",
        payload
    ))
}

fn validate_time(hour: u8, minute: u8, is_24h: bool) -> ParseResult {
    if hour > 23 || minute > 59 {
        return ParseResult::InvalidPayload(alloc::format!(
            "time out of range: {}:{} (hour 0-23, minute 0-59)",
            hour,
            minute
        ));
    }
    ParseResult::Valid(Command::SetTime {
        hour,
        minute,
        is_24h,
    })
}

/// Extract a numeric value from a simple JSON object by key name.
fn extract_json_number(json: &str, key: &str) -> Option<u8> {
    let pattern = alloc::format!("\"{}\":", key);
    let start = json.find(&pattern)?;
    let rest = &json[start + pattern.len()..];
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Extract a boolean value from a simple JSON object by key name.
fn extract_json_bool(json: &str, key: &str) -> Option<bool> {
    let pattern = alloc::format!("\"{}\":", key);
    let start = json.find(&pattern)?;
    let rest = &json[start + pattern.len()..];
    let rest = rest.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
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

    /// Float temperature payloads are rounded to nearest integer.
    #[test]
    fn test_set_temperature_float_rounding() {
        let cases: &[(&str, u8)] = &[("37.5", 38), ("37.4", 37), ("100.9", 101), ("38.0", 38)];
        for (i, (payload, expected)) in cases.iter().enumerate() {
            let topic = format!("{}/set_temperature", CMD_BASE);
            let result = parse_command(CMD_BASE, &topic, payload.as_bytes());
            assert_eq!(
                result,
                ParseResult::Valid(Command::SetTemperature(*expected)),
                "case {i}: set_temperature={payload}"
            );
        }
    }

    /// Extreme float values (very large, negative, NaN, infinity) must not overflow u8 cast.
    #[test]
    fn test_set_temperature_extreme_floats() {
        let extreme_cases: &[&str] = &["1e10", "-100.0", "-0.1", "999999.0", "256.0", "1000.0"];

        for (i, payload) in extreme_cases.iter().enumerate() {
            let topic = format!("{}/set_temperature", CMD_BASE);
            let result = parse_command(CMD_BASE, &topic, payload.as_bytes());
            // All extreme cases should be rejected (either InvalidPayload or TemperatureOutOfRange)
            assert!(
                !matches!(result, ParseResult::Valid(Command::SetTemperature(_))),
                "case {i}: extreme payload '{payload}' should not produce a Valid result, got {:?}",
                result
            );
        }
    }

    /// NaN and infinity payloads are rejected as invalid.
    #[test]
    fn test_set_temperature_nan_and_infinity() {
        let cases: &[&str] = &["NaN", "inf", "-inf", "+inf"];

        for (i, payload) in cases.iter().enumerate() {
            let topic = format!("{}/set_temperature", CMD_BASE);
            let result = parse_command(CMD_BASE, &topic, payload.as_bytes());
            assert!(
                matches!(result, ParseResult::InvalidPayload(_)),
                "case {i}: payload '{payload}' should be InvalidPayload, got {:?}",
                result
            );
        }
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

    /// Invalid payload edge cases: non-UTF8, empty payloads.
    #[test]
    fn test_invalid_payload_errors() {
        // Non-UTF8 payload
        let result = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", &[0xFF, 0xFE]);
        assert!(
            matches!(result, ParseResult::InvalidPayload(_)),
            "non-utf8: expected InvalidPayload, got {:?}",
            result
        );

        // Empty payloads are rejected for toggle commands
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
            let result = parse_command(CMD_BASE, &topic, b"");
            assert!(
                matches!(result, ParseResult::InvalidPayload(_)),
                "empty toggle payload for {subtopic}: expected InvalidPayload, got {:?}",
                result
            );
        }
    }

    /// Toggle commands accept arbitrary non-empty payloads (e.g. "on", "rest", "low").
    #[test]
    fn test_toggle_accepts_non_bool_payloads() {
        let cases: &[(&str, &[u8])] = &[
            ("pump1", b"on"),
            ("heat_mode", b"rest"),
            ("temp_range", b"low"),
            ("blower", b"1"),
        ];
        for (subtopic, payload) in cases {
            let topic = format!("{}/{}", CMD_BASE, subtopic);
            let result = parse_command(CMD_BASE, &topic, *payload);
            assert!(
                matches!(result, ParseResult::Valid(Command::ToggleItem(_))),
                "toggle {subtopic} with payload '{}': expected Valid(ToggleItem), got {:?}",
                std::str::from_utf8(payload).unwrap(),
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
            parse_command_ok(CMD_BASE, "launa/test_spa_001/command/pump1", b""),
            None,
        );
    }

    /// Set time commands: HH:MM format, JSON format, and error cases.
    #[test]
    fn test_set_time_hh_mm_format() {
        let cases: &[(&[u8], u8, u8, bool)] = &[
            (b"14:30", 14, 30, false),
            (b"09:05", 9, 5, false),
            (b"23:59", 23, 59, false),
            (b"00:00", 0, 0, false),
            (b" 8:07 ", 8, 7, false),
        ];

        for (i, (payload, h, m, is_24h)) in cases.iter().enumerate() {
            let topic = format!("{}/set_time", CMD_BASE);
            let result = parse_command(CMD_BASE, &topic, *payload);
            assert_eq!(
                result,
                ParseResult::Valid(Command::SetTime {
                    hour: *h,
                    minute: *m,
                    is_24h: *is_24h,
                }),
                "case {i}: set_time={}",
                std::str::from_utf8(payload).unwrap()
            );
        }
    }

    #[test]
    fn test_set_time_json_format() {
        let cases: &[(&[u8], u8, u8, bool)] = &[
            (br#"{"hour":14,"minute":30}"#, 14, 30, false),
            (br#"{"hour":9,"minute":5,"is_24h":true}"#, 9, 5, true),
            (br#"{"hour":0,"minute":0,"is_24h":false}"#, 0, 0, false),
            (br#"{"hour":23,"minute":59,"is_24h":true}"#, 23, 59, true),
        ];

        for (i, (payload, h, m, is_24h)) in cases.iter().enumerate() {
            let topic = format!("{}/set_time", CMD_BASE);
            let result = parse_command(CMD_BASE, &topic, *payload);
            assert_eq!(
                result,
                ParseResult::Valid(Command::SetTime {
                    hour: *h,
                    minute: *m,
                    is_24h: *is_24h,
                }),
                "case {i}: set_time={}",
                std::str::from_utf8(payload).unwrap()
            );
        }
    }

    #[test]
    fn test_set_time_invalid() {
        let cases: &[(&[u8], &str)] = &[
            (b"25:00", "out of range"),
            (b"12:60", "out of range"),
            (b"abc", "invalid"),
            (b"", "invalid"),
            (b"14", "invalid"),
            (b"24:00", "out of range"),
        ];

        for (i, (payload, _expected)) in cases.iter().enumerate() {
            let topic = format!("{}/set_time", CMD_BASE);
            let result = parse_command(CMD_BASE, &topic, *payload);
            assert!(
                matches!(result, ParseResult::InvalidPayload(_)),
                "case {i}: set_time={}: expected InvalidPayload, got {:?}",
                std::str::from_utf8(payload).unwrap(),
                result
            );
        }
    }
}
