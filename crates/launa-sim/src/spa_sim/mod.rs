//! Simulated Balboa BP6013G1 spa mainboard.
//!
//! Generates realistic RS-485 byte streams identical to what a real spa controller
//! would send. Processes incoming commands and updates internal state accordingly.
//! Designed to be connected to `SpaApp` (launa-core) via a `SimTransport`.
//!
//! The sim uses Rust types natively (enums, f32 temps) and only converts to raw
//! bytes at the frame generation boundary.

pub mod config;
pub mod error_injection;
pub mod fault_manager;
pub mod frame_gen;
pub mod frame_splitter;
pub mod physics;
pub mod state;

use alloc::boxed::Box;
use alloc::vec::Vec;

use launa_protocol::fault::FaultCode;
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::status::PumpState;
use launa_protocol::Temperature;

pub use config::{
    FaultLogConfig, FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig,
};
pub use error_injection::ErrorInjection;
pub use fault_manager::FaultManager;
pub use frame_splitter::FrameSplitter;
pub use state::{SpaEvent, SpaEventType, SpaState};

use frame_gen::{
    apply_toggle_by_code, generate_client_id_assignment, generate_config_response,
    generate_fault_log_response, generate_fault_log_response_for_entry,
    generate_filter_cycles_response, generate_information_response, generate_ready_frame,
    generate_registration_query, generate_status_frame,
};
use physics::{next_physics_noise_rand, next_rand, simulate_physics, PhysicsContext};

/// Simulated Balboa BP6013G1 spa mainboard.
///
/// Connects to a `SimTransport` and simulates the real spa's behavior:
/// - Sends periodic status updates (~1 per second via `tick()`)
/// - Sends registration queries until a client registers
/// - Sends `Ready` messages after each status update
/// - Processes incoming command frames and updates internal state
///
/// ## Error Injection
///
/// The simulator supports several error injection features for testing:
/// - **Command success rate**: Drop commands probabilistically
/// - **Bus silence**: Suppress all output for a number of ticks
/// - **Spontaneous events**: Schedule state changes at specific ticks
/// - **Corrupt frames**: Inject frames with bad CRC
/// - **Duplicate frames**: Send the same status frame twice in one tick
///
/// ## Simulation Realism
///
/// - **Frame jitter**: Add 0..N random padding bytes before status frames
/// - **Command latency**: Defer state changes by N ticks
/// - **Ready interval**: Send Ready frames at randomized intervals
pub struct SpaSim {
    pub state: SpaState,
    pub client_id: Option<u8>,
    next_client_id: u8,
    tick_count: u64,
    registered: bool,

    // Subsystems
    error_injection: ErrorInjection,
    fault_manager: FaultManager,
    frame_splitter: FrameSplitter,

    pending_events: Vec<SpaEvent>,

    // Simulation realism fields
    /// Maximum random padding bytes before status frame (0 = no jitter).
    jitter_padding_bytes: u64,
    /// Number of ticks to defer command state changes (0 = immediate).
    command_latency_ticks: u64,
    /// Pending commands waiting for their latency to expire.
    /// Each entry is (remaining_ticks, Box<dyn FnOnce(&mut SpaState)>).
    pending_commands: Vec<(u64, Box<dyn FnOnce(&mut SpaState)>)>,
    /// Min/max ticks between Ready frames. Default (1,1) = every tick.
    ready_interval_range: (u64, u64),
    /// Ticks remaining until the next Ready frame should be sent.
    ready_countdown: u64,
    /// PRNG state for ready interval randomization.
    ready_rng_state: u64,

    // Temperature simulation
    /// If set, the status frame reports 0xFF for current_temp (unknown temperature).
    report_unknown_temp: bool,
    /// If > 0.0, each status frame adds ±jitter to current_temp using deterministic PRNG.
    sensor_noise_jitter: f32,

