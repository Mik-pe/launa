# Balboa Spa Control Protocol

Reference for the BP series WiFi/RS-485 protocol. Source:
<https://github.com/ccutrer/balboa_worldwide_app/blob/main/doc/protocol.md>

## Transport

- **TCP**: Port 4257 (via WiFi module). Spa immediately sends status updates ~once per second.
- **RS-485**: 115200 baud, 8N1. Shared bus — only send after "Ready" message.
- **Discovery**: UDP broadcast to port 30303. Spa responds with hostname (`BWGSPA`) + MAC address.
  Balboa MAC prefix: `00:15:27`.

## Message Frame Format

```
 0  1  2  3  4  ... -2 -1
MS ML MT MT MT ... CS ME
```

- **MS, ME**: Start/End marker (always `0x7E` / `~`)
- **ML**: Message Length (excluding MS/ME)
- **MT**: Message Type (2 bytes)
- **CS**: CRC-8 checksum (init=0x02, poly=0x07, no reflect, xorout=0x02)

### CRC-8 Calculation

```text
init = 0x02
poly = 0x07
reflect_in = false
reflect_out = false
xor_out = 0x02

CRC is computed over the full message body (length byte through last data byte),
excluding the start/end 0x7E markers.
```

## Client Registration (ID Assignment)

1. Spa broadcasts: `FE BF 00` — "any new clients?"
2. Client responds: `FE BF 01 02 F1 73` — ID request (with CRC)
3. Spa replies: `FE BF 02 <ID>` — assigned ID byte
4. Client acknowledges: `<ID> BF 03` — ID ack
5. Client waits for `<ID> BF 06` (ready-to-send) before sending commands

## Incoming Messages

### Ready Indicator

- Type: `10 BF 06` (RS-485 only)
- Indicates it is safe to immediately send a message onto the bus.

### Status Update (sent every ~1 second)

- Type: `FF AF 13`

```
Offset: 0  1  2  3  4  5  6  7  8  9 10 11 12 13 14 15 16 17 18 19 20 21 22 23
Field: ST IM CT HH MM HM RT SA SB F9 FA P1 P2 CB LF MR -- -- -- -- ST -- -- --
```

> Verified against real Balboa BP6013G1 hardware (NorthernMan54/esp32_balboa_spa).

| Offset | Field | Description |
|--------|-------|-------------|
| 0 | ST | Spa State: 0x00=Running, 0x05=Hold Mode, 0x14=A/B Temps ON, 0x17=Test Mode |
| 1 | IM | Init Mode: 0x00=Idle, 0x01=Priming Mode, 0x02=Fault, 0x03=Reminder |
| 2 | CT | Current Temperature (÷2 if Celsius; 0xFF = unknown) |
| 3 | HH | Hour (always 0-24) |
| 4 | MM | Minute |
| 5 | HM | Heating Mode: 0=Ready, 1=Rest, 3=Ready-in-Rest |
| 6 | RT | Reminder Type: 0x00=None, 0x04=Clean filter, etc. |
| 7 | SA | Sensor A Temperature (or Hold Timer Minutes if in Hold Mode) |
| 8 | SB | Sensor B Temperature (if A/B Temps ON, else 0) |
| 9 | F9 | Flags: bit 0=Temperature Scale (0=F, 1=C), bit 1=24h time, bits 2-3=Filter Mode |
| 10 | FA | Flags: bit 2=Temp Range (0=Low, 1=High), bit 3=Needs Heat, bits 4-5=Heating State |
| 11 | P1 | Pump status (pumps 1-4): bits 0-1=Pump1, bits 2-3=Pump2, bits 4-5=Pump3, bits 6-7=Pump4 |
| 12 | P2 | Pump status (pumps 5-6): bits 0-1=Pump5, bits 2-3=Pump6 (see P2 bit table below) |
| 13 | CB | Circ pump (bit 1), Blower (bits 2-3) |
| 14 | LF | Lights: bits 0-1=Light 1, bits 2-3=Light 2, bits 4-5=Light 3, bits 6-7=Light 4 |
| 15 | MR | Mister: 0=OFF, 1=ON |
| 20 | ST | Set Temperature (÷2 if Celsius) |

