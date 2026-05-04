# RS-485 Registration Research

Status of Balboa BP6013G1 client registration investigation. Registration
works with the spa-emulator (no other bus devices) but **never succeeds on
the real spa controller**.

## Problem Statement

The ESP32 receives spa bus traffic (status, queries, CTS frames) correctly but
the spa never responds with a `ClientIdAssignment` (`FE BF 02`) after we send
a `NewClientResponse` (`FE BF 01 02 <hash>`).

## Hardware Setup

- **Controller**: Balboa BP6013G1
- **Transceiver**: MAX13487EESA (auto-direction, no DE pin)
- **Verified working**: RS-485 debugger firmware successfully communicates
  between two ESP32s, each with their own MAX13487E transceiver, connected
  to the same bus. Both TX and RX work correctly on both devices.
- **UART**: ESP32 UART1, 115200 baud 8N1, GPIO17 (TX) / GPIO16 (RX)
- **Display panel**: connected to the same RS-485 bus on channel 0x10

## Bus Traffic Pattern

Measured via ESP32 microsecond timestamps (burst sniffer capture):

```
 Spa  → FFAF status     every ~300ms (25 bytes, ~2.2ms on wire)
 Spa  → FEBF query      every ~1s    (7 bytes, ~0.6ms on wire)
 Spa  → 10BF CTS        every ~19ms  (3 bytes, ~0.26ms on wire)
 Display → 10BF Ready   every ~19ms  (5 bytes, ~0.43ms on wire)
```

The display panel creates a constant stream of `10BF` frames. Typical cycle:

```
 0.0ms   Spa  → 10BF [06]     (CTS for ch 0x10)
 1.1ms   Disp → 10BF [110000] (Ready response)
19.9ms   Spa  → 10BF [06]     (next CTS)
21.0ms   Disp → 10BF [110000] (next Ready)
 ...
```

When FEBF query arrives, it replaces one CTS in the cycle:

```
551.5ms  Spa  → FEBF [00]     (NewClientQuery)
562.0ms  Spa  → 10BF [06]     (next CTS, +10.4ms gap)
563.0ms  Disp → 10BF [110000]
```

The 10.4ms gap after FEBF is the only clear window for our response.

## Registration Protocol

From `docs/protocol.md`:

```
1. Spa  → FE BF 00               (NewClientQuery)
2. Client → FE BF 01 02 <hash>   (NewClientResponse)
3. Spa  → FE BF 02 <channel>     (ClientIdAssignment)
4. Client → <channel> BF 03       (ClientIdAck)
```

Our response frame (hash=E356):

```
7E 08 FE BF 01 02 E3 56 3F 7E    (10 bytes, CRC-8 verified correct)
```

## What Was Tested

### 1. Frame Format — Verified Correct

- CRC-8 independently verified with Python implementation
- Byte sequence `7E 08 FE BF 01 02 E3 56 3F 7E` matches protocol spec
- Also tested hash `F173` (universal hash used by other implementations) — no difference

### 2. Timing Variations — All Failed

Tested 12 combinations via MQTT-tunable parameters:

| delay_ms | immediate | result |
|----------|-----------|--------|
| 0        | true      | fail   |
| 1        | true      | fail   |
| 2        | true      | fail   |
| 5        | true      | fail   |
| 10       | true      | fail   |
| 0        | false     | fail   |
| 1        | false     | fail   |
| 2        | false     | fail   |
| 5        | false     | fail   |
| 10       | false     | fail   |
| 20       | false     | fail   |
| 50       | false     | fail   |

### 3. Async Fast-Path (channel bypass) — Failed

Added a frame-level fast-path in `uart_task` that sends the response
directly via `transport.write()` when the decoder produces a `FE BF 00`
frame, bypassing the async channel pipeline. Response latency: ~5ms.
Still no spa response.

```rust
// In uart_task, inside the byte-by-byte decode loop:
if frame.message_type == [0xFE, 0xBF]
    && frame.payload.len() == 1
    && frame.payload[0] == 0x00
{
    if let Some(ref resp) = reg_response_bytes {
        let _ = transport.write_sync(resp);
    }
}
```

### 4. Sync Polling Loop (zero async overhead) — Failed

Rewrote `uart_task` as a sync polling loop using non-blocking
`uart.read()` / `uart.write()` / `uart.flush()` directly, eliminating
all embassy async overhead. Response latency: sub-millisecond.
Still no spa response.

### 5. ExistingClientRequest (channel 0x0A) — Failed

Tried `ExistingClientRequest` (`FE BF 04 0A E3 56`) to claim the WiFi
module channel. Also tried channel 0x10 (display channel). Both ignored.