    // Physics model fields
    /// Ambient temperature used for cooling calculations.
    /// Always stored as Fahrenheit internally.
    /// Default: 70.0°F (backward compatible with original hardcoded value).
    ambient_temp: Temperature,
    /// Heat contribution per tick per running pump (in °F). Even when is_heating=false,
    /// running pumps slowly raise water temp via waste heat.
    /// Default: 0.0 (backward compatible — no pump heat contribution).
    pump_heat_contribution: f32,
    /// Number of ticks after creation that report 0xFF for current_temp.
    /// Default: 0 (backward compatible — no unknown temp period).
    physics_unknown_temp_ticks: u64,
    /// Counter for how many ticks have run since creation (used for unknown temp period).
    physics_tick_count: u64,
    /// Heater overshoot amount in °F. When set, heating continues past set_temp by this amount.
    /// Hysteresis for re-heat is half the overshoot value. Default: 0.0 (no overshoot).
    physics_overshoot: f32,
    /// Physics-mode sensor noise amplitude (±N°F). Applied to reported temp in physics mode.
    /// Distinct from the legacy `sensor_noise_jitter` field.
    /// Default: 0.0 (no noise). Uses deterministic PRNG for reproducibility.
    physics_noise_amplitude: f32,
    /// PRNG state for physics-model sensor noise. Seeded from initial state for determinism.
    physics_noise_rng: u64,
    /// Whether the heater has reached the overshoot ceiling and should stop heating.
    /// Re-heating only occurs when temp drops below set_temp - (overshoot/2).
    heating_overshot: bool,

    // Priming mode simulation
    /// If > 0, the spa is in priming mode (init_mode=0x01). Auto-decrements each tick.
    /// When it reaches 0, priming mode exits (init_mode returns to 0x00).
    priming_remaining_ticks: u64,

    // Configurable response data
    /// Custom filter cycles configuration. Defaults to the hardcoded filter data.
    filter_cycles_config: FilterCyclesConfig,
    /// Custom information response configuration. Defaults to the hardcoded info data.
    information_config: InformationConfig,
    /// Custom configuration response data. Defaults to the hardcoded config data.
    spa_config_config: SpaConfigConfig,
}

impl SpaSim {
    pub fn new() -> Self {
        SpaSim {
            state: SpaState::default(),
            client_id: None,
            next_client_id: 0x02,
            tick_count: 0,
            registered: false,

            error_injection: ErrorInjection::new(),
            fault_manager: FaultManager::new(),
            frame_splitter: FrameSplitter::new(),

            pending_events: Vec::new(),

            jitter_padding_bytes: 0,
            command_latency_ticks: 0,
            pending_commands: Vec::new(),
            ready_interval_range: (1, 1),
            ready_countdown: 1,
            ready_rng_state: 0,

            report_unknown_temp: false,
            sensor_noise_jitter: 0.0,

            ambient_temp: Temperature::fahrenheit(70.0),
            pump_heat_contribution: 0.0,
            physics_unknown_temp_ticks: 0,
            physics_tick_count: 0,
            physics_overshoot: 0.0,
            physics_noise_amplitude: 0.0,
            physics_noise_rng: 0xDEADBEEFCAFE1234,
            heating_overshot: false,

            priming_remaining_ticks: 0,

            filter_cycles_config: FilterCyclesConfig::default(),
            information_config: InformationConfig::default(),
            spa_config_config: SpaConfigConfig::default(),
        }
    }

    /// Set the probability that commands are accepted (0.0 = never, 1.0 = always).
    ///
    /// Uses a deterministic PRNG seeded by a per-command counter for reproducibility.
    pub fn set_command_success_rate(&mut self, rate: f32) {
        self.error_injection.set_command_success_rate(rate);
    }

    /// Simulate bus silence: suppress all output for `duration_ticks` ticks.
    pub fn simulate_bus_silence(&mut self, duration_ticks: u64) {
        self.error_injection.simulate_bus_silence(duration_ticks);
    }

    /// Schedule a spontaneous event to fire at the given tick.
    pub fn schedule_event(&mut self, at_tick: u64, event: SpaEventType) {
        self.pending_events.push(SpaEvent {
            tick: at_tick,
            event_type: event,
        });
    }

    /// Convenience method: schedule a filter cycle start at the given tick.
    pub fn simulate_filter_cycle_start(&mut self, pump_index: usize, at_tick: u64) {
        self.schedule_event(at_tick, SpaEventType::FilterCycleStart { pump_index });
    }

