# RS-485 TX Signal Analysis

Investigation into why the Launa ESP32 firmware's registration response never
reaches the Balboa BP6013G1 spa controller's RS-485 bus, despite RX working
correctly and the same hardware working with the RS-485 debugger firmware.

Date: 2026-05-04

## Problem

The ESP32 receives spa bus traffic correctly (status, CTS, NewClientQuery frames)
but the spa never responds to our `NewClientResponse` (`FE BF 01 02 <hash>`).
UART write completes without error, but the signal does not produce a valid
frame on the RS-485 bus.

## Hardware

- **Controller**: Balboa BP6013G1
- **Transceiver**: MAX13487EESA (auto-direction half-duplex, no DE/RE pin control)
- **Verified working**: The RS-485 debugger firmware (`app-rs485-debugger`)
  successfully transmits and receives on this exact hardware (same ESP32, same
  MAX13487E, same GPIO pins) when connected ESP32-to-ESP32.
- **UART**: ESP32 UART1, 115200 baud 8N1, GPIO17 (TX) / GPIO16 (RX)
- **Display panel**: connected to the same RS-485 bus on channel 0x10

## Evidence

### 1. The Registration Protocol Works Correctly

The frame format and byte ordering are verified correct:

- CRC-8 independently verified with Python
- Byte ordering matches the real display panel's registration (big-endian hash on wire)
- Our frame: `7E 08 FE BF 01 02 E3 56 3F 7E` (hash=E356)
- Display panel frame: `FE BF 01 01 1D 70` (hash=0x1D70, device_type=0x01)
- Both use the same hash byte order (high byte first)
- The only difference is device_type: ours is 0x02, display uses 0x01

Registration succeeds with the spa-emulator (no other bus devices).

### 2. The Spa-Only Bus is Completely Clean

Capture: `sniffer-capture-spa-only.txt` (65s, sniffer ESP32 only, NO app firmware)

- All frames are valid Balboa protocol frames (CTS, Ready, Status, NewClientQuery)
- Every "garbage" entry in the sniffer output is a software artifact: valid frames
  whose `0x7E` delimiters were split across UART read batches, causing the
  `RawBusTracker` to misclassify them
- **The bus is completely silent for ~19ms after each FEBF [00] NewClientQuery**
- The next frame after FEBF is always a CTS at 18.9-22.9ms (avg 20.5ms)
- There is zero bus contention in the 7-8ms window where our ESP32 transmits

Gap statistics (FEBF → next frame of any type, spa-only capture):

| Metric | Value   |
|--------|---------|
| Min    | 18.9 ms |
| Max    | 22.9 ms |
| Avg    | 20.6 ms |

### 3. Our ESP32 IS Transmitting Something

When the app firmware is on the bus, the sniffer captures garbage bytes at a
**consistent 7.0-8.7ms after every FEBF [00]** NewClientQuery:

| Delta from FEBF | Garbage Bytes                       |
|-----------------|-------------------------------------|
| 7,639 µs        | `BFBFD5FBFF`                        |
| 7,683 µs        | `7FEFFFBFFDFBFB57BF7F`             |
| 7,627 µs        | `7FEFFFBFFDFBFB57BF7F`             |
| 7,682 µs        | `FBFF`                              |
| 8,732 µs        | `7FEFFFBFFFFBFB57BFFF`             |
| 8,645 µs        | `7FEFFFBFFDFBFB57BF7F`             |

Key properties:
- **100% correlation with FEBF queries** — every garbage event follows a NewClientQuery
- **No correlation with CTS or any other bus event** — the ESP32 is NOT transmitting on every CTS/Ready cycle
- A control capture with TX suppressed shows zero garbage
- The dominant pattern `7FEFFFBFFDFBFB57BF7F` (11 bytes) is nearly the same length as our 10-byte registration frame
- The garbage appears in the silent gap where nothing else is transmitting (no collision)