#### Pump Status Decoding

Each pump occupies 2 bits. Valid values: 0=off, 1=low, 2=high.

##### P1 Byte (offset 11) — Pumps 1-4

```text
Bit  7  6 | 5  4 | 3  2 | 1  0
     Pump4  Pump3  Pump2  Pump1
```

##### P2 Byte (offset 12) — Pumps 5-6

```text
Bit  7  6 | 5  4 | 3  2 | 1  0
     (unused)(unused) Pump6  Pump5
```

Extraction formula:

```text
Pump 1 = (P1 >> 0) & 0x03
Pump 2 = (P1 >> 2) & 0x03
Pump 3 = (P1 >> 4) & 0x03
Pump 4 = (P1 >> 6) & 0x03
Pump 5 = (P2 >> 0) & 0x03
Pump 6 = (P2 >> 2) & 0x03
```

### Configuration Response

- Type: `0A BF 2E`
- Length: 11

| Byte | Name | Values |
|------|------|--------|
| 0 | Pumps 1-4 | Bits N to N+1: Pump N/2+1 (0=None, 1=1-speed, 2=2-speed) |
| 1 | Pumps 5-6 | Bits 0-1: Pump 5, Bits 6-7: Pump 6 (0=None, 1=1-speed, 2=2-speed) |
| 2 | Lights | Bits 0-1: Light 1, Bits 6-7: Light 2 (0=None, 1=Present) |
| 3 | Flags Byte 3 | Bits 0-1: Blower, Bit 7: Circulation Pump |
| 4 | Flags Byte 4 | Bit 0: Aux 1, Bit 1: Aux 2, Bits 4-5: Mister |
| 5 | Unknown | 0x00 or 0x68 |

Examples:

```
2 1-speed pumps, 1 light, circ pump:  0B 10 BF 2E 05 00 01 90 00 68
2 2-speed pumps, 1 light, no circ/blower: 0B 0A BF 2E 0A 00 01 50 00 00
2 2-speed + 1 1-speed pump, 1 light, circ pump: 0B 10 BF 2E 1A 00 01 90 00 68
3 2-speed pumps, 1 light, no circ/blower: 0B 10 BF 2E 2A 00 01 50 00 00
```

### WiFi Module Configuration Response

- Type: `0A BF 94`
- Length: 29

| Byte(s) | Name | Values |
|---------|------|--------|
| 0-2 | Unknown | Unknown |
| 3-8 | Full MAC address | Varies |
| 9-16 | Unknown | 0 |
| 17-19 | MAC address: OUI | 00:15:27 (Balboa Instruments) |
| 20-21 | Unknown | 0xFF |
| 22-24 | MAC address: NIC-specific | Varies |

Examples:

```
1D 0A BF 94 02 02 80 00 15 27 10 AB D2 00 00 00 00 00 00 00 00 00 15 27 FF FF 10 AB D2
1D 0A BF 94 02 14 80 00 15 27 3F 9B 95 00 00 00 00 00 00 00 00 00 15 27 FF FF 3F 9B 95
```

### Settings Request

- Type: `0A BF 22`
- Length: 8
- Payload: `CC AA BB` (settings code + 2 argument bytes)

| Code | Name | Arguments | Response |
|------|------|-----------|----------|
| 0x00 | Configuration | `0x00 0x01` | Configuration Response (0x2E) |
| 0x01 | Filter Cycles | `0x00 0x00` | Filter Cycles Message (0x23) |
| 0x02 | Information | `0x00 0x00` | Information Response (0x24) |
| 0x04 | Unknown | `0x00 0x00` | Settings 0x04 Response (0x25) |
| 0x08 | Preferences | `0x00 0x00` | Preferences Response (0x26) |
| 0x10 | Unknown | `0x00 0x00` | (None) |
| 0x20 | Fault Log | `EN 0x00` | Fault Log Response (0x28) |
| 0x40 | Unknown | `0x00 0x00` | Settings 0x40 Response |
| 0x80 | GFCI Test | `0x00 0x00` | GFCI Test Response (0x2B) |