    /// Inject a corrupt frame on the next `generate_status_frame()` call.
    ///
    /// The payload's last byte is XOR'd with 0xFF, producing a bad CRC.
    pub fn inject_corrupt_frame(&mut self) {
        self.error_injection.inject_corrupt_frame();
    }

    /// Inject a duplicate status frame on the next `tick()` call.
    ///
    /// The status frame bytes will be emitted twice in a single tick.
    pub fn inject_duplicate_frame(&mut self) {
        self.error_injection.inject_duplicate_frame();
    }

    /// Simulate a spa reboot.
    ///
    /// Resets registration state (unregistered, client_id cleared),
    /// re-sends registration query on the next tick.
    /// Does NOT reset spa state (temperatures, pump states, etc.) to simulate
    /// a real spa that retains its physical state across reboots.
    pub fn simulate_spa_reboot(&mut self) {
        self.registered = false;
        self.client_id = None;
        // Note: don't reset state (temps, pumps, etc.) — real spa retains physical state
    }

    /// Simulate a fault state.
    ///
    /// Sets the internal fault flag so status frames report init_mode=0x02 (fault active).
    /// The fault log response will carry the given `FaultCode`.
    pub fn simulate_fault_state(&mut self, code: FaultCode) {
        self.fault_manager.simulate_fault_state(code);
    }

    /// Clear the active fault state.
    ///
    /// Restores init_mode to 0x00 in subsequent status frames.
    pub fn clear_fault_state(&mut self) {
        self.fault_manager.clear_fault_state();
    }

    /// Simulate a transient fault that auto-clears after `ticks` ticks.
    ///
    /// For the next `ticks` ticks, status frames report init_mode=0x02 (fault active).
    /// After `ticks` ticks, the fault is automatically cleared and init_mode returns to 0x00.
    /// If `ticks` is 0, no fault is set (immediately cleared).
    pub fn simulate_transient_fault(&mut self, code: FaultCode, ticks: u64) {
        self.fault_manager.simulate_transient_fault(code, ticks);
    }

    /// Simulate priming mode.
    ///
    /// Sets init_mode=0x01 in status frames. The priming mode auto-exits after
    /// `duration_ticks` ticks. Use `clear_priming_mode()` for manual exit.
    /// If `duration_ticks` is 0, priming mode is not entered.
    /// Default: off (0 ticks remaining).
    pub fn simulate_priming_mode(&mut self, duration_ticks: u64) {
        self.priming_remaining_ticks = duration_ticks;
    }

    /// Manually clear priming mode.
    ///
    /// Immediately exits priming mode regardless of remaining duration.
    pub fn clear_priming_mode(&mut self) {
        self.priming_remaining_ticks = 0;
    }

    /// Simulate temperature sensor noise.
    ///
    /// Each status frame will add ±`jitter` to the reported `current_temp`
    /// using a deterministic PRNG. Set jitter to 0.0 to disable noise.
    pub fn simulate_sensor_noise(&mut self, jitter: f32) {
        self.sensor_noise_jitter = jitter.abs();
    }

    /// Simulate unknown temperature.
    ///
    /// Status frames will report 0xFF for current_temp (decoded as `None`)
    /// until `clear_unknown_temp()` is called.
    pub fn simulate_unknown_temp(&mut self) {
        self.report_unknown_temp = true;
    }

    /// Clear the unknown temperature simulation, resuming normal temperature reporting.
    pub fn clear_unknown_temp(&mut self) {
        self.report_unknown_temp = false;
    }

    /// Set the number of initial ticks that report 0xFF (unknown temperature).
    ///
    /// During the first N ticks, status frames report `current_temp = None`.
    /// Internal physics still run normally — only the reported value is affected.
    /// Default: 0 (backward compatible — no unknown temp period).
    pub fn set_physics_unknown_temp_ticks(&mut self, ticks: u64) {
        self.physics_unknown_temp_ticks = ticks;
    }

