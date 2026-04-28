# InterruptExecutor Refactor Plan

## Problem

The MQTT task freezes for 9+ minutes while the device shows "online" (retained
availability). Root cause: cooperative executor starvation. All tasks run at the
same priority on a single `ThreadModeExecutor` (started via `esp_rtos::start` using
`software_interrupt0`). When `mqtt_task` is blocked inside
`select(transport.read(), Timer::after(15s))`, the `net_task` must be polled to
process WiFi RX data and fire wakers — but cooperative scheduling means they
compete for CPU, and the 15s timer may not fire promptly.

## Solution

Move the embassy-net `Runner` and all tasks that use the network `Stack` onto a
dedicated **`InterruptExecutor`** at a higher interrupt priority. The
`InterruptExecutor` preempts the `ThreadModeExecutor`, guaranteeing `net_task`
always gets CPU time when WiFi data arrives.

### Key Constraint: `Runner` is not `Send`

`Runner<'static, Interface<'static>>` is **not `Send`** (it contains
`&RefCell<Inner>`). We cannot pass it to `SendSpawner::spawn()`. Solution:
**bootstrap task pattern**:

1. Spawn a small `Send`-safe bootstrap task on the `InterruptExecutor` via
   `SendSpawner`.
2. Inside the bootstrap task, call `embassy_net::new()` to create the
   `Stack` + `Runner`.
3. Use `Spawner::for_current_executor()` (returns a regular `Spawner`, not
   `SendSpawner`) to spawn non-`Send` tasks like `net_task` and `mqtt_task`.

## Architecture After Refactor

```
┌─────────────────────────────────────────────────────────────┐
│ InterruptExecutor (software_interrupt1, Priority::Priority1) │
│                                                             │
│  net_bootstrap ──► creates Stack+Runner, spawns tasks below │
│  net_task       ──► runner.run() (smoltcp tick)             │
│  mqtt_task      ──► MQTT publish/subscribe/reconnect loop   │
└─────────────────────────────────────────────────────────────┘
                        ↕ (preempts)
┌─────────────────────────────────────────────────────────────┐
│ ThreadModeExecutor (software_interrupt0)                     │
│                                                             │
│  connection_task ──► WiFi connection lifecycle              │
│  uart_task       ──► RS-485 UART receive loop               │
│  main event loop ──► frame processing, commands, ticks      │
└─────────────────────────────────────────────────────────────┘
```

### Cross-Executor Communication

All channels use `CriticalSectionRawMutex` which is safe for cross-executor use:

| Channel | Direction | Purpose |
|---------|-----------|---------|
| `STATE_CHANNEL` | main → mqtt | Spa status updates |
| `COMMAND_CHANNEL` | mqtt → main | Parsed MQTT commands |
| `DIAGNOSTICS_CHANNEL` | main → mqtt | Diagnostics payloads |
| `ALERT_CHANNEL` | main → mqtt | Alert payloads |
| `SNIFF_CHANNEL` | main → mqtt | Raw frame JSON |
| `OTA_CHANNEL` | mqtt → main | OTA firmware URLs |
| `PUMP_TIMER_CHANNEL` | mqtt → main | Pump timer commands |
| `WIFI_RECONNECT_SIGNAL` | connection → mqtt | Force MQTT reconnect |

## What Moves vs Stays

### Move to InterruptExecutor

- `mqtt_task` and everything in `mqtt_client.rs`
- `net_task`
- WiFi `Stack` + `Runner` creation (`embassy_net::new()`)
- All MQTT-related statics owned by `mqtt_task`

### Stay on ThreadModeExecutor

- `main` event loop (frame processing, tick timer, watchdog)
- `uart_task`
- `connection_task`
- `SpaApp` logic
- Hardware watchdog feeding
- All UART/RS-485 code
- OTA execution (needs `Stack` reference but runs on main loop)

---

## Implementation Steps

### Step 1: Add a shared `Stack` static for cross-executor access

The `Stack` reference is needed by both the `InterruptExecutor` (mqtt_task) and
the `ThreadModeExecutor` (OTA code). We expose it through a `OnceLock`-like
pattern or a `StaticCell` that gets written once.

**Add to `app/src/wifi.rs`:**