> EN: entry number 0-23 (0xFF = last fault).

### Filter Cycles Message

- Type: `0A BF 23`
- Length: 12
- Sent by Main Board in response to Settings Request, or sent by client to write filter cycle settings.
- Writing does not generate a response from the Main Board.

| Byte | Name | Values |
|------|------|--------|
| 0 | Filter 1 Start: Hour | 0-23 |
| 1 | Filter 1 Start: Minute | 0-59 |
| 2 | Filter 1 Duration: Hours | 0-23 |
| 3 | Filter 1 Duration: Minutes | 0-59 |
| 4 | Filter 2 Enable/Start: Hour | Bits 0-6: Hour (0-23), Bit 7: Enable (0=OFF, 1=ON) |
| 5 | Filter 2 Start: Minute | 0-59 |
| 6 | Filter 2 Duration: Hours | 0-23 |
| 7 | Filter 2 Duration: Minutes | 0-59 |

### Information Response

- Type: `0A BF 24`
- Length: 25

| Byte(s) | Name | Description/Values |
|---------|------|---------------------|
| 0-3 | Software ID (SSID) | Displayed (decimal): "M\<byte0\>_\<byte1\> V\<byte2\>[.\<byte3\>]" |
| 4-11 | System Model Number | ASCII-encoded string |
| 12 | Current Setup Number | Refer to controller tech sheets |
| 13-16 | Configuration Signature | Checksum of system configuration file |
| 17 | Heater Voltage | 0x01=240V |
| 18 | Heater Type | 0x06, 0x0A=Standard |
| 19-20 | DIP Switch Settings | LSB-first (bit 0 of byte 19 is position 1) |

Examples:

```
M100_210 V6, CSTBP3UL, Setup 2, Sig 57072108, 240V, Standard, DIP 0100000000
  25 10 BF 24 64 D2 06 00 43 53 54 42 50 33 55 4C 02 57 07 21 08 01 0A 02 00

M100_201 V44, MBP501UX, Setup 3, Sig A82F6383, 240V, Standard, DIP 101000
  25 10 BF 24 64 C9 2C 00 4D 42 50 35 30 31 55 58 03 A8 2F 63 83 01 06 05 00
```

### Settings 0x04 Response

- Type: `0A BF 25`
- Length varies by SSID (14-15 bytes observed).
- Payload format unknown.

Examples:

```
0E 10 BF 25 02 02 32 63 50 68 20 07 01
0E 0A BF 25 05 01 32 63 50 68 61 07 41
0F 10 BF 25 09 03 32 63 50 68 49 03 41 02
```

### Preferences Response

- Type: `0A BF 26`
- Length: 23
- Sent by Main Board after Settings Request (same channel), or after Set Preference Request (broadcast channel).

| Byte(s) | Name | Description/Values |
|---------|------|---------------------|
| 0 | Unknown | 0 |
| 1 | Reminders | 0=OFF, 1=ON |
| 2 | Unknown | 0 |
| 3 | Temperature Scale | 0=1°F, 1=0.5°C |
| 4 | Clock Mode | 0=12-hour, 1=24-hour |
| 5 | Cleanup Cycle | 0=OFF, 1-8 (30 minute increments) |
| 6 | Dolphin Address | 0=none, 1-7=address |
| 7 | Unknown | 0 |
| 8 | M8 Artificial Intelligence | 0=OFF, 1=ON |
| 9-17 | Unknown | 0 |

### Fault Log Response

- Type: `0A BF 28`
- Length: 15