    /// Set the heater overshoot amount.
    ///
    /// When set, heating continues past `set_temp` by this amount before stopping.
    /// Re-heating occurs when the temperature drops below `set_temp - overshoot/2`.
    /// Takes a `Temperature` so the scale is explicit: `Temperature::fahrenheit(2.0)`.
    /// Default: `Temperature::fahrenheit(0.0)` (no overshoot).
    pub fn set_physics_overshoot(&mut self, overshoot: Temperature) {
        self.physics_overshoot = overshoot.to_fahrenheit().max(0.0);
    }

    /// Set the physics-mode temperature sensor noise amplitude (±N°F).
    ///
    /// When set > 0.0, each tick adds deterministic noise to the *reported* temperature
    /// (not the internal temperature). Uses a deterministic PRNG for reproducibility.
    /// Default: 0.0 (no noise — backward compatible).
    pub fn set_physics_noise_amplitude(&mut self, amplitude: f32) {
        self.physics_noise_amplitude = amplitude.max(0.0);
    }

    /// Set the ambient temperature used for cooling calculations.
    ///
    /// Takes a `Temperature` so the scale is explicit: `Temperature::fahrenheit(70.0)`
    /// or `Temperature::celsius(21.1)`.
    /// Default: `Temperature::fahrenheit(70.0)`.
    pub fn set_ambient_temp(&mut self, temp: Temperature) {
        self.ambient_temp = temp;
    }

    /// Get the current ambient temperature.
    pub fn ambient_temp(&self) -> Temperature {
        self.ambient_temp
    }

    /// Set the pump waste heat contribution per tick per running pump.
    ///
    /// When set, each running pump (non-Off) contributes this amount of waste
    /// heat per physics tick, slowly raising the water temperature even without
    /// active heating. Takes a `Temperature` so the scale is explicit.
    /// The contribution is additive: 3 pumps × 0.02°F = 0.06°F/tick.
    /// Default: `Temperature::fahrenheit(0.0)` (no pump heat).
    pub fn set_pump_heat_contribution(&mut self, contribution: Temperature) {
        self.pump_heat_contribution = contribution.to_fahrenheit().max(0.0);
    }

    /// Generate a deterministic pseudo-random f32 in [-1.0, 1.0] using the physics noise PRNG.
    fn next_physics_noise_rand(&mut self) -> f32 {
        next_physics_noise_rand(&mut self.physics_noise_rng)
    }

    /// Inject a partial frame split at the given byte position.
    ///
    /// Causes the next `tick()` to emit only the first `split_point` bytes of the
    /// status frame. The following `tick()` emits the remainder plus a Ready frame.
    /// One-shot — resets after firing (subsequent ticks are normal).
    ///
    /// If `split_point` is 0, the first tick emits the full status frame and the
    /// second tick emits just the Ready frame.
    pub fn inject_partial_frame_at(&mut self, split_point: usize) {
        self.frame_splitter.inject_partial_frame_at(split_point);
    }

    /// Set the maximum number of random padding bytes to add before the status frame.
    ///
    /// With `jitter_padding_bytes > 0`, each `tick()` adds 0..N random padding bytes
    /// before the status frame, simulating bus noise. Uses the existing LCG PRNG.
    /// Default: 0 (no jitter, identical to original behavior).
    pub fn set_jitter_padding_bytes(&mut self, max_bytes: u64) {
        self.jitter_padding_bytes = max_bytes;
    }

    /// Set the number of ticks to defer command state changes.
    ///
    /// With `command_latency_ticks > 0`, incoming commands are buffered and their
    /// state changes are applied N `tick()` calls later via `pending_commands`.
    /// Default: 0 (immediate, identical to original behavior).
    pub fn set_command_latency_ticks(&mut self, ticks: u64) {
        self.command_latency_ticks = ticks;
    }

    /// Set the interval range (min, max) for Ready frame transmission.
    ///
    /// With range (1, 1) (default), a Ready frame is sent every tick (original behavior).
    /// With range (2, 5), Ready frames are sent at randomized intervals between 2-5 ticks.
    pub fn set_ready_interval_range(&mut self, min: u64, max: u64) {
        self.ready_interval_range = (min.max(1), max.max(min));
    }

