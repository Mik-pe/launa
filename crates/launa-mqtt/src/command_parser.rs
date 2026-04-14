//! Parse incoming MQTT command messages into `Command` variants.

use launa_protocol::command::{Command, ToggleItem};

/// Parse an incoming MQTT message into a `Command`.
///
/// `command_topic_base` is the base command topic, e.g. `"launa/test_spa/command"`.
/// `topic` is the full topic the message arrived on.
/// `payload` is the raw MQTT payload bytes.
///
/// Returns `Some(Command)` if the message is a recognized command, or `None` if
/// the topic does not match any known command subtopic.
pub fn parse_command(command_topic_base: &str, topic: &str, payload: &[u8]) -> Option<Command> {
    // topic must be exactly command_topic_base + "/" + subcommand
    if !topic.starts_with(command_topic_base) {
        return None;
    }

    let suffix = &topic[command_topic_base.len()..];

    // Must start with '/'
    if !suffix.starts_with('/') {
        return None;
    }

    let subtopic = &suffix[1..];

    let payload_str = core::str::from_utf8(payload).ok()?;

    match subtopic {
        "pump1" => parse_toggle(payload_str, ToggleItem::Pump1),
        "pump2" => parse_toggle(payload_str, ToggleItem::Pump2),
        "pump3" => parse_toggle(payload_str, ToggleItem::Pump3),
        "light1" => parse_toggle(payload_str, ToggleItem::Light1),
        "blower" => parse_toggle(payload_str, ToggleItem::Blower),
        "heat_mode" => parse_toggle(payload_str, ToggleItem::HeatingMode),
        "temp_range" => parse_toggle(payload_str, ToggleItem::TemperatureRange),
        "hold_mode" => parse_toggle(payload_str, ToggleItem::HoldMode),
        "set_temperature" => parse_set_temperature(payload_str),
        _ => None,
    }
}

fn parse_toggle(payload: &str, item: ToggleItem) -> Option<Command> {
    // We accept any payload for toggles; the command is sent regardless
    // since the spa protocol uses toggle semantics (press = toggle).
    // But we only produce the command if payload is "true" or "false".
    match payload {
        "true" | "false" => Some(Command::ToggleItem(item)),
        _ => None,
    }
}

fn parse_set_temperature(payload: &str) -> Option<Command> {
    let temp: u8 = payload.parse().ok()?;
    Some(Command::SetTemperature(temp))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD_BASE: &str = "launa/test_spa_001/command";

    #[test]
    fn test_parse_pump1_on() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Pump1)));
    }

    #[test]
    fn test_parse_pump1_off() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", b"false");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Pump1)));
    }

    #[test]
    fn test_parse_pump2() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/pump2", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Pump2)));
    }

    #[test]
    fn test_parse_pump3() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/pump3", b"false");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Pump3)));
    }

    #[test]
    fn test_parse_light1() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/light1", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Light1)));
    }

    #[test]
    fn test_parse_blower() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/blower", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::Blower)));
    }

    #[test]
    fn test_parse_heat_mode() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/heat_mode", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::HeatingMode)));
    }

    #[test]
    fn test_parse_temp_range() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/temp_range", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::TemperatureRange)));
    }

    #[test]
    fn test_parse_hold_mode() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/hold_mode", b"true");
        assert_eq!(cmd, Some(Command::ToggleItem(ToggleItem::HoldMode)));
    }

    #[test]
    fn test_parse_set_temperature() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/set_temperature", b"104");
        assert_eq!(cmd, Some(Command::SetTemperature(104)));
    }

    #[test]
    fn test_parse_set_temperature_low() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/set_temperature", b"80");
        assert_eq!(cmd, Some(Command::SetTemperature(80)));
    }

    #[test]
    fn test_parse_unknown_subtopic() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/unknown", b"true");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_wrong_base() {
        let cmd = parse_command(
            "launa/other_spa/command",
            "launa/test_spa_001/command/pump1",
            b"true",
        );
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_invalid_toggle_payload() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/pump1", b"on");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_invalid_temperature() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command/set_temperature", b"abc");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_state_topic_not_command() {
        let cmd = parse_command(
            CMD_BASE,
            "launa/test_spa_001/state",
            b"true",
        );
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_exact_base_topic_no_subtopic() {
        let cmd = parse_command(CMD_BASE, "launa/test_spa_001/command", b"true");
        assert_eq!(cmd, None);
    }
}