```rust
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use embassy_net::Stack;

/// Globally-shared network stack reference, written once by the InterruptExecutor
/// bootstrap task and read by the main loop (OTA code). `Stack` is `Copy` (wraps
/// `&RefCell<Inner>`), so reads after the write are safe.
static STACK_PTR: core::sync::atomic::AtomicPtr<Stack<'static>> = core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
static STACK_READY: AtomicBool = AtomicBool::new(false);

/// Store the Stack reference so other executors can access it.
/// Called exactly once from inside the InterruptExecutor.
pub(crate) fn store_stack(stack: &'static Stack<'static>) {
    STACK_PTR.store(stack as *const Stack<'static> as *mut Stack<'static>, AtomicOrdering::Release);
    STACK_READY.store(true, AtomicOrdering::Release);
}

/// Retrieve the shared Stack reference. Returns `None` if not yet initialized.
/// Safe to call from any executor after `store_stack` has been called.
pub fn get_stack() -> Option<&'static Stack<'static>> {
    if STACK_READY.load(AtomicOrdering::Acquire) {
        // SAFETY: store_stack wrote a valid &'static reference; we only
        // read it as shared (&), and Stack is internally synchronized.
        unsafe { Some(&*STACK_PTR.load(AtomicOrdering::Acquire)) }
    } else {
        None
    }
}
```