    /// Set a custom fault log configuration.
    ///
    /// When set, `generate_fault_log_response()` will produce frames encoding
    /// the configured values. Default behavior is preserved when this is not called.
    pub fn set_fault_log_config(&mut self, config: FaultLogConfig) {
        self.fault_manager.set_fault_log_config(config);
    }

    /// Set a multi-entry fault log.
    ///
    /// Each entry in the Vec corresponds to a fault log entry. When walking entries
    /// via `generate_fault_log_response_for_entry()`, entry_number 1 maps to index 0,
    /// entry_number 2 maps to index 1, etc.
    ///
    /// Entry 0 and entries past the end return a sentinel response (fault_count = 0).
    pub fn set_fault_log_entries(&mut self, entries: Vec<FaultLogConfig>) {
        self.fault_manager.set_fault_log_entries(entries);
    }

    /// Set a custom filter cycles configuration.
    ///
    /// When set, `generate_filter_cycles_response()` will produce frames encoding
    /// the configured values. Default behavior is preserved when this is not called.
    pub fn set_filter_cycles_config(&mut self, config: FilterCyclesConfig) {
        self.filter_cycles_config = config;
    }

    /// Set a custom information response configuration.
    ///
    /// When set, `generate_information_response()` will produce frames encoding
    /// the configured values. Default behavior is preserved when this is not called.
    pub fn set_information_config(&mut self, config: InformationConfig) {
        self.information_config = config;
    }

    /// Set a custom spa configuration response.
    ///
    /// When set, `generate_config_response()` will produce frames encoding
    /// the configured values. Default behavior is preserved when this is not called.
    ///
    /// If `adapt_temperature_scale` is true, the scale bit in byte 3 will be
    /// set/cleared to match the current `SpaState::temp_scale`.
    pub fn set_spa_config_config(&mut self, config: SpaConfigConfig) {
        self.spa_config_config = config;
    }

    /// Deterministic pseudo-random check for command acceptance.
    ///
    /// Returns `true` if the command should be accepted based on the success rate.
    fn should_accept_command(&mut self) -> bool {
        self.error_injection.should_accept_command()
    }

    /// Generate a deterministic pseudo-random u64 using the ready RNG state.
    fn next_ready_rand(&mut self) -> u64 {
        next_rand(&mut self.ready_rng_state)
    }

    /// Generate a random padding length in 0..max using the LCG PRNG.
    fn jitter_padding_len(&mut self) -> usize {
        if self.jitter_padding_bytes == 0 {
            return 0;
        }
        let rand_val = self.next_ready_rand();
        (rand_val % self.jitter_padding_bytes) as usize
    }

    /// Compute the next ready countdown value from the interval range.
    fn next_ready_interval(&mut self) -> u64 {
        let (min, max) = self.ready_interval_range;
        if min == max {
            return min;
        }
        let rand_val = self.next_ready_rand();
        min + (rand_val % (max - min + 1))
    }

    /// Decrement the transient fault countdown, clearing the fault when it reaches zero.
    fn tick_transient_fault_countdown(&mut self) {
        self.fault_manager.tick_transient_fault_countdown();
    }

    /// Decrement the priming mode countdown.
    fn tick_priming_countdown(&mut self) {
        if self.priming_remaining_ticks > 0 {
            self.priming_remaining_ticks -= 1;
        }
    }

    /// Decrement the ready-frame countdown and emit a Ready frame if the countdown reaches zero.
    ///
    /// After emitting, the countdown is reset to the next randomized interval.
    fn maybe_emit_ready(&mut self, output: &mut Vec<u8>) {
        if self.ready_countdown > 0 {
            self.ready_countdown -= 1;
        }
        if self.ready_countdown == 0 {
            output.extend_from_slice(&self.generate_ready_frame());
            self.ready_countdown = self.next_ready_interval();
        }
    }

    /// Process pending deferred commands, decrementing timers and applying expired ones.
    fn process_pending_commands(&mut self) {
        let mut i = 0;
        while i < self.pending_commands.len() {
            self.pending_commands[i].0 -= 1;
            if self.pending_commands[i].0 == 0 {
                let (_, apply_fn) = self.pending_commands.remove(i);
                apply_fn(&mut self.state);
            } else {
                i += 1;
            }
        }
    }

