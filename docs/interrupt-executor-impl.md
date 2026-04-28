# InterruptExecutor Implementation Guide

## API Reference (verified against esp-rtos 0.3.0, embassy-executor 0.10)

```rust
// esp-rtos 0.3.0 InterruptExecutor API:
let executor = esp_rtos::embassy::InterruptExecutor::<SWI>::new(sw_interrupt);
// SWI is the const generic: 0, 1, or 2
let send_spawner: embassy_executor::SendSpawner = executor.start(esp_hal::interrupt::Priority::Priority1);
// start() takes ONLY the priority, not the interrupt

// embassy-executor 0.10 Spawner API (from inside an InterruptExecutor task):
let spawner: embassy_executor::Spawner = embassy_executor::Spawner::for_current_executor().await;
// This returns Spawner (not SendSpawner), can spawn non-Send tasks

// InterruptExecutor has UnsafeCell internals — cannot be a plain static.
// Must use mk_static! or static_cell:
let executor = mk_static!(esp_rtos::embassy::InterruptExecutor<1>, esp_rtos::embassy::InterruptExecutor::new(sw_int1));
```

## Architecture

```
InterruptExecutor (SWI 1, Priority1 — preempts ThreadModeExecutor)
├── net_bootstrap — creates Stack+Runner, spawns net_task + mqtt_task
├── net_task — runner.run() (smoltcp poll)
└── mqtt_task — MQTT publish/subscribe/reconnect

ThreadModeExecutor (SWI 0, started by esp_rtos::start)
├── connection_task — WiFi lifecycle
├── uart_task — RS-485 receive
└── main loop — frame processing, commands, watchdog
```

## Files to Change

### 1. app/src/wifi.rs

Add these at module level:

```rust
use embassy_executor::{SendSpawner, Spawner};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_hal::interrupt::software::SoftwareInterrupt;

static STACK_READY_SIGNAL: Signal<CriticalSectionRawMutex, &'static Stack<'static>> = Signal::new();

pub(crate) struct MqttConfigArgs {
    pub device_id: alloc::string::String,
    pub mqtt_host: alloc::string::String,
    pub mqtt_port: u16,
    pub mqtt_user: alloc::string::String,
    pub mqtt_password: alloc::string::String,
    pub boot_id: u32,
}

struct NetBootstrapArgs {
    interface: esp_radio::wifi::Interface<'static>,
    net_config: embassy_net::Config,
    seed: u64,
    mqtt_config: MqttConfigArgs,
}

#[embassy_executor::task]
async fn net_bootstrap(args: NetBootstrapArgs) {
    let (stack, runner) = embassy_net::new(
        args.interface,
        args.net_config,
        mk_static!(StackResources<4>, StackResources::<4>::new()),
        args.seed,
    );
    let stack_ref = mk_static!(Stack<'static>, stack);

    STACK_READY_SIGNAL.signal(stack_ref);

    let spawner = Spawner::for_current_executor().await;
    spawner.must_spawn(net_task(runner));

    // Connect MQTT inside the InterruptExecutor context
    let mqtt_config = crate::config::AppConfig {
        device_id: args.mqtt_config.device_id,
        wifi_ssid: alloc::string::String::new(),
        wifi_password: alloc::string::String::new(),
        mqtt_host: args.mqtt_config.mqtt_host,
        mqtt_port: args.mqtt_config.mqtt_port,
        mqtt_user: args.mqtt_config.mqtt_user,
        mqtt_password: args.mqtt_config.mqtt_password,
    };
    let mut mqtt = match crate::mqtt_client::MqttClient::connect(
        stack_ref, &mqtt_config, args.mqtt_config.boot_id,
    ).await {
        Ok(m) => m,
        Err(e) => {
            error!("MQTT connect failed in net_bootstrap: {:?}, resetting", e);
            esp_hal::system::software_reset();
        }
    };
    if let Err(e) = mqtt.post_connect_publish(false).await {
        warn!("Post-connect publish failed: {:?}", e);
    }
    spawner.must_spawn(crate::mqtt_task::mqtt_task(mqtt));
}
```

Change `WifiStack::connect()` signature to:
```rust
pub async fn connect(
    spawner: Spawner,
    wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
    sw_int1: SoftwareInterrupt<'static, 1>,
    rng: Rng,
    ssid: &str,
    password: &str,
    hostname: &str,
    mqtt_config: MqttConfigArgs,
) -> Result<Self, esp_radio::wifi::WifiError>
```

In `connect()` body, replace the embassy_net::new + spawner.spawn(net_task) block with:
```rust
    // Spawn connection_task on ThreadModeExecutor
    spawner.spawn(connection_task(controller).map_err(|e| {
        error!("Failed to spawn connection_task: {:?}", e);
        esp_radio::wifi::WifiError::Failed
    })?);

    // Start InterruptExecutor for net + MQTT tasks
    let net_executor = mk_static!(
        esp_rtos::embassy::InterruptExecutor<1>,
        esp_rtos::embassy::InterruptExecutor::new(sw_int1)
    );
    let send_spawner = net_executor.start(esp_hal::interrupt::Priority::Priority1);
    send_spawner.spawn(net_bootstrap(NetBootstrapArgs {
        interface: wifi_interface,
        net_config,
        seed,
        mqtt_config,
    })).map_err(|e| {
        error!("Failed to spawn net_bootstrap: {:?}", e);
        esp_radio::wifi::WifiError::Failed
    })?;

    // Wait for bootstrap to create Stack and signal it back
    let stack = STACK_READY_SIGNAL.wait().await;

    info!("Waiting for DHCP...");
    stack.wait_config_up().await;
```

