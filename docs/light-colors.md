# Balboa Color Light Protocol

## How It Works

Balboa color-changing lights (MoodEFX series) use the standard light toggle command
to cycle through colors. There is **no dedicated color command** in the protocol.

### Behavior

1. Send toggle command (`0A BF 11 11 00`) to turn light on -- first color in sequence (e.g., blue)
2. Send another toggle to advance to next color (e.g., red) -- light stays on
3. Continue toggling to cycle through: blue, red, green, purple, fade, etc.
4. After a timeout (~2 minutes of no toggles), the LED controller resets to default color
5. Toggle when light is on = next color. Toggle when off = turns on at default color

The exact color sequence and timeout depend on the MoodEFX model (7-LED, 22-LED, etc.)

### Protocol Details

- **Command**: Same toggle as on/off: `0A BF 11 11 00` (light 1) or `0A BF 11 12 00` (light 2)
- **Status byte**: Offset 13 in status payload. `0x03` = light 1 on, bits 2-3 = light 2
- **Color is NOT readable**: The status byte only reports on/off, not current color
- **Brightness is NOT controllable**: Fixed brightness in hardware

### Cross-Reference Sources

- `brianfeucht/esphome-balboa-spa`: Light component uses `ColorMode::ON_OFF` only,
  sends toggle `0x11`/`0x12`, reads on/off from `input_queue[19]` (offset 13)
- `ccutrer/balboa_worldwide_app`: Protocol doc says "LF: Light flag: 0x03 == on"
- Confirmed: no color-specific protocol extensions exist in any known implementation

### Implementation Notes

No changes needed to `launa-protocol`. The existing `ToggleItem::Light1` already sends
the correct toggle command. Each MQTT command to toggle/turn on the light advances the
color. A future "cycle color" button in HA could send a toggle when the light is already
on to let users advance colors without going through off.