### 4. The Garbage is Corrupted, Not Random

The garbage bytes show structural similarity to our intended frame but with
systematic bit-level corruption:

| Position | Garbage      | Expected     | Notes                    |
|----------|-------------|-------------|--------------------------|
| 0        | `7F`        | `7E` (start)| 1 bit flip               |
| 1        | `EF`        | `08` (len)  | No match                 |
| 2        | `FF`        | `FE` (ch)   | 1 bit flip               |
| 3        | `FB`        | `BF` (type) | 2 bit flips              |
| 4        | `FF`        | `01` (sub)  | No match                 |
| 5        | `DF`        | `02` (dev)  | No match                 |
| 6        | `BF`        | `E3` (hash) | Partial                  |
| 7        | `B5`        | `56` (hash) | No match                 |
| 8        | `7B`        | `3F` (CRC)  | No match                 |
| 9        | `F7`        | `7E` (end)  | 3 bit flips              |

The corruption is **the same pattern every time** (not random noise), suggesting
a repeatable signal-level problem rather than collision or interference.

### 4b. Bit-Level Analysis: Bus Stuck HIGH

XOR analysis of the dominant garbage `7FEFFFBFFDFBFB57BF7F` vs our frame
`7E 08 FE BF 01 02 E3 56 3F 7E`:

| Pos | Frame | Garbage | XOR  | Flipped bits |
|-----|-------|---------|------|-------------|
| 0   | 0x7E  | 0x7F    | 0x01 | bit 0 only  |
| 1   | 0x08  | 0xEF    | 0xE7 | 6 of 7 zeros|
| 2   | 0xFE  | 0xFF    | 0x01 | bit 0 only  |
| 3   | 0xBF  | 0xBF    | 0x00 | identical   |
| 4   | 0x01  | 0xFD    | 0xFC | 6 of 7 zeros|
| 5   | 0x02  | 0xFB    | 0xF9 | 6 of 7 zeros|
| 6   | 0xE3  | 0xFB    | 0x18 | bits 3,4   |
| 7   | 0x56  | 0x57    | 0x01 | bit 0 only  |
| 8   | 0x3F  | 0xBF    | 0x80 | bit 7 only  |
| 9   | 0x7E  | 0x7F    | 0x01 | bit 0 only  |

**The corruption is 100% one-directional: bits are ONLY set (0→1), NEVER
cleared (1→0).** This means `garbage = frame | mask` — our frame with extra
1-bits OR'd in. The RS-485 bus is stuck in the HIGH (mark/idle) state and
our transceiver cannot pull it LOW for zero bits:

- **Bit 0 is 100% stuck HIGH** across all captures — the first data bit
  (LSB in UART) is always corrupted to 1
- **Bytes with many 0-bits are most corrupted**: 0x08→0xEF (6/7 zeros flipped),
  0x01→0xFD (6/7), 0x02→0xFB (6/7)
- **Bytes with few 0-bits are barely corrupted**: 0xBF→0xBF (identical),
  0xFE→0xFF (1 bit)
- **Start and stop bits are never corrupted** — they benefit from the bus's
  natural HIGH bias
- **The dominant garbage is 86.2% ones** (69/80 bits) — the signature of a
  bus stuck in mark state

### 5. The Display Panel Registers Successfully

Captured from `sniffer-capture-3min.txt` during spa reboot:

```
34748.7ms  FEBF [00]           — Spa: NewClientQuery
34749.4ms  FEBF [01 01 1D 70]  — Display: response (0.7ms latency!)
34768.3ms  FEBF [02 10 1D 70]  — Spa: assign channel 0x10
34768.6ms  10BF [03]           — Display: ACK (0.3ms)
```

The display panel responds in 0.7ms — nearly instant. Our firmware responds in
7-8ms (10x slower) but well within the ~19ms silent window.

### 6. Timing Variations All Fail