Remove the existing `spawner.spawn(net_task(runner)...)` call.

### 2. app/src/main.rs

Change the interrupt setup:
```rust
let sw_int = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
let sw_int1 = sw_int.software_interrupt1;
esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
```

Update `init_wifi` to pass sw_int1 and mqtt_config:
```rust
async fn init_wifi(
    spawner: Spawner,
    wifi_peripheral: esp_hal::peripherals::WIFI<'static>,
    sw_int1: esp_hal::interrupt::software::SoftwareInterrupt<'static, 1>,
    rng: esp_hal::rng::Rng,
    ssid: &str,
    password: &str,
    hostname: &str,
    mqtt_config: crate::wifi::MqttConfigArgs,
) -> crate::wifi::WifiStack {
    match crate::wifi::WifiStack::connect(
        spawner, wifi_peripheral, sw_int1, rng, ssid, password, hostname, mqtt_config,
    ).await {
        Ok(stack) => stack,
        Err(e) => { /* same error handling */ }
    }
}
```

Update the call site — remove `connect_mqtt` and `mqtt_task` spawning:
```rust
// Before init_wifi call, build MqttConfigArgs:
let mqtt_config_args = crate::wifi::MqttConfigArgs {
    device_id: app_config.device_id.clone(),
    mqtt_host: app_config.mqtt_host.clone(),
    mqtt_port: app_config.mqtt_port,
    mqtt_user: app_config.mqtt_user.clone(),
    mqtt_password: app_config.mqtt_password.clone(),
    boot_id: boot_id(),
};

let wifi_stack = init_wifi(
    spawner,
    peripherals.WIFI,
    sw_int1,
    esp_hal::rng::Rng::new(),
    &app_config.wifi_ssid,
    &app_config.wifi_password,
    &app_config.device_id,
    mqtt_config_args,
).await;

// DELETE: let bid = boot_id();
// DELETE: let mut mqtt = connect_mqtt(...)
// DELETE: let _ = mqtt.post_connect_publish(false).await;
// DELETE: crash alarm publish via mqtt.publish()
// DELETE: spawner.spawn(mqtt_task::mqtt_task(mqtt).unwrap())
// DELETE: the connect_mqtt function
```

Crash alarm: Instead of publishing directly via mqtt, send via ALERT_CHANNEL:
```rust
if let Some(ref crash) = pending_crash_alarm {
    let alarm_json = crash_info::crash_alarm_json(crash, FIRMWARE_VERSION);
    let _ = ALERT_CHANNEL.try_send(Vec::from(alarm_json.as_bytes()));
    info!("Crash alarm queued: reason={}", crash.reason.as_str());
    drop(pending_crash_alarm.take());
}
```
Note: ALERT_CHANNEL already drains in mqtt_task, so this just works.

### 3. app/src/mqtt_client.rs

Remove both `socket.set_timeout(Some(Duration::from_secs(60)))` calls (in `connect` and `reconnect`).

Add explicit yield in `recv()` before the socket read — add `Timer::after(Duration::from_millis(1)).await;` right before the `let read_fut = transport.read(&mut buf);` line.

### 4. app/src/mqtt_task.rs

Add the MQTT_TASK_TICK watchdog counter (already partially done):
```rust
pub(crate) static MQTT_TASK_TICK: AtomicU32 = AtomicU32::new(0);
```
Bump it at end of each loop iteration:
```rust
MQTT_TASK_TICK.fetch_add(1, Ordering::Relaxed);
```

### 5. app/src/main.rs — watchdog check in main loop

After `wdt.feed()`, add:
```rust
let mqtt_tick = mqtt_task::MQTT_TASK_TICK.load(Ordering::Relaxed);
if mqtt_tick != mqtt_last_tick {
    mqtt_last_tick = mqtt_tick;
    mqtt_last_tick_time = Instant::now();
} else if mqtt_last_tick_time.elapsed().as_secs() >= 30 {
    warn!("MQTT task appears frozen (tick unchanged for {}s)", mqtt_last_tick_time.elapsed().as_secs());
    send_alert("error", "mqtt_task_frozen");
    mqtt_last_tick_time = Instant::now();
}
```

## What NOT to Change

- `uart_task` — stays on ThreadModeExecutor
- `connection_task` — stays on ThreadModeExecutor
- Channel types (CriticalSectionRawMutex) — already cross-executor safe
- `net_task` function body — unchanged
- `mqtt_task` function body — unchanged (just add tick bump)
- OTA code in main loop — uses `wifi_stack.stack` which still works

## Build Verification

```bash
cd app && cargo check    # Must compile cleanly
cargo test               # Workspace tests pass
```