| Byte | Name | Values |
|------|------|--------|
| 0 | Total Entries | 0-24 |
| 1 | Entry Number | 0-23 (0=Entry #1) |
| 2 | Message Code | (see fault codes below) |
| 3 | Days Ago | 0-255 |
| 4 | Time: Hour | 0-23 |
| 5 | Time: Minute | 0-59 |
| 6 | Flags | TODO |
| 7 | Set Temperature | Scaled by Temperature Scale |
| 8 | Sensor A Temperature | Scaled by Temperature Scale |
| 9 | Sensor B Temperature | Scaled by Temperature Scale |

#### Fault Codes

| Code | Message |
|------|---------|
| 15 | Sensors are out of sync |
| 16 | The water flow is low |
| 17 | The water flow has failed |
| 18 | The settings have been reset |
| 19 | Priming Mode |
| 20 | The clock has failed |
| 21 | The settings have been reset |
| 22 | Program memory failure |
| 26 | Sensors are out of sync — Call for service |
| 27 | The heater is dry |
| 28 | The heater may be dry |
| 29 | The water is too hot |
| 30 | The heater is too hot |
| 31 | Sensor A Fault |
| 32 | Sensor B Fault |
| 34 | A pump may be stuck on |
| 35 | Hot fault |
| 36 | The GFCI test failed |
| 37 | Standby Mode (Hold Mode) |

### GFCI Test Response

- Type: `0A BF 2B`
- Length: 6
- Sent during initialization and after Settings Request with code 0x80.

| Byte 0 | Description |
|--------|-------------|
| 0x00 | N/A or FAIL |
| 0x01 | PASS |

## Outgoing Messages

### Configuration Request

- Type: `0A BF 04`
- No payload.

### Toggle Item

- Type: `0A BF 11`
- Payload: `II 00`

| Item | Code |
|------|------|
| Pump 1 | 0x04 |
| Pump 2 | 0x05 |
| Pump 3 | 0x06 |
| Pump 4 | 0x07 |
| Pump 5 | 0x08 |
| Pump 6 | 0x09 |
| Blower | 0x0C |
| Mister | 0x0E |
| Light 1 | 0x11 |
| Light 2 | 0x12 |
| Light 3 | 0x13 |
| Light 4 | 0x14 |
| Aux 1 | 0x16 |
| Aux 2 | 0x17 |
| Soak Mode | 0x1D |
| Circulation Pump | 0x3D |
| Hold Mode | 0x3C |
| Temperature Range | 0x50 |
| Heating Mode | 0x51 |
| Normal Operation | 0x01 |
| Clear Notification | 0x03 |

### Set Temperature

- Type: `0A BF 20`
- Payload: `TT` (temperature value, ×2 if Celsius)
- Ranges: F high 80-104, F low 50-80, C high 26-40, C low 10-26

### Set Temperature Scale

- Type: `0A BF 27`
- Payload: `01 TS` (TS: 0x00=F, 0x01=C)

### Set Time

- Type: `0A BF 21`
- Payload: `HH MM` (high bit of HH enables 24h time)

### Set Filter Cycles

- Type: `0A BF 23`
- Length: 12
- Same format as Filter Cycles Message (see Incoming Messages).
- Client sends to write filter cycle settings. Main Board does not respond.

### Set Preference Request

- Type: `0A BF 27`
- Length: 7
- Payload: `CC VV` (preference code + value)
- Main Board responds with a full Preferences Response (0x26) on broadcast channel.

| Code | Name | Values |
|------|------|--------|
| 0x00 | Reminders | 0=OFF, 1=ON |
| 0x01 | Temperature Scale | 0=1°F, 1=0.5°C |
| 0x02 | Clock Mode | 0=12-hour, 1=24-hour |
| 0x03 | Cleanup Cycle | 0=OFF, 1-8 (30 minute increments) |
| 0x04 | Dolphin Address | 0=none, 1-7=address |
| 0x05 | Unknown | Unknown |
| 0x06 | M8 Artificial Intelligence | 0=OFF, 1=ON |

### Change Setup Request

- Type: `0A BF 2A`
- Length: 6
- Payload: setup number
- Main Board performs a reset after processing.

### Lock Request

- Type: `0A BF 2D`
- Length: 6

| Byte 0 | Description |
|--------|-------------|
| 0x01 | Lock Settings |
| 0x02 | Lock Panel |
| 0x03 | Unlock Settings |
| 0x04 | Unlock Panel |

### Toggle Test Setting Request

- Type: `0A BF E0`
- Length: 6

| Byte 0 | Description |
|--------|-------------|
| 0x03 | Sensor A/B Temperatures |
| 0x04 | Timeouts |
| 0x05 | Temp Limits |

### Nothing to Send (no-op ack)

- Type: `<ID> BF 07`