> **Alternative**: Since `Stack<'static>` is `Copy` (it's just a `&'static RefCell<Inner>`),
> we could store it in an `AtomicPtr` or simply pass it through a channel. But the
> cleanest approach is to store it as a static and have `WifiStack::connect()` await
> its availability.

Actually, simpler approach: `WifiStack::connect()` blocks until DHCP, so the
`Stack` reference is already available by the time it returns. We can just return
it from `connect()` as we do today. The key change is that `connect()` itself now
spawns the InterruptExecutor internally and the Stack is created inside the
bootstrap task. We need a way to pass the `&'static Stack` back out.

**Better approach using a signal:**

```rust
use embassy_sync::signal::Signal;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

/// Signal used by the net_bootstrap task to publish the Stack reference
/// back to the caller of WifiStack::connect().
static STACK_READY_SIGNAL: Signal<CriticalSectionRawMutex, &'static Stack<'static>> = Signal::new();
```

### Step 2: Create the `net_bootstrap` task

This task runs on the `InterruptExecutor`. It creates the embassy-net
`Stack` + `Runner`, signals the Stack back to `WifiStack::connect()`, and
spawns `net_task` and `mqtt_task` on the same executor.

**Add to `app/src/wifi.rs`:**

```rust
use embassy_executor::{InterruptExecutor, SendSpawner, Spawner};
use esp_hal::interrupt::software::SoftwareInterrupt;
use esp_hal::interrupt::Priority;

// The InterruptExecutor runs on software_interrupt1 at a higher priority
// than the ThreadModeExecutor (which uses software_interrupt0).
static NET_EXECUTOR: InterruptExecutor<1> = InterruptExecutor::new();

/// Arguments passed to net_bootstrap. All fields are Send-safe.
pub(crate) struct NetBootstrapArgs {
    pub interface: esp_radio::wifi::Interface<'static>,
    pub net_config: embassy_net::Config,
    pub seed: u64,
    pub mqtt_config: MqttConfigArgs,
}

/// MQTT configuration needed by mqtt_task. All fields are Send (String, u16, u32).
pub(crate) struct MqttConfigArgs {
    pub device_id: alloc::string::String,
    pub mqtt_host: alloc::string::String,
    pub mqtt_port: u16,
    pub mqtt_user: alloc::string::String,
    pub mqtt_password: alloc::string::String,
    pub boot_id: u32,
}

/// Bootstrap task: creates Stack+Runner, signals Stack back to connect(),
/// then spawns net_task and mqtt_task on the InterruptExecutor.
#[embassy_executor::task]
async fn net_bootstrap(args: NetBootstrapArgs) {
    let (stack, runner) = embassy_net::new(
        args.interface,
        args.net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        args.seed,
    );
    let stack_ref = mk_static!(Stack<'static>, stack);

    // Signal the Stack reference back to WifiStack::connect()
    STACK_READY_SIGNAL.signal(stack_ref);

    // Get a regular Spawner for this executor (needed for non-Send tasks)
    let spawner = Spawner::for_current_executor();

    // Spawn net_task (Runner is not Send, but we're on the same executor)
    spawner.spawn(net_task(runner)).unwrap();  // must_spawn or unwrap
    info!("InterruptExecutor: net_task spawned");

    // Connect MQTT
    let mqtt_config = crate::config::AppConfig {
        device_id: args.mqtt_config.device_id,
        wifi_ssid: alloc::string::String::new(), // not needed by MQTT
        wifi_password: alloc::string::String::new(),
        mqtt_host: args.mqtt_config.mqtt_host,
        mqtt_port: args.mqtt_config.mqtt_port,
        mqtt_user: args.mqtt_config.mqtt_user,
        mqtt_password: args.mqtt_config.mqtt_password,
    };

    let mqtt = match crate::mqtt_client::MqttClient::connect(
        stack_ref,
        &mqtt_config,
        args.mqtt_config.boot_id,
    ).await {
        Ok(m) => m,
        Err(e) => {
            error!("MQTT connect failed in net_bootstrap: {:?}, resetting", e);
            esp_hal::system::software_reset();
        }
    };

    // Post-connect publish (discovery, availability, subscriptions)
    if let Err(e) = mqtt.post_connect_publish(false).await {
        warn!("Post-connect publish failed: {:?}", e);
    }

    // Spawn mqtt_task on this executor
    spawner.spawn(crate::mqtt_task::mqtt_task(mqtt)).unwrap();
    info!("InterruptExecutor: mqtt_task spawned");
}
```

> **Note on `Spawner::for_current_executor()`**: This is available in
> embassy-executor 0.10. It returns a `Spawner` (not `SendSpawner`) that can
> spawn non-`Send` tasks on the current executor. This is the critical piece
> that allows us to spawn `net_task(runner)` where `Runner` is `!Send`.

### Step 3: Modify `WifiStack::connect()` to use the InterruptExecutor

The `connect()` function now:
1. Initializes WiFi as before (creates controller + interfaces).
2. Creates the `InterruptExecutor` from `software_interrupt1`.
3. Gets a `SendSpawner` from the executor.
4. Spawns `net_bootstrap` with `Send`-safe arguments.
5. Starts the executor (it runs in the background on interrupt).
6. Waits for `STACK_READY_SIGNAL` to get the `Stack` reference.
7. Waits for DHCP.
8. Returns `WifiStack` with the stack reference.

**Replace `WifiStack::connect()` in `app/src/wifi.rs`:**

```rust
impl WifiStack {
    /// Connect to WiFi and wait for DHCP address.
    ///
    /// Initializes WiFi, starts an InterruptExecutor at higher priority for
    /// the network stack and MQTT task, and blocks until DHCP is acquired.
    pub async fn connect(
        wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
        rng: Rng,
        ssid: &str,
        password: &str,
        hostname: &str,
        sw_interrupt1: SoftwareInterrupt<'static, 1>,
        mqtt_config: MqttConfigArgs,
        spawner: Spawner,  // ThreadModeExecutor spawner for connection_task
    ) -> Result<Self, esp_radio::wifi::WifiError> {
        let station_config = WifiConfig::Station(
            StationConfig::default()
                .with_ssid(ssid)
                .with_password(alloc::string::String::from(password)),
        );

        info!("Starting WiFi... (free heap: {} bytes)", esp_alloc::HEAP.free());
        let (controller, interfaces) = esp_radio::wifi::new(
            wifi_peripheral,
            ControllerConfig::default().with_initial_config(station_config),
        )
        .inspect_err(|e| {
            error!("WiFi init failed: {:?} (free heap: {} bytes)", e, esp_alloc::HEAP.free());
        })?;

        info!("WiFi started, connecting...");

        let wifi_interface = interfaces.station;

        // Build network config
        let mut dhcp_config = DhcpConfig::default();
        let truncated: heapless::String<32> = hostname
            .char_indices()
            .take_while(|(i, _)| *i < 32)
            .map(|(_, c)| c)
            .collect();
        if !truncated.is_empty() {
            dhcp_config.hostname = Some(truncated);
        }
        let net_config = NetConfig::dhcpv4(dhcp_config);
        let seed = ((rng.random() as u64) << 32) | (rng.random() as u64);

        // Spawn connection_task on the ThreadModeExecutor
        spawner.spawn(connection_task(controller)).map_err(|e| {
            error!("Failed to spawn connection_task: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?;

        // Start the InterruptExecutor and spawn net_bootstrap
        let send_spawner: SendSpawner = NET_EXECUTOR.start(sw_interrupt1, Priority::Priority1);
        send_spawner.spawn(net_bootstrap(NetBootstrapArgs {
            interface: wifi_interface,
            net_config,
            seed,
            mqtt_config,
        })).map_err(|e| {
            error!("Failed to spawn net_bootstrap: {:?}", e);
            esp_radio::wifi::WifiError::Failed
        })?;

        // Wait for the InterruptExecutor to create the Stack and signal it back
        let stack = STACK_READY_SIGNAL.wait().await;

        info!("Waiting for DHCP...");
        stack.wait_config_up().await;

        if let Some(config) = stack.config_v4() {
            info!("Got IP: {}", config.address);
        }

        Ok(WifiStack { stack })
    }
}
```

**Key changes:**
- `connect()` no longer takes a `Spawner` for `net_task` — the InterruptExecutor
  handles that internally via `net_bootstrap`.
- `connect()` takes `sw_interrupt1: SoftwareInterrupt<'static, 1>` for the
  InterruptExecutor.
- `connect()` takes `mqtt_config: MqttConfigArgs` to pass MQTT configuration
  to `net_bootstrap`.
- The `connection_task` is still spawned on the ThreadModeExecutor via `spawner`.

### Step 4: Update `app/src/main.rs`

#### 4a. Capture `software_interrupt1` and pass it to `init_wifi`

In `main()`, change the interrupt setup:

```rust
// BEFORE:
let sw_int = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

// AFTER:
let sw_int = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
let sw_interrupt1 = sw_int.software_interrupt1;  // hand off to InterruptExecutor
```

> **Important**: `esp_rtos::start` consumes `software_interrupt0` and starts
> the ThreadModeExecutor. The `software_interrupt1` is still available on the
> `SoftwareInterruptControl` struct. We must extract it before passing to
> `init_wifi`.

#### 4b. Build `MqttConfigArgs` before calling `init_wifi`

```rust
let mqtt_config_args = wifi::MqttConfigArgs {
    device_id: app_config.device_id.clone(),
    mqtt_host: app_config.mqtt_host.clone(),
    mqtt_port: app_config.mqtt_port,
    mqtt_user: app_config.mqtt_user.clone(),
    mqtt_password: app_config.mqtt_password.clone(),
    boot_id: boot_id(),
};
```

#### 4c. Update `init_wifi` to pass the new arguments

```rust
async fn init_wifi(
    spawner: Spawner,
    wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
    rng: esp_hal::rng::Rng,
    ssid: &str,
    password: &str,
    hostname: &str,
    sw_interrupt1: SoftwareInterrupt<'static, 1>,
    mqtt_config: wifi::MqttConfigArgs,
) -> crate::wifi::WifiStack {
    match wifi::WifiStack::connect(
        wifi_peripheral, rng, ssid, password, hostname,
        sw_interrupt1, mqtt_config, spawner,
    ).await {
        Ok(stack) => stack,
        Err(e) => {
            error!("WiFi init failed: {:?} (free heap: {} bytes), resetting in 5s",
                e, esp_alloc::HEAP.free());
            Timer::after(Duration::from_secs(5)).await;
            esp_hal::system::software_reset();
        }
    }
}
```

#### 4d. Remove MQTT connect + post_connect + crash alarm from `main()`

These lines in `main()` must be **removed** because MQTT connect now happens
inside `net_bootstrap`:

```rust
// DELETE these lines from main():
let bid = boot_id();
let mut mqtt = connect_mqtt(wifi_stack.stack, &app_config, bid).await;
let _ = mqtt.post_connect_publish(false).await;

// DELETE the crash alarm publish block (it happens inside net_bootstrap or mqtt_task)
if let Some(ref crash) = pending_crash_alarm {
    // ... entire block ...
}
```

Instead, pass `pending_crash_alarm` to `net_bootstrap` (or handle it via
`ALERT_CHANNEL` after mqtt_task starts).

**Approach**: Pass crash alarm info to `net_bootstrap` via `MqttConfigArgs`:

```rust
pub(crate) struct MqttConfigArgs {
    // ... existing fields ...
    pub pending_crash_alarm: Option<alloc::string::String>,  // JSON payload
}
```

Then in `net_bootstrap`, after spawning `mqtt_task`, send the crash alarm via
`ALERT_CHANNEL`. Actually, this doesn't work because the alarm needs to be
published by the mqtt_task itself. The cleanest approach:

**Option A**: Include crash alarm JSON in `MqttConfigArgs` and have `mqtt_task`
publish it as its first action.

**Option B**: Send the crash alarm via `ALERT_CHANNEL` after mqtt_task starts.
This requires a small delay to ensure mqtt_task is running.

**Option C** (simplest): Have `mqtt_task` check a static `Option<String>` at
startup and publish it before entering the main loop.

Go with **Option C** — add a static:

```rust
// In main.rs (or wifi.rs):
/// Crash alarm payload from previous boot, published by mqtt_task on first connect.
pub(crate) static PENDING_CRASH_ALARM: embassy_sync::channel::Channel<CriticalSectionRawMutex, alloc::string::String, 1> = embassy_sync::channel::Channel::new();
```

In `main()`, after `init_wifi()` returns (so the InterruptExecutor is running),
send the crash alarm:

```rust
if let Some(ref crash) = pending_crash_alarm {
    let alarm_json = crash_info::crash_alarm_json(crash, FIRMWARE_VERSION);
    let _ = PENDING_CRASH_ALARM.try_send(alarm_json);
}
```

Then in `mqtt_task`, at the top of the main loop, check for and publish the
pending crash alarm.

#### 4e. Remove `mqtt_task` spawning from `main()`

```rust
// DELETE:
spawner.spawn(mqtt_task::mqtt_task(mqtt).unwrap());
```

The mqtt_task is now spawned by `net_bootstrap` on the InterruptExecutor.

#### 4f. Remove the `connect_mqtt` function from `main.rs`

No longer needed — MQTT connect happens inside `net_bootstrap`.

#### 4g. Keep everything else on ThreadModeExecutor

```rust
// KEEP:
spawner.spawn(uart_task(uart_transport).unwrap());
// KEEP: main event loop
// KEEP: watchdog feeding
// KEEP: OTA handling
```

#### 4h. Update the call site

```rust
// In main(), replace:
let wifi_stack = init_wifi(
    spawner, peripherals.WIFI, esp_hal::rng::Rng::new(),
    &app_config.wifi_ssid, &app_config.wifi_password, &app_config.device_id,
).await;

// With:
let bid = boot_id();
let mqtt_config_args = wifi::MqttConfigArgs {
    device_id: app_config.device_id.clone(),
    mqtt_host: app_config.mqtt_host.clone(),
    mqtt_port: app_config.mqtt_port,
    mqtt_user: app_config.mqtt_user.clone(),
    mqtt_password: app_config.mqtt_password.clone(),
    boot_id: bid,
};
let wifi_stack = init_wifi(
    spawner, peripherals.WIFI, esp_hal::rng::Rng::new(),
    &app_config.wifi_ssid, &app_config.wifi_password, &app_config.device_id,
    sw_interrupt1, mqtt_config_args,
).await;

// After init_wifi returns, send crash alarm to mqtt_task
if let Some(ref crash) = pending_crash_alarm {
    let alarm_json = crash_info::crash_alarm_json(crash, FIRMWARE_VERSION);
    let _ = PENDING_CRASH_ALARM.try_send(alarm_json);
    info!("Crash alarm queued for MQTT publish: reason={}", crash.reason.as_str());
}
```

### Step 5: Modify `app/src/mqtt_task.rs`

#### 5a. Remove `#[embassy_executor::task]` attribute

The task is now spawned by `net_bootstrap` using a regular `Spawner`, not a
`SendSpawner`. We need to **keep** the `#[embassy_executor::task]` attribute
because that's how embassy identifies async functions that can be spawned.
However, since `mqtt_task` takes `MqttClient` which contains `TcpTransport`
(which contains `TcpSocket` which is `!Send`), the task **cannot** be spawned
via `SendSpawner`. The `#[embassy_executor::task]` macro creates a task that
is spawned via regular `Spawner::spawn()`, which is exactly what we get from
`Spawner::for_current_executor()`.

**No change needed** to the attribute — `#[embassy_executor::task]` is correct
for regular `Spawner` spawning. The important thing is we don't use
`#[embassy_executor::task(send = true)]` (which would require `Send` bounds).

#### 5b. Add pending crash alarm handling at the start of mqtt_task

At the beginning of the `mqtt_task` function, before the main loop, add:

```rust
// Publish pending crash alarm from previous boot
if let Ok(alarm_json) = crate::PENDING_CRASH_ALARM.try_receive() {
    let topics = launa_mqtt::topics::TopicBuilder::new(&mqtt.device_id);
    let alert_topic = topics.alert_topic();
    match mqtt.publish(&alert_topic, alarm_json.as_bytes(), 1, false).await {
        Ok(()) => info!("Crash alarm published from mqtt_task"),
        Err(e) => warn!("Failed to publish crash alarm: {:?}", e),
    }
}
```

### Step 6: Modify `app/src/mqtt_client.rs` — explicit yield in `recv()`

The `recv()` method already has an explicit yield (`Timer::after(Duration::from_millis(1))`)
before each socket read. This was added as a workaround for cooperative executor
starvation. With the InterruptExecutor, this yield is no longer strictly necessary
(since `net_task` on the InterruptExecutor preempts the ThreadModeExecutor), but
it's still good practice to yield periodically.

**No change needed** — the existing yield is harmless and may still help on the
InterruptExecutor if mqtt_task and net_task compete there.

### Step 7: Add imports for `InterruptExecutor` and related types

In `app/src/wifi.rs`, add:

```rust
use embassy_executor::InterruptExecutor;
use esp_hal::interrupt::software::SoftwareInterrupt;
use esp_hal::interrupt::Priority;
```

In `app/Cargo.toml`, verify that `embassy-executor` version supports
`InterruptExecutor` and `Spawner::for_current_executor()`:

- `embassy-executor = "0.10"` — `InterruptExecutor` has been available since
  embassy-executor 0.5+. `Spawner::for_current_executor()` was added in
  embassy-executor 0.6+. Version 0.10 supports both. **No Cargo.toml change needed.**

---

## Detailed File-by-File Changes

### File: `app/src/wifi.rs`

1. **Add imports:**
   ```rust
   use embassy_executor::{InterruptExecutor, SendSpawner, Spawner};
   use embassy_sync::signal::Signal;
   use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
   use esp_hal::interrupt::software::SoftwareInterrupt;
   use esp_hal::interrupt::Priority;
   ```

2. **Add statics and structs:**
   ```rust
   static NET_EXECUTOR: InterruptExecutor<1> = InterruptExecutor::new();
   static STACK_READY_SIGNAL: Signal<CriticalSectionRawMutex, &'static Stack<'static>> = Signal::new();

   pub(crate) struct MqttConfigArgs { ... }
   pub(crate) struct NetBootstrapArgs { ... }
   ```

3. **Add `net_bootstrap` task** (see Step 2 above).

4. **Replace `WifiStack::connect()`** (see Step 3 above).

5. **Keep `connection_task` and `net_task` unchanged** — they are still
   `#[embassy_executor::task]` functions, just spawned by different executors.

6. **Remove `net_task`'s `#[embassy_executor::task]`** — Actually, keep it.
   The task is still spawned via `Spawner::spawn()`, and the macro generates
   the necessary wrapper. **No change needed** to `net_task`.

### File: `app/src/main.rs`

1. **Extract `software_interrupt1`** from `SoftwareInterruptControl` (Step 4a).

2. **Add `PENDING_CRASH_ALARM` static** (Step 4c).

3. **Build `MqttConfigArgs` before `init_wifi`** (Step 4b).

4. **Update `init_wifi` signature and body** (Step 4c).

5. **Remove `connect_mqtt` function** entirely.

6. **Remove MQTT connect + post_connect + crash alarm publish** from main (Step 4d).

7. **Remove `spawner.spawn(mqtt_task::mqtt_task(mqtt))`** (Step 4e).

8. **Send crash alarm via `PENDING_CRASH_ALARM`** after `init_wifi` returns (Step 4g).

9. **Add import** for `SoftwareInterrupt`:
   ```rust
   use esp_hal::interrupt::software::SoftwareInterrupt;
   ```

### File: `app/src/mqtt_task.rs`

1. **Add crash alarm publish** at the start of the function (Step 5b).

### File: `app/src/mqtt_client.rs`

No changes needed. The existing yield and timeout logic works correctly on
the InterruptExecutor.

---

## Risks and Considerations

### Stack Reference Safety

The `Stack` reference is used by both `mqtt_task` (InterruptExecutor) and OTA
code (ThreadModeExecutor). `Stack<'static>` is `Copy` (wraps `&'static RefCell<Inner>`).
Accessing it from two executors at different priorities is safe because
embassy-net internally uses critical sections for all mutations. The `RefCell`
is only borrowed by `Runner::run()` on the InterruptExecutor, and external
users (like OTA TCP connections) go through `Stack` methods that use interior
mutability with proper synchronization.

### Cross-Executor Signals

- `WIFI_RECONNECT_SIGNAL`: Written by `connection_task` (ThreadModeExecutor),
  read by `mqtt_task` (InterruptExecutor). Uses `CriticalSectionRawMutex` — safe.
- All channels (`STATE_CHANNEL`, `COMMAND_CHANNEL`, etc.): Use
  `CriticalSectionRawMutex` — safe for cross-executor use.

### Interrupt Priority

- `ThreadModeExecutor` runs on `software_interrupt0` at the default priority.
- `InterruptExecutor` runs on `software_interrupt1` at `Priority::Priority1`
  (higher than default).
- This ensures the `InterruptExecutor` (net_task, mqtt_task) preempts the
  `ThreadModeExecutor` (uart_task, connection_task, main loop) whenever it
  has work to do.

### OTA Code

The OTA code runs on the ThreadModeExecutor's main loop but needs the `Stack`
reference. It currently gets this from `wifi_stack.stack`. This still works
because `WifiStack::connect()` returns the `WifiStack` with the stack reference,
and the reference remains valid for the entire `'static` lifetime.

### Connection Task

`connection_task` stays on the ThreadModeExecutor. It uses `WifiController`
which is `Send`. The controller's `is_connected()` and `rssi()` methods don't
need the network stack — they interact with the WiFi driver directly. The
`WIFI_RECONNECT_SIGNAL` it fires is safe for cross-executor use.

### MQTT Task Watchdog

The existing `MQTT_TASK_TICK` atomic counter continues to work unchanged.
The main loop reads it and detects if mqtt_task is frozen. With the
InterruptExecutor, the mqtt_task is much less likely to freeze, but the
watchdog provides an additional safety net.

### Boot Validation (Firmware Marking)

The `validate_firmware()` call in `main()` currently happens after MQTT connect.
With this refactor, MQTT connect happens inside `net_bootstrap` on the
InterruptExecutor. We should call `validate_firmware()` in `main()` after
`init_wifi()` returns (which implies MQTT is connected). The timing works
because `init_wifi()` blocks until DHCP + MQTT connect + post_connect_publish
are all done.

---

## Implementation Order

1. **wifi.rs**: Add `MqttConfigArgs`, `NetBootstrapArgs`, `NET_EXECUTOR`,
   `STACK_READY_SIGNAL`, and `net_bootstrap` task.
2. **wifi.rs**: Rewrite `WifiStack::connect()` to use InterruptExecutor.
3. **main.rs**: Add `PENDING_CRASH_ALARM` static, update `init_wifi`,
   remove `connect_mqtt`, remove MQTT connect from main, pass crash alarm
   via channel.
4. **mqtt_task.rs**: Add crash alarm publish at startup.
5. **Verify build**: `cd app && cargo check` (with appropriate target).
6. **Run workspace tests**: `cargo test` (workspace crates, no ESP32 needed).
7. **Flash and test**: Verify MQTT connectivity, state publishing, command
   reception, OTA, and the absence of MQTT task freezes.

---

## Notes for the Implementer

- The `esp_rtos::start()` call consumes `software_interrupt0` and never returns.
  It starts the ThreadModeExecutor. The `main()` async function runs as the
  first task on this executor.
- `NET_EXECUTOR.start(sw_interrupt1, priority)` starts the InterruptExecutor
  on `software_interrupt1`. It returns a `SendSpawner` that can spawn tasks
  with `Send` bounds.
- `Spawner::for_current_executor()` can only be called from within a task
  running on the executor. It returns a regular `Spawner` without `Send` bounds.
- The `InterruptExecutor<1>` type parameter refers to the number of interrupt
  priorities it supports (not the interrupt number). Check the
  embassy-executor 0.10 API for the exact type parameters.
- The `connection_task` needs the `Spawner` from the ThreadModeExecutor to be
  spawned. Since `main()` runs on the ThreadModeExecutor, we still have access
  to its `Spawner` for spawning `connection_task` and `uart_task`.