    /// Advance simulated time by 1 second.
    ///
    /// Returns the raw bytes the spa would transmit this second:
    /// - If not registered: a registration query (`FE BF 00`)
    /// - Always: a status update frame (`FF AF 13` with 24-byte payload)
    /// - Always: a `Ready` frame (`10 BF 06`) indicating the bus is free
    ///
    /// Error injection features may modify this output:
    /// - Bus silence suppresses all output
    /// - Spontaneous events are applied before frame generation
    /// - Duplicate frame injection doubles the status frame
    /// - Partial frame injection splits status frame across two ticks
    pub fn tick(&mut self) -> Vec<u8> {
        self.tick_count += 1;

        // Process deferred commands (decrement timers, apply expired)
        self.process_pending_commands();

        // Bus silence: suppress all output
        if self.error_injection.tick_bus_silence() {
            // Still decrement transient fault and priming counters even during silence
            self.tick_transient_fault_countdown();
            self.tick_priming_countdown();

            return Vec::new();
        }

        // Partial frame injection — second tick: emit remainder + Ready
        if let Some(remainder) = self.frame_splitter.take_remainder() {
            let mut output: Vec<u8> = remainder;

            // Always include Ready frame on the remainder tick
            output.extend_from_slice(&self.generate_ready_frame());
            self.ready_countdown = self.next_ready_interval();

            // Still decrement counters for transient fault and priming
            self.tick_transient_fault_countdown();
            self.tick_priming_countdown();

            return output;
        }

        let mut output = Vec::new();

        // Send registration query if no client is registered
        if !self.registered {
            output.extend_from_slice(&self.generate_registration_query());
        }

        // Process scheduled spontaneous events
        self.process_pending_events();

        // Update physical simulation
        self.simulate_physics();

        // Frame jitter: add random padding bytes before status frame
        let padding_len = self.jitter_padding_len();
        for _ in 0..padding_len {
            output.push(0x00);
        }

        // Send status update (this reads fault_active and priming_remaining_ticks)
        let status_bytes = self.generate_status_frame();
        self.tick_transient_fault_countdown();
        self.tick_priming_countdown();

        // Partial frame injection — first tick: split the status frame
        if let Some(split_point) = self.frame_splitter.take_split_point() {
            if split_point == 0 {
                // Split at 0: emit full status frame now, remainder (empty) next tick + Ready
                output.extend_from_slice(&status_bytes);
                self.frame_splitter.set_remainder(Vec::new());
            } else if split_point >= status_bytes.len() {
                // Split point past end: emit full frame normally (edge case)
                output.extend_from_slice(&status_bytes);

                self.maybe_emit_ready(&mut output);
            } else {
                // Split in the middle: emit first N bytes now, store remainder for next tick
                output.extend_from_slice(&status_bytes[..split_point]);
                self.frame_splitter
                    .set_remainder(status_bytes[split_point..].to_vec());
            }
        } else if self.error_injection.take_duplicate_next() {
            // Duplicate frame injection: send status frame twice
            output.extend_from_slice(&status_bytes);
            output.extend_from_slice(&status_bytes);

            self.maybe_emit_ready(&mut output);
        } else {
            output.extend_from_slice(&status_bytes);

            self.maybe_emit_ready(&mut output);
        }

        output
    }

