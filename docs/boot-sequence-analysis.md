# Boot Sequence & Registration Analysis

Analysis of a 3-minute RS-485 bus capture from a Balboa BP6013G1 spa controller.
Captured with two ESP32 devices on the bus: one running `app/` (main firmware,
should attempt registration) and one running `app-sniffer/` (passive receive-only
monitor). The spa was rebooted approximately 40 seconds into the capture.

Capture date: 2026-05-04
Source: `docs/sniffer-capture-3min.txt`

---

## Capture Summary

| Metric | Value |
|--------|-------|
| Duration | 146 seconds (~2.4 min) |
| Total frames | 14,887 |
| Reboot at | ~t=33.7s (1.02s gap in traffic) |

### Frame Counts

| Frame Type | Count | Description |
|---|---|---|
| `10BF [06]` CTS | 7,018 | Spa polls channel 0x10 |
| `10BF [110000]` Ready | 7,020 | Display responds to CTS |
| `FFAF [13]` Status | 523 | Status broadcasts (~3.3 Hz) |
| `FEBF [00]` NewClientQuery | 262 | Registration polls |
| `FEBF [01]` NewClientResponse | 1 | Display panel response |
| `FEBF [02]` ClientIdAssignment | 1 | Spa assigns channel |
| `10BF [03]` ClientIdAck | 1 | Display acknowledges |
| `XX BF [04]` ConfigReq (scan) | 51 | Spa boot enumeration |
| `FFAF [12/14/2B]` | 7 | Boot initialization broadcasts |

---

## Pre-Boot Steady State (t=0 to ~33.7s)

The spa is running normally with the display panel registered on channel 0x10.
Normal traffic pattern:

| Frame | Rate | Period |
|-------|------|--------|
| CTS / Ready cycle | ~52 Hz | 19ms |
| Status broadcast | ~3.3 Hz | 300ms |
| NewClientQuery | ~1 Hz | ~970ms |

No registration responses from any device. The spa queries for new clients
once per second, but nothing answers (the display is already registered).

---

## Boot Sequence (t=33.7s onward)

The reboot triggers a clear 6-phase sequence:

### Phase 1: Bus Enumeration (t=33.74s, duration 177ms)

The spa sends `XX BF 04` (ConfigReq) to channels **0x0A through 0x52** at ~3.5ms
intervals, probing for any existing clients. Only channel 0x10 (the display)
responds.

Channels probed: 0x0A, 0x0C, 0x0F, 0x10, 0x11-0x3F, 0x52

### Phase 2: Initialization Broadcasts (t=33.93s - 34.29s, duration 360ms)

Seven non-status FFAF frames broadcast the spa's configuration to all clients:

```
t=33.93s  FFAF [2B 00]                    — Config marker
t=34.03s  FFAF [12 03 03 04 91 00 ...]    — Init data (pumps/config)
t=34.05s  FFAF [14 03 28 04 11 00 ...]    — Init data (filter cycles)
t=34.07s  FFAF [12 04 00 04 11 1B 3D ...] — Init data
t=34.09s  FFAF [14 04 04 00 00 11 00 ...] — Init data
t=34.27s  FFAF [12 01 04 00 1B 01 91 ...] — Final init (config signature)
t=34.29s  FFAF [14 01 04 00 11 28 00 ...] — Final init
```

### Phase 3: Display Panel Registration (t=34.75s)

```
34748.7ms  FEBF [00]           — Spa: "Any new clients?"
34749.4ms  FEBF [01 01 1D 70]  — Display panel: "Yes! type=1, hash=0x1D70"
34768.3ms  FEBF [02 10 1D 70]  — Spa: "Assigned channel 0x10"
34768.6ms  10BF [03]           — Display: ACK channel 0x10
```

Critical timing:
- **Query to Response: 0.7ms** — the display responds almost instantly
- **Response to Assignment: 18.9ms** — spa processes and assigns
- **Assignment to ACK: 0.3ms** — display acknowledges immediately
- Display uses device hash **0x1D70**, gets assigned channel **0x10**

### Phase 4: Post-Registration Config Exchange (t=34.78s - 34.85s)

Two config frames relayed through the CTS mechanism to the newly registered display:

```
10BF [12 01 04 00 1B 01 91 02 ...]  — Config data
10BF [14 01 04 00 11 28 00 00 ...]  — Config data
```

### Phase 5: First Status (t=34.89s)

First `FFAF [13]` status frame arrives with:
- state=0x13, init_mode=0x01, temp=4, set=2
- This is a post-boot initialization state

### Phase 6: Rapid Re-Registration (t=34.9s - ~45s)

After the initial registration, the spa aggressively polls for additional new
clients. NewClientQuery frames arrive every **80-160ms** (vs normal ~970ms),
gradually settling back to the normal rate over ~10 seconds.

The FEBF period histogram shows this clearly:

| Period Range | Count | Percentage |
|---|---|---|
| 0-100ms | 70 | 26.8% |
| 100-200ms | 49 | 18.8% |
| 200-500ms | 1 | 0.4% |
| 500-1100ms (normal ~1s) | 125 | 47.9% |
| 1.1-3s | 16 | 6.1% |

---

## Key Finding: Our ESP32 Did Not Respond

The main ESP32 (running `app/` firmware) was connected to the same RS-485 bus
during this entire capture. It should have been attempting to register in
response to NewClientQuery frames.

**Evidence that our ESP32 did not transmit:**

1. **No `FEBF [01]` with our hash.** The only NewClientResponse in the entire
   capture uses hash 0x1D70 (the display panel). Our firmware uses 0xE356 or
   0xF173.

2. **No frames from any unrecognized channel.** The only non-spa, non-display
   frames are the boot enumeration scan (spa TX, not ours).

3. **No ClientIdAssignment for any new channel.** Only channel 0x10 is assigned,
   to the display.

4. **262 unanswered NewClientQueries.** The spa polled for new clients 262 times
   over 146 seconds. Our ESP32 responded to zero of them.

This confirms the hypothesis from `registration-research.md`: the ESP32's UART
TX writes complete successfully (no error), but the signal does not reach the
RS-485 bus. The MAX13487E auto-direction transceiver may not be switching to
TX mode, or the bus impedance/termination may prevent our signal from being
detectable by the spa controller.

---

## Implications for Registration Implementation

### What We Learned

1. **The display responds in 0.7ms.** This is extremely fast. Our response
   must arrive within the ~19ms gap between FEBF query and the next CTS frame.

2. **The display uses hash 0x1D70, device type 0x01.** This is the real display
   panel's hash. Different device types may use different values.

3. **Channel 0x10 is the display's channel.** We must register on a different
   channel. NorthernMan54's approach of claiming channel 0x0A (WiFi module)
   may be viable.

4. **The spa queries aggressively post-boot** (~80-160ms intervals for ~10s),
   giving multiple registration opportunities during the boot window.

5. **The total boot sequence takes ~1.3s** from enumeration to first status,
   with registration occurring approximately 1 second into the boot.

### Required Next Steps

1. **Verify TX reaches the bus.** Connect the sniffer ESP32 to watch for our
   registration response frame while the main ESP32 attempts to transmit. This
   is the single most important diagnostic.

2. **If TX is confirmed working**, investigate timing. We need sub-millisecond
   response latency to match the display panel's 0.7ms response time.

3. **If TX is not reaching the bus**, the issue is hardware. Either:
   - The MAX13487E auto-direction is not detecting our UART TX
   - Bus termination/impedance is preventing our signal
   - Our RS-485 port is on a different bus segment from the display