### 6. Half-Duplex Idle-Gap TX — Response Never Sent

Implemented proper half-duplex protocol: only transmit after confirming
bus idle (no RX data for 87µs). **The bus is never idle** — the display
panel's 10BF frames arrive every 5-15ms. The response is queued but
never transmitted.

```rust
loop {
    // Step 1: RECEIVE
    match transport.read_sync(&mut buf) {
        Ok(n) if n > 0 => { /* decode frames, continue */ }
        Ok(_) => {} // no data, proceed to idle check
    }
    if bus_active { continue; }

    // Step 2: IDLE CHECK
    esp_hal::rom::ets_delay_us(87);
    match transport.read_sync(&mut buf) {
        Ok(n) if n > 0 => { continue; } // bus still active
        _ => {} // bus is idle
    }

    // Step 3: TRANSMIT (never reached — bus never idles)
    if let Some(resp) = pending_response.take() {
        transport.write_sync(&resp);
    }
}
```

### 7. Immediate TX After Decode (current approach) — Fails

Transmit the registration response after the byte-by-byte decode loop
completes (when `pending_reg_response` is set by FEBF decode). The sniff
data confirms a 10ms gap after FEBF. `read_sync` batches complete before
the next frame starts. FIFO is empty. Response IS written to UART.
But the spa still doesn't respond.

```rust
// After the for &byte in &buf[..n] loop:
if pending_reg_response {
    if let Some(ref resp) = reg_response_bytes {
        let _ = transport.write_sync(resp);
    }
    pending_reg_response = false;
}
```

### Sniffer Tool

Burst capture sniffer implemented. Captures up to 200 frames over 2s with
microsecond-precision timestamps. Publishes JSON to `launa/<device>/sniff`.

Trigger: `mosquitto_pub -t 'launa/launa_spa/command/sniff' -m 'true'`

Output format:
```json
{
  "capture_us": 2001572,
  "frame_count": 195,
  "frames": [
    [461, "10BF", "110000"],
    [551529, "FEBF", "00"],
    ...
  ]
}
```

## Root Cause Hypothesis

**Unknown.** The response IS being written to the UART with correct timing
(~10ms gap after FEBF query, verified by burst sniffer). Frame format is
correct (CRC verified). All timing and method variations have been exhausted.

Possible remaining causes:

1. **The ESP32's UART TX doesn't actually drive the RS-485 bus when
   connected to the spa.** The transceivers are verified working between
   two ESP32s. But the spa's RS-485 bus may have different impedance,
   termination, or voltage levels that prevent the MAX13487E from
   driving the line. Needs verification with the debugger connected
   to the SPA's bus (not just ESP32-to-ESP32).

2. **The spa's RS-485 port is a different bus segment from the display.**
   If the spa has separate bus segments, our device may be on a read-only
   diagnostic port that doesn't accept incoming registration responses.

3. **The BP6013G1 firmware has a registration lock or bug.** The spa
   may have a maximum number of registered clients, or registration may
   require a specific configuration mode.

4. **The auto-direction transceiver TX detection is too slow.** The
   MAX13487E detects TX data on its DI pin to switch to TX mode. If the
   detection threshold or timing doesn't match the ESP32's UART idle
   characteristics, the transceiver may stay in RX mode.

## Key Code Paths

### uart_task (app/src/main.rs)

The main UART loop. Sync polling with frame decoding:

```rust
loop {
    match transport.read_sync(&mut buf) {
        Ok(n) if n > 0 => {
            for &byte in &buf[..n] {
                if let Some(frame) = decoder.feed(byte) {
                    // Immediate TX for NewClientQuery
                    if frame.message_type == [0xFE, 0xBF]
                        && frame.payload.len() >= 1
                        && frame.payload[0] == 0x00
                    {
                        if let Some(ref resp) = reg_response_bytes {
                            let _ = transport.write_sync(resp);
                        }
                    }
                    frame_sender.send(frame).await;
                }
            }
        }
        // ... idle check and transmit for other frames
    }
}
```

### write_sync (app/src/transport.rs)

Blocking write to UART TX FIFO + flush:

```rust
pub fn write_sync(&mut self, data: &[u8]) -> Result<(), TransportError> {
    let mut written = 0;
    while written < data.len() {
        let n = self.uart.write(&data[written..])
            .map_err(|_| TransportError::Io)?;
        written += n;
    }
    while self.uart.flush().is_err() {} // busy-wait for shift register
    Ok(())
}
```

### Registration state machine (crates/launa-protocol/src/registration.rs)

