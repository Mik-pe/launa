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
| 12 | P2 | Pump status (pumps 5-6): bits 0-1=Pump5, bits 2-3=Pump6 |
| 13 | CB | Circ pump (bit 1), Blower (bits 2-3) |
| 14 | LF | Light: bits 0-1=Light 1, bits 2-3=Light 2 |
| 15 | MR | Mister: 0=OFF, 1=ON |
| 20 | ST | Set Temperature (÷2 if Celsius) |

#### Pump Status Decoding

Each pump occupies 2 bits. Valid values: 0=off, 1=low, 2=high.

```text
Pump 1 = (P1 >> 0) & 0x03
Pump 2 = (P1 >> 2) & 0x03
Pump 3 = (P1 >> 4) & 0x03
Pump 4 = (P1 >> 6) & 0x03
Pump 5 = (P2 >> 0) & 0x03
Pump 6 = (P2 >> 2) & 0x03
```

### Configuration Response

- Type: `0A BF 94` or `0A BF 2E`

```
Offset: 0  1  2  3  4  5  6  7  8  9 ...
Data:   02 02 80 00 15 27 10 AB D2 00 ...
```

Configuration byte mapping (from `0A BF 2E`):

| Byte | Bits | Description |
|------|------|-------------|
| 5 | 0-1 | Pump 1 (0=none, 1=1-speed, 2=2-speed) |
| 5 | 2-3 | Pump 2 |
| 5 | 4-5 | Pump 3 |
| 5 | 6-7 | Pump 4 |
| 6 | 0-1 | Pump 5 |
| 6 | 6-7 | Pump 6 |
| 7 | 0-1 | Light 1 |
| 7 | 2-3 | Light 2 |
| 8 | 0-1 | Blower |
| 8 | 7 | Circulation pump |
| 9 | 0 | Aux 1 |
| 9 | 1 | Aux 2 |
| 9 | 4-5 | Mister |
| 3 | 0 | Temperature scale (0=F, 1=C) |

### Filter Cycles Response

- Type: `0A BF 23`

```
Offset: 0  1  2  3  4  5  6  7
Field:  1H 1M 1D 1E 2H 2M 2D 2E
```

- Filter 1 start hour, minute, duration hours, duration minutes
- Filter 2 start hour (high bit = enable flag), minute, duration hours, duration minutes

### Information Response

- Type: `0A BF 24`

```
Offset: 0  1  2  3  4-11     12 13-16     17-18  19-20
Field:  SI SI SV SV SM(8B)   SU CS(4B)    HT HT  DS DS
```

- SI: Software ID (e.g. "M100_220")
- SV: Software Version (e.g. "V17")
- SM: System Model, ASCII (e.g. "BFBP20  ")
- SU: Current Setup
- CS: Configuration Signature (e.g. "3D12382E")
- HT: Heater Voltage (0x01=240V), Heater Type (0x0A=Standard)
- DS: DIP Switch Settings

### Fault Log Response

- Type: `0A BF 28`

```
Offset: 0  1  2  3  4  5  6  7  8  9
Field:  FC EN MC DD HH MM FF ST TA TB
```

| Offset | Description |
|--------|-------------|
| 0 | Fault Count |
| 1 | Entry Number |
| 2 | Message Code (see fault codes below) |
| 3 | Days Ago |
| 4 | Time Hours |
| 5 | Time Minutes |
| 6 | Flags (Heating Mode, Temp Range) |
| 7 | Set Temperature |
| 8 | Sensor A Temperature |
| 9 | Sensor B Temperature |

#### Fault Codes

| Code | Description |
|------|-------------|
| 15 | Sync |
| 16 | Low flow |
| 17 | Flow failed |
| 18 | Settings reset |
| 19 | Priming |
| 20 | Clock failed |
| 22 | Program memory |
| 26 | Sync — call service |
| 27 | Heater dry |
| 28 | Heater maybe dry |
| 29 | Water too hot |
| 30 | Heater too hot |
| 31 | Sensor A fault |
| 32 | Sensor B fault |
| 34 | Pump stuck on |
| 35 | Hot fault |
| 36 | GFCI test failed |
| 37 | Standby / Hold |

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
| Blower | 0x0C |
| Light 1 | 0x11 |
| Hold Mode | 0x3C |
| Heating Mode | 0x51 |
| Temperature Range | 0x50 |

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

### Settings Request

- Type: `0A BF 22`

| Request | Payload |
|---------|---------|
| Panel | `00 00 01` |
| Filter Cycles | `01 00 00` |
| Information | `02 00 00` |
| Preferences | `08 00 00` |
| Fault Log | `20 EN 00` (EN=entry number, FF=last) |

### Nothing to Send (no-op ack)

- Type: `<ID> BF 07`