    /// Process any scheduled spontaneous events whose tick has arrived.
    fn process_pending_events(&mut self) {
        let mut i = 0;
        while i < self.pending_events.len() {
            if self.pending_events[i].tick <= self.tick_count {
                let event = self.pending_events.remove(i);
                match event.event_type {
                    SpaEventType::FilterCycleStart { pump_index } => {
                        if pump_index < 6 && self.state.pumps[pump_index] == PumpState::Off {
                            self.state.pumps[pump_index] = PumpState::Low;
                        }
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    /// Process all bytes the controller has written to the transport.
    ///
    /// Parses them as frames and handles each one (toggles, temp changes,
    /// registration responses, etc.). Updates internal state accordingly.
    /// Returns any response frames the spa would send back.
    pub fn process_incoming_bytes(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(bytes);
        let mut responses = Vec::new();

        for frame in &frames {
            if let Some(response) = self.process_frame(frame) {
                responses.extend_from_slice(&response);
            }
        }

        responses
    }

    /// Process a single parsed frame from the controller.
    pub fn process_frame(&mut self, frame: &Frame) -> Option<Vec<u8>> {
        match frame.message_type {
            [0x0A, 0xBF] => {
                if frame.payload.is_empty() {
                    return None;
                }
                match frame.payload[0] {
                    0x04 => Some(self.generate_config_response()),
                    0x11 => {
                        if frame.payload.len() >= 2 {
                            if self.should_accept_command() {
                                let item_code = frame.payload[1];
                                if self.command_latency_ticks == 0 {
                                    self.handle_toggle_by_code(item_code);
                                } else {
                                    let latency = self.command_latency_ticks;
                                    self.pending_commands.push((
                                        latency,
                                        Box::new(move |state: &mut SpaState| {
                                            apply_toggle_by_code(state, item_code);
                                        }),
                                    ));
                                }
                            }
                        }
                        None
                    }
                    0x20 => {
                        if frame.payload.len() >= 2 {
                            if self.should_accept_command() {
                                let raw_temp = frame.payload[1];
                                let scale = self.state.temp_scale;
                                if self.command_latency_ticks == 0 {
                                    self.state
                                        .set_target_temp(Temperature::from_wire(raw_temp, scale));
                                } else {
                                    let latency = self.command_latency_ticks;
                                    self.pending_commands.push((
                                        latency,
                                        Box::new(move |state: &mut SpaState| {
                                            state.set_target_temp(Temperature::from_wire(
                                                raw_temp, scale,
                                            ));
                                        }),
                                    ));
                                }
                            }
                        }
                        None
                    }
                    0x22 => {
                        if frame.payload.len() >= 2 {
                            match frame.payload[1] {
                                0x01 => Some(self.generate_filter_cycles_response()),
                                0x02 => Some(self.generate_information_response()),
                                0x20 => Some(self.generate_fault_log_response()),
                                _ => None,
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            [0xFE, 0xBF] => {
                if frame.payload.len() >= 1 && frame.payload[0] == 0x01 {
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    Some(self.generate_client_id_assignment(id))
                } else {
                    None
                }
            }
            _ => {
                // Client ID ack: <ID> BF 03
                if frame.payload.len() >= 1
                    && frame.message_type[1] == 0xBF
                    && frame.payload[0] == 0x03
                {
                    self.client_id = Some(frame.message_type[0]);
                    self.registered = true;
                }
                None
            }
        }
    }

    /// Simulate temperature changes and time progression.
    ///
    /// Uses a realistic thermal model:
    /// - **Heating**: Rate proportional to `(set_temp + overshoot - current_temp) / delta_range`.
    ///   Base rate ~0.5°F/tick at full delta, tapering as temp approaches target.
    ///   Heating only active when at least one pump or circ_pump is running.
    /// - **Cooling**: Rate proportional to `(current_temp - ambient_temp) / cooling_range`.
    ///   Base rate ~0.1°F/tick when well above ambient, tapering near ambient.
    /// - **Pump heat**: Running pumps contribute waste heat (configurable via
    ///   `set_pump_heat_contribution()`). Each running pump adds the configured amount
    ///   per tick. Default: 0.0 (off).
    /// - **Overshoot**: If `physics_overshoot > 0`, heating continues past `set_temp`
    ///   by the overshoot amount before stopping. Re-heats at `set_temp - overshoot/2`.
    /// - **Interlock**: `is_heating` is forced false whenever all pumps and circ_pump
    ///   are off. Heating resumes automatically when a pump restarts (if temp < set_point).
    ///
    /// Assumptions: ambient temperature defaults to 70°F (configurable via `set_ambient_temp()`).
    fn simulate_physics(&mut self) {
        let mut ctx = PhysicsContext {
            ambient_temp: self.ambient_temp,
            pump_heat_contribution: self.pump_heat_contribution,
            physics_tick_count: self.physics_tick_count,
            physics_overshoot: self.physics_overshoot,
            heating_overshot: self.heating_overshot,
            physics_noise_rng: self.physics_noise_rng,
        };
        simulate_physics(&mut self.state, &mut ctx);
        self.physics_tick_count = ctx.physics_tick_count;
        self.heating_overshot = ctx.heating_overshot;
        self.physics_noise_rng = ctx.physics_noise_rng;
    }

    /// Generate a complete framed status update.
    ///
    /// This is the boundary where Rust types → raw wire bytes.
    /// If corrupt frame injection is enabled, the last payload byte is flipped.
    pub fn generate_status_frame(&mut self) -> Vec<u8> {
        // Pre-compute noise values using the SpaSim's PRNG state
        let physics_noise_value = if self.physics_noise_amplitude > 0.0 {
            self.next_physics_noise_rand() * self.physics_noise_amplitude
        } else {
            0.0
        };
        let ready_rand_value = if self.sensor_noise_jitter > 0.0 {
            self.next_ready_rand() as i64 as f64
        } else {
            0.0
        };

        let inject_corrupt = self.error_injection.take_corrupt_next();

        let result = generate_status_frame(
            &self.state,
            self.priming_remaining_ticks,
            self.fault_manager.fault_active,
            self.report_unknown_temp,
            self.sensor_noise_jitter,
            self.physics_unknown_temp_ticks,
            self.physics_tick_count,
            self.physics_noise_amplitude,
            physics_noise_value,
            ready_rand_value,
            inject_corrupt,
        );

        result
    }

    /// Generate a `Ready` frame (`10 BF 06`).
    pub fn generate_ready_frame(&self) -> Vec<u8> {
        generate_ready_frame()
    }

    /// Generate a registration query (`FE BF 00`).
    pub fn generate_registration_query(&self) -> Vec<u8> {
        generate_registration_query()
    }

    /// Generate a client ID assignment (`FE BF 02 <ID>`).
    fn generate_client_id_assignment(&self, id: u8) -> Vec<u8> {
        generate_client_id_assignment(id)
    }

    /// Generate a configuration response.
    pub fn generate_config_response(&self) -> Vec<u8> {
        generate_config_response(&self.state, &self.spa_config_config)
    }

    /// Generate an information response.
    pub fn generate_information_response(&self) -> Vec<u8> {
        generate_information_response(&self.information_config)
    }

    /// Generate a fault log response.
    pub fn generate_fault_log_response(&self) -> Vec<u8> {
        generate_fault_log_response(&self.fault_manager.fault_log_config)
    }

    /// Generate a fault log response for a specific entry number.
    ///
    /// Entry numbers are 1-based. Entry 0 or entries past the end of the
    /// fault_log_entries list return a sentinel response with fault_count=0.
    /// When no multi-entry fault log is configured, falls back to the single
    /// fault_log_config for entry 1.
    pub fn generate_fault_log_response_for_entry(&self, entry_number: u8) -> Vec<u8> {
        generate_fault_log_response_for_entry(
            &self.fault_manager.fault_log_config,
            &self.fault_manager.fault_log_entries,
            entry_number,
        )
    }

    /// Generate a filter cycles response.
    pub fn generate_filter_cycles_response(&self) -> Vec<u8> {
        generate_filter_cycles_response(&self.filter_cycles_config)
    }

    /// Handle a toggle command by raw protocol item code.
    ///
    /// This is the boundary where raw bytes → Rust type mutations.
    fn handle_toggle_by_code(&mut self, item_code: u8) {
        apply_toggle_by_code(&mut self.state, item_code);
    }

    /// Advance by one tick and return the parsed StatusUpdate from the output.
    ///
    /// Convenience method that combines `tick()` with frame decoding and
    /// status parsing. Returns `None` if no valid status frame was found
    /// in the tick output (e.g., during bus silence).
    pub fn tick_status(&mut self) -> Option<launa_protocol::status::StatusUpdate> {
        let output = self.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&output);
        for frame in &frames {
            if frame.message_type == [0xFF, 0xAF] && frame.payload.len() == 24 {
                if let Ok(status) = launa_protocol::status::StatusUpdate::parse(&frame.payload) {
                    return Some(status);
                }
            }
        }
        None
    }

    /// How many ticks have elapsed.
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }
}

#[cfg(test)]
mod tests;