Parses registration messages and tracks state:

```rust
pub enum RegistrationMessage {
    NewClientQuery,
    NewClientResponse { device_type: u8, client_hash: [u8; 2] },
    ClientIdAssignment { channel: u8, client_hash: [u8; 2] },
    ClientIdAck { channel: u8 },
    ExistingClientRequest { channel: u8, client_hash: [u8; 2] },
    ExistingClientResponse { channel: u8, client_hash: [u8; 2] },
    ClearToSend { channel: u8 },
}
```

### SpaApp registration handler (crates/launa-core/src/spa_app.rs)

SpaApp's `SendNewClientResponse` is a no-op since the sync fast-path in
uart_task handles it:

```rust
RegistrationAction::SendNewClientResponse => {
    // The sync fast-path in uart_task sends the NewClientResponse
    // directly when it sees FE BF 00 — no action needed here.
    self.registration_started_at = Some(now);
}
```

## Open Questions

1. **Does our TX actually appear on the RS-485 bus?** This is the #1
   unknown. We need a second ESP32 with the RS-485 debugger firmware
   connected to the same bus to verify. Or an oscilloscope/logic analyzer
   on the bus wires.

2. **Is the spa's RS-485 expansion port full-duplex?** Some Balboa
   controllers use separate TX/RX pairs on the expansion connector.
   If so, our TX may go to a different bus segment than the display.

3. **Do other RS-485 implementations work alongside a display panel?**
   We need to find reports of successful registration on a BP6013G1
   that has an existing topside display connected.

4. **Could the MAX13487E auto-direction be the problem?** The transceiver
   uses edge detection on the TX data line to switch modes. If the
   ESP32's UART idle state (high) doesn't create a detectable edge,
   the transceiver may never switch to TX. An explicit DE pin would
   resolve this but requires hardware modification.

5. **Should we try the WiFi module approach (0x0A)?** NorthernMan54's
   implementation skips registration entirely and claims channel 0x0A
   (the WiFi module channel). This works because the spa expects a
   WiFi module and assigns it a fixed channel.

## Burst Sniffer Measurements

Burst capture from real spa (BP6013G1 with display panel connected), captured
via MQTT sniff command. 197 frames over 2012.8 ms with microsecond timestamps.

Full capture data: [docs/sniff-capture.json](sniff-capture.json)

### Capture Summary

| Metric | Value |
|--------|-------|
| Capture duration | 2012795 µs (2012.8 ms) |
| Frame count | 197 |
| Frame rate | 97.9 frames/sec |
| Min inter-frame gap | 60 µs (0.060 ms) |
| Max inter-frame gap | 30851 µs (30.851 ms) |
| Average inter-frame gap | 10267 µs (10.267 ms) |
| Median inter-frame gap | 9894 µs (9.894 ms) |

### Frame Types Observed

| Type | Count | Description |
|------|-------|-------------|
| 10BF | 189 | CTS (06) + Ready (110000) handshake |
| FFAF | 6 | Status broadcast (~300ms period) |
| FEBF [00] | 2 | NewClientQuery (~1s period) |

### Gap Distribution

```
  0.0 - 0.5 ms:    4 (  2.1%)  ← CTS/Ready response pairs
  0.5 - 1.0 ms:    5 (  2.6%)  ← CTS/Ready response pairs
  1.0 - 2.0 ms:   76 ( 39.2%)  ← CTS/Ready response pairs (typical)
  2.0 - 5.0 ms:    7 (  3.6%)  ← Occasionally wider CTS/Ready gaps
  5.0 - 10.0 ms:   5 (  2.6%)  ← After FFAF status frames
 10.0 - 15.0 ms:   7 (  3.6%)  ← After FEBF query (TX window!)
 15.0 - 20.0 ms:  79 ( 40.7%)  ← CTS-to-CTS cycle period (dominant)
 20.0 - 50.0 ms:  11 (  5.7%)  ← Gaps involving FFAF or FEBF
```

### CTS/Ready Cycle Timing

The dominant traffic pattern is the CTS/Ready handshake at ~19ms intervals:

```
| Metric                        | Value                    |
|-------------------------------|--------------------------|
| CTS → Ready gap               | 0.7–1.5 ms (avg ~1.1 ms) |
| Ready → next CTS gap (idle)   | 17–19 ms (avg ~18.8 ms)  |
| CTS → CTS period (full cycle) | 18.8–20.1 ms (avg ~19 ms)|
| CTS cycle frequency           | ~52.6 Hz                 |
```