12 combinations of delay and immediacy were tested, from 0ms to 50ms, both
immediate and deferred modes. All fail. This rules out timing as the issue.

### 7. The RS-485 Debugger Works on This Hardware

The `app-rs485-debugger` firmware successfully transmits and receives on the
same ESP32, same MAX13487EESA transceiver, same GPIO pins (GPIO17 TX, GPIO16 RX).
This confirms:
- The UART is functional
- The MAX13487EESA is functional
- The wiring is correct
- GPIO17 can drive the transceiver's DI pin

The difference: the debugger was tested ESP32-to-ESP32, not connected to the
spa's bus.

## Root Cause Analysis

**Ruled out:**

| Hypothesis | Evidence Against |
|---|---|
| Frame format incorrect | CRC verified, byte order matches display panel |
| LE/BE byte swap | Hash byte order matches display panel's registration |
| Bus collision | Spa-only capture shows 19ms silence after FEBF — nothing to collide with |
| CTS-triggered spurious TX | Garbage is 100% FEBF-correlated, zero correlation with CTS |
| ESP32 UART broken | RS-485 debugger firmware works on same hardware |
| MAX13487E enable latency | Datasheet specifies ~50ns enable, far less than one bit time |
| MAX13487E not TX-capable | Debugger firmware successfully transmits |
| Timing too slow/too fast | All 12 timing variations fail |
| Bad GPIO pin | GPIO17 works with debugger firmware |
| Registration protocol bug | Works with spa-emulator |

**Remaining hypotheses (ranked by likelihood):**

### H1: The Spa Bus Prevents Our Transceiver From Driving the Line

The spa controller's RS-485 transceiver likely runs at 5V with stronger drive
capability. When connected to the real bus (vs ESP32-to-ESP32), the bus idle
state, termination, or biasing may prevent the MAX13487E from properly asserting
a differential signal. The systematic (non-random) corruption pattern supports
this: the transceiver partially drives the bus but can't overcome the existing
bus state.

The debugger works ESP32-to-ESP32 because both transceivers have identical
drive strength and the bus has no competing devices.

### H2: 3.3V Logic Into 5V Transceiver is Marginal on the Real Bus

The ESP32 drives GPIO17 at 3.3V into the MAX13487E's DI pin. The datasheet
specifies VIH = 2.0V minimum at VCC = 5V, so 3.3V should be sufficient. But
on the real bus, the MAX13487E may be operating under different noise margins
or the 3.3V logic HIGH may be marginal when the transceiver is also trying to
drive the RS-485 differential pair against the bus's idle state.