The bus is "idle" (no data on wire) for ~18.8 ms between display Ready
response and the next CTS. However, the auto-direction transceiver makes
it risky to transmit during these idle gaps — the display panel may start
transmitting its Ready response at any time if the spa sends a CTS.

### FEBF NewClientQuery Timing — CRITICAL

Two FEBF queries were captured, at 659.8 ms and 1661.0 ms into the capture.
This gives a FEBF period of **~1001 ms** (approximately once per second).

#### FEBF #1 — at 659.773 ms

```
>>> [66]   659,773 µs    FEBF [00]      (NewClientQuery, +25.987ms from prev frame)
  [67]   672,758 µs    10BF [06]      (CTS, +12.985ms)  ← TX WINDOW
  [68]   674,386 µs    10BF [110000]  (Ready, +1.628ms)
  [69]   692,670 µs    10BF [06]      (CTS, +18.284ms)
```

**Gap after FEBF: 12.985 ms** before the next CTS frame. This is our
TX window for the NewClientResponse.

#### FEBF #2 — at 1660.985 ms

```
  [161] 1,663,725 µs  10BF [06]      (CTS)
>>> [162] 1,660,985 µs  FEBF [00]      (NewClientQuery, +27.102ms from prev CTS)
  [163] 1,672,771 µs  10BF [06]      (CTS, +11.786ms)  ← TX WINDOW
  [164] 1,676,654 µs  10BF [110000]  (Ready, +3.883ms)
  [165] 1,692,678 µs  10BF [06]      (CTS, +16.024ms)
```

**Gap after FEBF: 11.786 ms** before the next CTS frame.

### FEBF Replaces a CTS Frame

The FEBF query does NOT appear in addition to the regular CTS cycle — it
**replaces** one CTS frame. Evidence:

- Normal CTS-to-CTS period: ~19 ms
- Gap from last CTS before FEBF#1 to FEBF#1: 27.96 ms (~1.5× normal)
- Gap from FEBF#1 to next CTS: 11.05 ms (~0.6× normal)
- Sum: 27.96 + 11.05 = 39.01 ms ≈ 2 × normal CTS period

The spa skips one CTS cycle when it sends a FEBF query. The FEBF takes
the place of the CTS that would have been sent at ~24 ms, and the next
CTS comes at ~45 ms (one full cycle after the skipped CTS would have been).

### FFAF Status Frame Timing

Status frames arrive every ~300 ms (avg 299.8 ms, range 298.0–300.9 ms).
Each status frame is ~25 bytes (~2.2 ms on wire at 115200 baud). The CTS
cycle continues uninterrupted through status broadcasts — the CTS appears
~7–10 ms after the status frame ends.

### Potential TX Windows

Gaps > 1 ms where the bus is idle and a response could potentially be
transmitted:

| Window | Gap | Duration | Risk |
|--------|-----|----------|------|
| After FEBF query | FEBF → next CTS | 11–13 ms | **Best** — spa expects response here |
| After FFAF status | FFAF → next CTS | 7–10 ms | Moderate — spa not listening for response |
| CTS cycle idle | Ready → next CTS | 17–19 ms | Risky — spa may CTS at any time |

### Key Observations

1. **11–13 ms TX window after FEBF is confirmed.** The gap is consistent and
   much larger than any normal inter-frame gap. A 10-byte response frame
   takes ~0.87 ms at 115200 baud, so there is ample time.

2. **FEBF appears every ~1 second**, replacing one CTS frame. There are
   ~52 CTS cycles between each FEBF query.

3. **The bus is never truly idle for long.** The 19 ms CTS cycle means
   the spa transmits every ~19 ms, and the display responds within ~1 ms.
   The only extended idle period is the 11–13 ms gap after FEBF.

4. **No ClientIdAssignment (FE BF 02) was observed** in the capture.
   This confirms the spa is not responding to our registration attempts.

### Raw Frame Data

Full capture data (197 frames with microsecond timestamps): [docs/sniff-capture.json](sniff-capture.json)

## References

- `docs/protocol.md` — Balboa protocol reference
- `crates/launa-protocol/src/registration.rs` — registration state machine
- `app/src/main.rs` — uart_task (sync polling loop)
- `app/src/transport.rs` — Rs485Transport (sync read/write)
- `crates/launa-core/src/spa_app.rs` — SpaApp registration handling
- https://github.com/jasta/esp32-balboa-spa — Rust implementation (calls
  FE BF 00 "NewClientClearToSend", has 20ms CTS window)
- https://github.com/NorthernMan54/esp32_balboa_spa — ESP32 implementation
  that bypasses registration (pretends to be WiFi module 0x0A)