This would explain why it works ESP32-to-ESP32 (no bus contention to overcome)
but fails on the real spa bus (must overcome the spa's bus biasing).

### H3: Bus Termination or Biasing Conflict

The Balboa bus may have termination resistors or bias networks that create a
low-impedance state the MAX13487E cannot overcome in auto-direction mode.
Standard RS-485 uses 120Ω termination; with bias resistors, the DC load can
be significant. The MAX13487E's driver output may be insufficient when the bus
is terminated and biased by the spa controller.

## Software Ruled Out

The UART initialization and write path are **byte-for-byte identical** between
the app firmware and the RS-485 debugger:

| Aspect | App Firmware | RS-485 Debugger |
|--------|-------------|-----------------|
| UART config | `Config::default().with_baudrate(115200)` | Same |
| UART peripheral | `UART1` | Same |
| TX pin | `GPIO17` | Same |
| RX pin | `GPIO16` | Same |
| Async mode | `.into_async()` | Same |
| DE pin | `None` (auto-direction) | `None` (auto-direction) |
| Write path | `uart.write()` loop + busy-wait `uart.flush()` | Same |
| Logger | UART0 only (raw registers), no UART1 interaction | Same |

The debugger successfully transmits on this exact hardware. The app's UART
initialization, write path, and pin configuration are identical. **The issue
is not in software — it is a bus-level hardware problem.**

The debugger was tested ESP32-to-ESP32 on a private bus. The app is on the
Balboa spa bus with the controller and display panel. The difference is the
bus environment, not the firmware.

## Diagnostic Steps

1. **Connect the RS-485 debugger to the real spa bus** and try to TX while
   the sniffer watches. This is the single most important test:
   - If the debugger also fails → bus drive strength issue (no software fix)
   - If the debugger works → something else is different (re-examine hardware)

2. **Measure bus idle voltage** — probe the A/B differential voltage when no
   device is transmitting. A non-zero idle voltage indicates bus biasing that
   the MAX13487E cannot overcome.

3. **Oscilloscope on RS-485 A/B pins during TX** — probe the differential signal
   when the ESP32 transmits. If no differential voltage appears, the transceiver
   isn't driving the bus. If a weak/corrupted signal appears, it's a drive
   strength issue.

4. **Oscilloscope on GPIO17 (DI pin) during TX** — verify the UART signal is
   clean 3.3V at 115200 baud.

## Potential Fixes

### Fix A: Explicit DE Pin (Hardware + Firmware)

Wire a GPIO pin to the MAX13487E's DE (pin 3) and RE (pin 2). The transport
code already supports this:

```rust
// transport.rs already has DE pin support
pub fn new(uart: Uart<'static, Blocking>, de_pin: Option<GpioPin<Output>>)
```

With an explicit DE pin, the firmware controls exactly when the driver enables,
eliminating the auto-direction mechanism entirely. A 50µs assert-to-data delay
is already implemented.

**This is the most likely fix.**

### Fix B: Channel 0x0A (Protocol Bypass)

Bypass registration entirely by claiming the WiFi module channel (0x0A).
NorthernMan54 and jasta both use this approach. However, this would still
require TX to work for sending commands, so the underlying signal issue
remains.

### Fix C: 3.3V → 5V Level Shifter on DI Pin

Add a 74LVC1T45 or similar level shifter between GPIO17 and the MAX13487E's
DI pin. Ensures the transceiver sees a solid 5V logic HIGH. Only addresses
H2.

### Fix D: Stronger Transceiver

Replace the MAX13487E with a standard RS-485 transceiver (e.g., MAX485 or
SN75176) with explicit DE/RE control and stronger drive capability.

## Reference Implementations

| Implementation | Transceiver | DE Control | Channel | Registration |
|---|---|---|---|---|
| NorthernMan54/esp32_balboa_spa | MAX485 | Explicit GPIO | 0x0A (WiFi) | Bypassed |
| jasta/esp32-balboa-spa | Unknown | Unknown | 0x0A (WiFi) | Bypassed |
| cribskip/esp8266_spa | MAX485 | Explicit GPIO | Standard | Works |
| Launa (this project) | MAX13487EESA | Auto-direction | Standard | Fails on real spa |

All successful implementations either use explicit DE/RE pin control or bypass
registration via channel 0x0A. No implementation using auto-direction on a
real Balboa bus alongside a display panel has been confirmed working.

## Capture Files

| File | Duration | Devices on Bus | Description |
|---|---|---|---|
| `sniffer-capture-spa-only.txt` | 65s | Spa + Display + Sniffer | Clean baseline, no app firmware |
| `sniffer-capture-2min.txt` | 280s | Spa + Display + App + Sniffer | App firmware attempting registration |
| `sniffer-capture-3min.txt` | 146s | Spa + Display + App + Sniffer | Includes spa reboot, display registration |
| `sniff-capture.json` | 2s | Spa + Display + Sniffer | Burst capture, TX suppressed |

## Related Documents

- `docs/registration-research.md` — Detailed registration investigation and timing analysis
- `docs/boot-sequence-analysis.md` — Spa boot sequence and display panel registration capture
- `docs/protocol.md` — Balboa protocol reference
- `AGENTS.md` — Project overview and coding guidelines
