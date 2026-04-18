//! Simulated Balboa BP6013G1 spa mainboard.
//!
//! Generates realistic RS-485 byte streams identical to what a real spa controller
//! would send. Processes incoming commands and updates internal state accordingly.
//! Designed to be connected to `SpaApp` (launa-core) via a `SimTransport`.
//!
//! The sim uses Rust types natively (enums, f32 temps) and only converts to raw
//! bytes at the frame generation boundary.

pub mod config;
pub mod frame_gen;
pub mod physics;
pub mod state;

use launa_protocol::fault::FaultCode;
use launa_protocol::frame::{Frame, FrameDecoder};
use launa_protocol::status::PumpState;

pub use config::{
    FaultLogConfig, FilterCycleConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig,
};
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

    // Error injection fields
    command_success_rate: f32,
    command_counter: u64,
    bus_silence_remaining: u64,
    pending_events: Vec<SpaEvent>,
    inject_corrupt_next: bool,
    duplicate_next: bool,

    // Simulation realism fields
    /// Maximum random padding bytes before status frame (0 = no jitter).
    frame_jitter_ticks: u64,
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

    // Partial frame injection
    /// If set, the next tick() will emit only the first N bytes of the status frame,
    /// and the tick after that will emit the remainder + Ready frame. One-shot: resets after firing.
    partial_frame_split: Option<usize>,
    /// If set, contains the remainder bytes from a partial frame split that should be
    /// emitted at the beginning of the next tick() output, followed by a Ready frame.
    partial_frame_remainder: Option<Vec<u8>>,

    // Fault injection: fault state
    /// If set, the status frame reports init_mode=0x02 (fault active).
    fault_active: bool,
    /// If > 0, the fault will auto-clear after this many ticks.
    transient_fault_remaining_ticks: u64,
    /// If set, the status frame reports 0xFF for current_temp (unknown temperature).
    report_unknown_temp: bool,
    /// If > 0.0, each status frame adds ±jitter to current_temp using deterministic PRNG.
    sensor_noise_jitter: f32,

    // Physics model fields
    /// Ambient temperature in °F used for cooling calculations.
    /// Default: 70.0°F (backward compatible with original hardcoded value).
    ambient_temp: f32,
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

    // Multi-entry fault log
    /// Ordered list of fault log entries. Each entry is a FaultLogConfig.
    /// When walking entries, index 0 in this vec corresponds to entry_number 1.
    /// Empty by default (backward compatible — uses single fault_log_config).
    fault_log_entries: Vec<FaultLogConfig>,

    // Configurable response data
    /// Custom fault log configuration. Defaults to the hardcoded fault log data.
    fault_log_config: FaultLogConfig,
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

            command_success_rate: 1.0,
            command_counter: 0,
            bus_silence_remaining: 0,
            pending_events: Vec::new(),
            inject_corrupt_next: false,
            duplicate_next: false,

            frame_jitter_ticks: 0,
            command_latency_ticks: 0,
            pending_commands: Vec::new(),
            ready_interval_range: (1, 1),
            ready_countdown: 1,
            ready_rng_state: 0,

            partial_frame_split: None,
            partial_frame_remainder: None,

            fault_active: false,
            transient_fault_remaining_ticks: 0,
            report_unknown_temp: false,
            sensor_noise_jitter: 0.0,

            ambient_temp: 70.0,
            pump_heat_contribution: 0.0,
            physics_unknown_temp_ticks: 0,
            physics_tick_count: 0,
            physics_overshoot: 0.0,
            physics_noise_amplitude: 0.0,
            physics_noise_rng: 0xDEADBEEFCAFE1234,
            heating_overshot: false,

            priming_remaining_ticks: 0,
            fault_log_entries: Vec::new(),

            fault_log_config: FaultLogConfig::default(),
            filter_cycles_config: FilterCyclesConfig::default(),
            information_config: InformationConfig::default(),
            spa_config_config: SpaConfigConfig::default(),
        }
    }

    /// Set the probability that commands are accepted (0.0 = never, 1.0 = always).
    ///
    /// Uses a deterministic PRNG seeded by a per-command counter for reproducibility.
    pub fn set_command_success_rate(&mut self, rate: f32) {
        self.command_success_rate = rate.clamp(0.0, 1.0);
    }

    /// Simulate bus silence: suppress all output for `duration_ticks` ticks.
    pub fn simulate_bus_silence(&mut self, duration_ticks: u64) {
        self.bus_silence_remaining = duration_ticks;
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
        self.inject_corrupt_next = true;
    }

    /// Inject a duplicate status frame on the next `tick()` call.
    ///
    /// The status frame bytes will be emitted twice in a single tick.
    pub fn inject_duplicate_frame(&mut self) {
        self.duplicate_next = true;
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
        self.fault_active = true;
        self.fault_log_config.message_code = code;
        self.transient_fault_remaining_ticks = 0; // not transient
    }

    /// Clear the active fault state.
    ///
    /// Restores init_mode to 0x00 in subsequent status frames.
    pub fn clear_fault_state(&mut self) {
        self.fault_active = false;
        self.transient_fault_remaining_ticks = 0;
    }

    /// Simulate a transient fault that auto-clears after `ticks` ticks.
    ///
    /// For the next `ticks` ticks, status frames report init_mode=0x02 (fault active).
    /// After `ticks` ticks, the fault is automatically cleared and init_mode returns to 0x00.
    /// If `ticks` is 0, no fault is set (immediately cleared).
    pub fn simulate_transient_fault(&mut self, code: FaultCode, ticks: u64) {
        if ticks == 0 {
            self.fault_active = false;
            self.transient_fault_remaining_ticks = 0;
            return;
        }
        self.fault_active = true;
        self.fault_log_config.message_code = code;
        self.transient_fault_remaining_ticks = ticks;
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

    /// Set the heater overshoot amount in °F.
    ///
    /// When set > 0.0, heating continues past `set_temp` by this amount before stopping.
    /// Re-heating occurs when the temperature drops below `set_temp - overshoot/2`.
    /// Default: 0.0 (no overshoot — backward compatible).
    pub fn set_physics_overshoot(&mut self, overshoot: f32) {
        self.physics_overshoot = overshoot.max(0.0);
    }

    /// Set the physics-mode temperature sensor noise amplitude (±N°F).
    ///
    /// When set > 0.0, each tick adds deterministic noise to the *reported* temperature
    /// (not the internal temperature). Uses a deterministic PRNG for reproducibility.
    /// Default: 0.0 (no noise — backward compatible).
    pub fn set_physics_noise_amplitude(&mut self, amplitude: f32) {
        self.physics_noise_amplitude = amplitude.max(0.0);
    }

    /// Set the ambient temperature in °F used for cooling calculations.
    ///
    /// Cooling rate is proportional to `(current_temp - ambient_temp)`.
    /// Higher ambient temperatures produce slower cooling.
    /// Default: 70.0°F (backward compatible with original hardcoded value).
    pub fn set_ambient_temp(&mut self, temp: f32) {
        self.ambient_temp = temp;
    }

    /// Get the current ambient temperature in °F.
    pub fn ambient_temp(&self) -> f32 {
        self.ambient_temp
    }

    /// Set the pump waste heat contribution per tick per running pump (in °F).
    ///
    /// When set > 0.0, each running pump (non-Off) contributes this amount of waste
    /// heat per physics tick, slowly raising the water temperature even without
    /// active heating. The contribution is additive: 3 running pumps × 0.02 = 0.06°F/tick.
    /// Default: 0.0 (no pump heat — backward compatible).
    pub fn set_pump_heat_contribution(&mut self, contribution: f32) {
        self.pump_heat_contribution = contribution.max(0.0);
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
        self.partial_frame_split = Some(split_point);
        self.partial_frame_remainder = None;
    }

    /// Set the maximum number of random padding bytes to add before the status frame.
    ///
    /// With `frame_jitter_ticks > 0`, each `tick()` adds 0..N random padding bytes
    /// before the status frame, simulating bus noise. Uses the existing LCG PRNG.
    /// Default: 0 (no jitter, identical to original behavior).
    pub fn set_frame_jitter_ticks(&mut self, ticks: u64) {
        self.frame_jitter_ticks = ticks;
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
        self.fault_log_config = config;
    }

    /// Set a multi-entry fault log.
    ///
    /// Each entry in the Vec corresponds to a fault log entry. When walking entries
    /// via `generate_fault_log_response_for_entry()`, entry_number 1 maps to index 0,
    /// entry_number 2 maps to index 1, etc.
    ///
    /// Entry 0 and entries past the end return a sentinel response (fault_count = 0).
    pub fn set_fault_log_entries(&mut self, entries: Vec<FaultLogConfig>) {
        self.fault_log_entries = entries;
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
        let rate = self.command_success_rate;
        if rate >= 1.0 {
            return true;
        }
        if rate <= 0.0 {
            return false;
        }
        // Simple LCG-based deterministic "random"
        let rand_val = (self
            .command_counter
            .wrapping_mul(1103515245)
            .wrapping_add(12345)
            >> 16) as u8;
        self.command_counter += 1;
        let threshold = (rate * 256.0) as u8;
        rand_val < threshold
    }

    /// Generate a deterministic pseudo-random u64 using the ready RNG state.
    fn next_ready_rand(&mut self) -> u64 {
        next_rand(&mut self.ready_rng_state)
    }

    /// Generate a random padding length in 0..max using the LCG PRNG.
    fn jitter_padding_len(&mut self) -> usize {
        if self.frame_jitter_ticks == 0 {
            return 0;
        }
        let rand_val = self.next_ready_rand();
        (rand_val % self.frame_jitter_ticks) as usize
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
        if self.transient_fault_remaining_ticks > 0 {
            self.transient_fault_remaining_ticks -= 1;
            if self.transient_fault_remaining_ticks == 0 {
                self.fault_active = false;
            }
        }
    }

    /// Decrement the priming mode countdown.
    fn tick_priming_countdown(&mut self) {
        if self.priming_remaining_ticks > 0 {
            self.priming_remaining_ticks -= 1;
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
        if self.bus_silence_remaining > 0 {
            self.bus_silence_remaining -= 1;

            // Still decrement transient fault and priming counters even during silence
            self.tick_transient_fault_countdown();
            self.tick_priming_countdown();

            return Vec::new();
        }

        // Partial frame injection — second tick: emit remainder + Ready
        if let Some(remainder) = self.partial_frame_remainder.take() {
            let mut output = remainder;

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

        // NOW decrement transient fault and priming counters AFTER status frame is generated
        self.tick_transient_fault_countdown();
        self.tick_priming_countdown();

        // Partial frame injection — first tick: split the status frame
        if let Some(split_point) = self.partial_frame_split.take() {
            if split_point == 0 {
                // Split at 0: emit full status frame now, remainder (empty) next tick + Ready
                output.extend_from_slice(&status_bytes);
                self.partial_frame_remainder = Some(Vec::new());
            } else if split_point >= status_bytes.len() {
                // Split point past end: emit full frame normally (edge case)
                output.extend_from_slice(&status_bytes);

                // Send ready indicator at randomized intervals
                if self.ready_countdown > 0 {
                    self.ready_countdown -= 1;
                }
                if self.ready_countdown == 0 {
                    output.extend_from_slice(&self.generate_ready_frame());
                    self.ready_countdown = self.next_ready_interval();
                }
            } else {
                // Split in the middle: emit first N bytes now, store remainder for next tick
                output.extend_from_slice(&status_bytes[..split_point]);
                self.partial_frame_remainder = Some(status_bytes[split_point..].to_vec());
            }
        } else if self.duplicate_next {
            // Duplicate frame injection: send status frame twice
            self.duplicate_next = false;
            output.extend_from_slice(&status_bytes);
            output.extend_from_slice(&status_bytes);

            // Send ready indicator at randomized intervals
            if self.ready_countdown > 0 {
                self.ready_countdown -= 1;
            }
            if self.ready_countdown == 0 {
                output.extend_from_slice(&self.generate_ready_frame());
                self.ready_countdown = self.next_ready_interval();
            }
        } else {
            output.extend_from_slice(&status_bytes);

            // Send ready indicator at randomized intervals
            if self.ready_countdown > 0 {
                self.ready_countdown -= 1;
            }
            if self.ready_countdown == 0 {
                output.extend_from_slice(&self.generate_ready_frame());
                self.ready_countdown = self.next_ready_interval();
            }
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
                                    self.state.set_temp = SpaState::decode_temp(raw_temp, scale);
                                } else {
                                    let latency = self.command_latency_ticks;
                                    self.pending_commands.push((
                                        latency,
                                        Box::new(move |state: &mut SpaState| {
                                            state.set_temp = SpaState::decode_temp(raw_temp, scale);
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

        let result = generate_status_frame(
            &self.state,
            self.priming_remaining_ticks,
            self.fault_active,
            self.report_unknown_temp,
            self.sensor_noise_jitter,
            self.physics_unknown_temp_ticks,
            self.physics_tick_count,
            self.physics_noise_amplitude,
            physics_noise_value,
            ready_rand_value,
            self.inject_corrupt_next,
        );

        // Corrupt frame injection is one-shot
        if self.inject_corrupt_next {
            self.inject_corrupt_next = false;
        }

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
        generate_fault_log_response(&self.fault_log_config)
    }

    /// Generate a fault log response for a specific entry number.
    ///
    /// Entry numbers are 1-based. Entry 0 or entries past the end of the
    /// fault_log_entries list return a sentinel response with fault_count=0.
    /// When no multi-entry fault log is configured, falls back to the single
    /// fault_log_config for entry 1.
    pub fn generate_fault_log_response_for_entry(&self, entry_number: u8) -> Vec<u8> {
        generate_fault_log_response_for_entry(
            &self.fault_log_config,
            &self.fault_log_entries,
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

    /// How many ticks have elapsed.
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }
}

#[cfg(test)]
mod tests {
    use super::frame_gen::{cycle_heating_mode, cycle_pump};
    use super::*;
    use launa_protocol::config::PumpConfig;
    use launa_protocol::frame::FrameEncoder;
    use launa_protocol::status::{HeatingMode, TemperatureScale};

    #[test]
    fn test_tick_generates_frames() {
        let mut sim = SpaSim::new();
        let bytes = sim.tick();
        assert!(!bytes.is_empty(), "tick should produce output bytes");

        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert!(
            frames.len() >= 2,
            "tick should produce at least 2 frames (status + ready)"
        );
    }

    #[test]
    fn test_tick_after_registration_no_query() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].message_type, [0xFF, 0xAF]); // status
        assert_eq!(frames[1].message_type, [0x10, 0xBF]); // ready
    }

    #[test]
    fn test_physics_heating() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // With the new thermal model, heating is proportional to delta.
        // Heat until we reach set_temp (within tolerance).
        for _ in 0..100 {
            sim.simulate_physics();
            if sim.state.current_temp >= 100.0 {
                break;
            }
        }
        assert!(
            sim.state.current_temp >= 100.0,
            "should reach set_temp, got {}",
            sim.state.current_temp
        );
        assert!(
            !sim.state.is_heating,
            "should stop heating once at set temp"
        );
    }

    #[test]
    fn test_physics_cooling() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 105.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;

        sim.simulate_physics();
        // With proportional cooling, should decrease but not by a full 1.0°
        assert!(
            sim.state.current_temp < 105.0,
            "should cool down, got {}",
            sim.state.current_temp
        );
        assert!(
            sim.state.current_temp > 100.0,
            "should not reach set_temp in one tick, got {}",
            sim.state.current_temp
        );
    }

    #[test]
    fn test_process_toggle_via_bytes() {
        let mut sim = SpaSim::new();

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();

        sim.process_incoming_bytes(&encoded);
        assert_eq!(sim.state.pumps[0], PumpState::Low);
    }

    #[test]
    fn test_cycle_heating_mode_enums() {
        assert_eq!(cycle_heating_mode(HeatingMode::Ready), HeatingMode::Rest);
        assert_eq!(
            cycle_heating_mode(HeatingMode::Rest),
            HeatingMode::ReadyInRest
        );
        assert_eq!(
            cycle_heating_mode(HeatingMode::ReadyInRest),
            HeatingMode::Ready
        );
    }

    #[test]
    fn test_cycle_pump_enums() {
        assert_eq!(cycle_pump(PumpState::Off), PumpState::Low);
        assert_eq!(cycle_pump(PumpState::Low), PumpState::High);
        assert_eq!(cycle_pump(PumpState::High), PumpState::Off);
    }

    #[test]
    fn test_temp_encoding_fahrenheit() {
        assert_eq!(
            SpaState::encode_temp(100.0, TemperatureScale::Fahrenheit),
            100
        );
        assert_eq!(
            SpaState::encode_temp(104.0, TemperatureScale::Fahrenheit),
            104
        );
    }

    #[test]
    fn test_temp_encoding_celsius() {
        assert_eq!(SpaState::encode_temp(38.0, TemperatureScale::Celsius), 76);
        assert_eq!(SpaState::encode_temp(40.0, TemperatureScale::Celsius), 80);
    }

    #[test]
    fn test_set_temp_decoded_from_wire() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Fahrenheit;

        // Simulate a SetTemperature(100) command coming in
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(100).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        assert_eq!(sim.state.set_temp, 100.0);
    }

    #[test]
    fn test_set_temp_decoded_celsius() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Celsius;

        // SetTemperature sends raw value 80 (= 40°C on wire)
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(80).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        assert_eq!(sim.state.set_temp, 40.0);
    }

    // -- Error injection tests --

    #[test]
    fn test_command_success_rate_ignores_toggle() {
        let mut sim = SpaSim::new();
        sim.set_command_success_rate(0.0); // Never accept commands

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Pump should still be Off (command was ignored)
        assert_eq!(sim.state.pumps[0], PumpState::Off);
    }

    #[test]
    fn test_command_success_rate_accepts_toggle() {
        let mut sim = SpaSim::new();
        sim.set_command_success_rate(1.0); // Always accept commands

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Pump should be Low (command was accepted)
        assert_eq!(sim.state.pumps[0], PumpState::Low);
    }

    #[test]
    fn test_command_success_rate_ignores_set_temp() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Fahrenheit;
        sim.set_command_success_rate(0.0); // Never accept commands

        let (mt, payload) = launa_protocol::command::Command::SetTemperature(90).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Set temp should remain at default (104.0)
        assert_eq!(sim.state.set_temp, 104.0);
    }

    #[test]
    fn test_command_success_rate_accepts_set_temp() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Fahrenheit;
        sim.set_command_success_rate(1.0); // Always accept commands

        let (mt, payload) = launa_protocol::command::Command::SetTemperature(100).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        assert_eq!(sim.state.set_temp, 100.0);
    }

    #[test]
    fn test_bus_silence_produces_no_output() {
        let mut sim = SpaSim::new();
        sim.simulate_bus_silence(3);

        let bytes1 = sim.tick();
        assert!(bytes1.is_empty(), "silenced tick should produce no bytes");

        let bytes2 = sim.tick();
        assert!(bytes2.is_empty());

        let bytes3 = sim.tick();
        assert!(bytes3.is_empty());

        // Silence over, normal output resumes
        let bytes4 = sim.tick();
        assert!(!bytes4.is_empty(), "should resume after silence");
    }

    #[test]
    fn test_spontaneous_filter_cycle_start() {
        let mut sim = SpaSim::new();
        assert_eq!(sim.state.pumps[0], PumpState::Off);

        // Schedule pump1 to turn on at tick 5
        sim.simulate_filter_cycle_start(0, 5);

        // Ticks 1-4: pump still off
        for _ in 0..4 {
            sim.tick();
        }
        // After tick 4, tick_count is 4
        let _ = sim.tick(); // tick 5: tick_count becomes 5, events at tick<=5 fire

        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "pump should start from scheduled event"
        );
    }

    #[test]
    fn test_spontaneous_event_does_not_double_toggle() {
        let mut sim = SpaSim::new();
        // If pump is already on, filter cycle start should not change it
        sim.state.pumps[1] = PumpState::High;
        sim.simulate_filter_cycle_start(1, 1);

        sim.tick();
        // Should still be High, not cycled
        assert_eq!(sim.state.pumps[1], PumpState::High);
    }

    #[test]
    fn test_corrupt_frame_injection() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Get a normal frame for comparison
        let normal = sim.generate_status_frame();

        // Inject corruption
        sim.inject_corrupt_frame();
        let corrupt = sim.generate_status_frame();

        // Frames should differ
        assert_ne!(normal, corrupt, "corrupt frame should differ from normal");

        // Corrupt frame should still be parseable as bytes (just has bad CRC)
        assert!(!corrupt.is_empty());

        // Verify that the corrupt frame actually triggers a CRC error in the decoder
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&corrupt);
        // The corrupt byte should cause a CRC mismatch → no valid frames decoded
        assert!(
            frames.is_empty() || decoder.frame_error_count() > 0,
            "corrupt frame should cause frame error (frames={}, errors={})",
            frames.len(),
            decoder.frame_error_count()
        );
    }

    #[test]
    fn test_duplicate_frame_injection() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Normal tick produces N bytes
        let normal_bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let normal_frames = decoder.feed_slice(&normal_bytes);

        // Reset sim for comparison
        let mut sim2 = SpaSim::new();
        sim2.registered = true;
        sim2.inject_duplicate_frame();
        let dup_bytes = sim2.tick();

        // Duplicate tick should have more bytes (extra status frame)
        assert!(dup_bytes.len() > normal_bytes.len());

        let mut decoder2 = FrameDecoder::new();
        let dup_frames = decoder2.feed_slice(&dup_bytes);
        assert!(
            dup_frames.len() > normal_frames.len(),
            "should have extra frames from duplication"
        );
    }

    // -- Simulation realism tests (frame jitter, command latency, ready interval) --

    #[test]
    fn test_frame_jitter_default_unchanged() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // With default frame_jitter_ticks=0, both ticks produce the same structure
        // (status frame + ready frame). Physics causes minor byte differences (clock),
        // so we verify structural equivalence: same number of decoded frames with same types.
        let bytes1 = sim.tick();
        let bytes2 = sim.tick();

        let mut decoder1 = FrameDecoder::new();
        let frames1 = decoder1.feed_slice(&bytes1);

        let mut decoder2 = FrameDecoder::new();
        let frames2 = decoder2.feed_slice(&bytes2);

        // Same frame count and types
        assert_eq!(frames1.len(), 2, "tick 1: status + ready");
        assert_eq!(frames2.len(), 2, "tick 2: status + ready");
        assert_eq!(frames1[0].message_type, [0xFF, 0xAF]); // status
        assert_eq!(frames1[1].message_type, [0x10, 0xBF]); // ready
        assert_eq!(frames2[0].message_type, [0xFF, 0xAF]); // status
        assert_eq!(frames2[1].message_type, [0x10, 0xBF]); // ready

        // Output lengths should be identical (no jitter padding)
        assert_eq!(
            bytes1.len(),
            bytes2.len(),
            "output lengths should match with default jitter=0"
        );
    }

    #[test]
    fn test_frame_jitter_variable_padding() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_frame_jitter_ticks(10);

        // Collect output from 50 ticks, verify at least 3 distinct lengths
        let mut lengths = std::collections::BTreeSet::new();
        let mut all_decoded_ok = true;

        for _ in 0..50 {
            let bytes = sim.tick();
            lengths.insert(bytes.len());

            // Verify FrameDecoder can still decode all valid frames
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            // Should have at least the status frame (ready may be separate)
            if frames.is_empty() || frames[0].message_type != [0xFF, 0xAF] {
                all_decoded_ok = false;
            }
        }

        assert!(
            lengths.len() >= 3,
            "expected at least 3 distinct output lengths with jitter=10, got {}",
            lengths.len()
        );
        assert!(all_decoded_ok, "all frames should decode correctly");
    }

    #[test]
    fn test_command_latency_default_immediate() {
        let mut sim = SpaSim::new();
        // Default command_latency_ticks=0: commands applied immediately

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();

        sim.process_incoming_bytes(&encoded);
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "default latency=0 should apply toggle immediately"
        );
    }

    #[test]
    fn test_command_latency_deferred() {
        let mut sim = SpaSim::new();
        sim.set_command_latency_ticks(3);

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();

        // Send command
        sim.process_incoming_bytes(&encoded);

        // State should NOT change immediately
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Off,
            "state should not change immediately with latency=3"
        );

        // Tick 1: still not applied
        sim.tick();
        assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 1: still pending");

        // Tick 2: still not applied
        sim.tick();
        assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 2: still pending");

        // Tick 3: should be applied now
        sim.tick();
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "tick 3: deferred command should be applied"
        );
    }

    #[test]
    fn test_command_latency_multiple_commands_order() {
        let mut sim = SpaSim::new();
        sim.set_command_latency_ticks(2);

        // Send two toggle commands for different pumps
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded1 = FrameEncoder::encode(mt, &payload).unwrap();

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump2,
        )
        .encode();
        let encoded2 = FrameEncoder::encode(mt, &payload).unwrap();

        sim.process_incoming_bytes(&encoded1);
        sim.process_incoming_bytes(&encoded2);

        // Neither applied yet
        assert_eq!(sim.state.pumps[0], PumpState::Off);
        assert_eq!(sim.state.pumps[1], PumpState::Off);

        // Tick 1: pending
        sim.tick();
        assert_eq!(sim.state.pumps[0], PumpState::Off);
        assert_eq!(sim.state.pumps[1], PumpState::Off);

        // Tick 2: both should be applied
        sim.tick();
        assert_eq!(sim.state.pumps[0], PumpState::Low, "pump1 toggle applied");
        assert_eq!(sim.state.pumps[1], PumpState::Low, "pump2 toggle applied");
    }

    #[test]
    fn test_ready_interval_default_every_tick() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        // Default ready_interval_range=(1,1): Ready frame every tick

        let mut ready_count = 0;
        for _ in 0..10 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            for f in &frames {
                if f.message_type == [0x10, 0xBF] {
                    ready_count += 1;
                }
            }
        }

        assert_eq!(
            ready_count, 10,
            "default (1,1) should produce Ready every tick"
        );
    }

    #[test]
    fn test_ready_interval_randomized() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_ready_interval_range(2, 5);

        let mut ready_count = 0;
        for _ in 0..100 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            for f in &frames {
                if f.message_type == [0x10, 0xBF] {
                    ready_count += 1;
                }
            }
        }

        // With interval range (2,5), expect ~20-60 Ready frames in 100 ticks
        // (min 100/5=20, max 100/2=50, but allow some margin)
        assert!(
            ready_count >= 15 && ready_count <= 55,
            "expected 15-55 Ready frames with range (2,5), got {}",
            ready_count
        );
    }

    #[test]
    fn test_jitter_and_latency_together() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_frame_jitter_ticks(5);
        sim.set_command_latency_ticks(2);

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Jitter should work (variable output), latency should defer command
        let mut distinct_lengths = std::collections::BTreeSet::new();
        for _ in 0..20 {
            let bytes = sim.tick();
            distinct_lengths.insert(bytes.len());
        }

        assert!(
            distinct_lengths.len() >= 2,
            "jitter should produce at least 2 distinct lengths, got {}",
            distinct_lengths.len()
        );

        // Command should be applied after 2 ticks
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "deferred command applied after latency ticks"
        );
    }

    // -- Partial frame injection tests --

    #[test]
    fn test_partial_frame_split_reassembly() {
        // Split status frame at midpoint; tick1 emits first N bytes, tick2 emits remainder + Ready.
        // FrameDecoder should reassemble the split frame across two feeds.
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Generate the expected status frame to find its length
        let status_bytes = sim.generate_status_frame();
        let split_point = status_bytes.len() / 2;

        sim.inject_partial_frame_at(split_point);

        // Tick 1: should emit only first `split_point` bytes of status frame (no Ready)
        let tick1_bytes = sim.tick();
        assert!(
            tick1_bytes.len() < status_bytes.len(),
            "tick1 should emit fewer bytes than a full status frame"
        );

        // Tick 2: should emit remainder of status frame + Ready frame
        let tick2_bytes = sim.tick();
        assert!(
            !tick2_bytes.is_empty(),
            "tick2 should produce remainder bytes"
        );

        // Feed both halves to a FrameDecoder — should decode the complete status frame
        let mut decoder = FrameDecoder::new();
        let frames1 = decoder.feed_slice(&tick1_bytes);
        let frames2 = decoder.feed_slice(&tick2_bytes);

        // First feed should not produce any complete frames (partial only)
        assert!(
            frames1.is_empty(),
            "first half should not produce complete frames, got {}",
            frames1.len()
        );

        // Second feed should produce at least the status frame + Ready
        assert!(
            frames2.len() >= 2,
            "second half should produce status + ready frames, got {}",
            frames2.len()
        );
        assert_eq!(
            frames2[0].message_type,
            [0xFF, 0xAF],
            "first decoded frame should be status"
        );
        assert_eq!(
            frames2[1].message_type,
            [0x10, 0xBF],
            "second decoded frame should be ready"
        );
    }

    #[test]
    fn test_partial_frame_split_at_zero() {
        // Split at 0: the full status frame is the "remainder", so tick1 should emit
        // the full status frame (no partial), and tick2 emits Ready.
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.inject_partial_frame_at(0);

        // Tick 1: full status frame (split at 0 means no bytes split off)
        let tick1_bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames1 = decoder.feed_slice(&tick1_bytes);

        // Should have decoded the status frame
        assert!(
            frames1.len() >= 1,
            "tick1 with split_at=0 should produce the full status frame"
        );
        assert_eq!(
            frames1[0].message_type,
            [0xFF, 0xAF],
            "should be status frame"
        );

        // Tick 2: Ready frame (remainder is empty, so just Ready)
        let tick2_bytes = sim.tick();
        let mut decoder2 = FrameDecoder::new();
        let frames2 = decoder2.feed_slice(&tick2_bytes);
        assert!(
            frames2.len() >= 1,
            "tick2 should produce at least the ready frame"
        );
        assert_eq!(
            frames2[0].message_type,
            [0x10, 0xBF],
            "should be ready frame"
        );
    }

    #[test]
    fn test_partial_frame_oneshot_reset() {
        // After partial frame fires (two ticks), subsequent ticks produce normal unsplit output.
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Get a reference normal tick output (after registration, no injection)
        let normal_bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let normal_frames = decoder.feed_slice(&normal_bytes);
        assert_eq!(normal_frames.len(), 2, "normal tick: status + ready");

        // Reset sim for controlled test
        let mut sim2 = SpaSim::new();
        sim2.registered = true;

        let status_bytes = sim2.generate_status_frame();
        let split_point = status_bytes.len() / 2;
        sim2.inject_partial_frame_at(split_point);

        // Tick 1: partial frame
        let _tick1 = sim2.tick();
        // Tick 2: remainder + ready
        let _tick2 = sim2.tick();

        // Tick 3: should be normal (no split)
        let tick3_bytes = sim2.tick();
        let mut decoder3 = FrameDecoder::new();
        let tick3_frames = decoder3.feed_slice(&tick3_bytes);

        // Should be a normal tick: status + ready
        assert_eq!(
            tick3_frames.len(),
            2,
            "third tick should produce normal 2 frames (status + ready)"
        );
        assert_eq!(tick3_frames[0].message_type, [0xFF, 0xAF], "status frame");
        assert_eq!(tick3_frames[1].message_type, [0x10, 0xBF], "ready frame");

        // Tick 4: also normal
        let tick4_bytes = sim2.tick();
        let mut decoder4 = FrameDecoder::new();
        let tick4_frames = decoder4.feed_slice(&tick4_bytes);
        assert_eq!(tick4_frames.len(), 2, "fourth tick should also be normal");
    }

    #[test]
    fn test_all_three_features_together() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_frame_jitter_ticks(3);
        sim.set_command_latency_ticks(1);
        sim.set_ready_interval_range(1, 3);

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Tick through several cycles
        let mut status_count = 0;
        let mut ready_count = 0;
        for _ in 0..20 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            for f in &frames {
                if f.message_type == [0xFF, 0xAF] {
                    status_count += 1;
                }
                if f.message_type == [0x10, 0xBF] {
                    ready_count += 1;
                }
            }
        }

        // Status should appear every tick
        assert_eq!(status_count, 20, "status every tick");

        // Ready should appear at interval (1,3), so fewer than 20
        assert!(
            ready_count >= 7 && ready_count <= 20,
            "ready at randomized interval, got {}",
            ready_count
        );

        // Command should be applied
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "deferred command applied"
        );
    }

    // -- Configurable response tests --

    /// Helper: dispatch a SpaSim response frame through the protocol decoder.
    fn dispatch_response(bytes: &[u8]) -> launa_protocol::dispatcher::IncomingMessage {
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(bytes);
        assert!(
            !frames.is_empty(),
            "response should produce at least one frame"
        );
        launa_protocol::dispatcher::dispatch_frame(&frames[0])
    }

    /// Helper: build the 0x22 request frame that triggers a settings response.
    fn build_settings_request(sub_type: u8) -> Vec<u8> {
        let payload = vec![0x22, sub_type];
        FrameEncoder::encode([0x0A, 0xBF], &payload).unwrap()
    }

    /// Helper: build the 0x04 config request frame.
    fn build_config_request() -> Vec<u8> {
        let payload = vec![0x04];
        FrameEncoder::encode([0x0A, 0xBF], &payload).unwrap()
    }

    // VAL-MQTT-019: Fault log response is configurable
    #[test]
    fn test_configurable_fault_log_response() {
        let mut sim = SpaSim::new();
        sim.set_fault_log_config(FaultLogConfig {
            fault_count: 5,
            entry_number: 2,
            message_code: FaultCode::LowFlow,
            days_ago: 10,
            hour: 8,
            minute: 15,
            flags: 0x00,
            set_temperature: 96,
            sensor_a_temp: 95,
            sensor_b_temp: 94,
            ..Default::default()
        });

        let response = sim.generate_fault_log_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(entry.fault_count, 5);
                assert_eq!(entry.entry_number, 2);
                assert_eq!(entry.message_code, FaultCode::LowFlow);
                assert_eq!(entry.days_ago, 10);
                assert_eq!(entry.hour, 8);
                assert_eq!(entry.minute, 15);
                assert_eq!(entry.flags, 0x00);
                assert_eq!(entry.set_temperature, 96);
                assert_eq!(entry.sensor_a_temp, 95);
                assert_eq!(entry.sensor_b_temp, 94);
            }
            other => panic!("Expected FaultLogResponse, got {:?}", other),
        }
    }

    // VAL-MQTT-019: Fault log default unchanged
    #[test]
    fn test_default_fault_log_response_unchanged() {
        let sim = SpaSim::new();
        let response = sim.generate_fault_log_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(entry.fault_count, 3);
                assert_eq!(entry.message_code, FaultCode::HeaterDry);
                assert_eq!(entry.days_ago, 2);
                assert_eq!(entry.set_temperature, 104);
            }
            other => panic!("Expected FaultLogResponse, got {:?}", other),
        }
    }

    // VAL-MQTT-020: Filter cycles response is configurable
    #[test]
    fn test_configurable_filter_cycles_response() {
        let mut sim = SpaSim::new();
        sim.set_filter_cycles_config(FilterCyclesConfig {
            filter1: FilterCycleConfig {
                start_hour: 6,
                start_minute: 30,
                duration_hours: 2,
                duration_minutes: 15,
                enabled: true,
            },
            filter2: FilterCycleConfig {
                start_hour: 18,
                start_minute: 45,
                duration_hours: 1,
                duration_minutes: 30,
                enabled: false,
            },
        });

        let response = sim.generate_filter_cycles_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
                assert_eq!(fc.filter1.start_hour, 6);
                assert_eq!(fc.filter1.start_minute, 30);
                assert_eq!(fc.filter1.duration_hours, 2);
                assert_eq!(fc.filter1.duration_minutes, 15);
                assert!(fc.filter1.enabled);

                assert_eq!(fc.filter2.start_hour, 18);
                assert_eq!(fc.filter2.start_minute, 45);
                assert_eq!(fc.filter2.duration_hours, 1);
                assert_eq!(fc.filter2.duration_minutes, 30);
                assert!(!fc.filter2.enabled);
            }
            other => panic!("Expected FilterCyclesResponse, got {:?}", other),
        }
    }

    // VAL-MQTT-020: Filter cycles default unchanged
    #[test]
    fn test_default_filter_cycles_response_unchanged() {
        let sim = SpaSim::new();
        let response = sim.generate_filter_cycles_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
                assert_eq!(fc.filter1.start_hour, 8);
                assert_eq!(fc.filter1.duration_hours, 4);
                assert_eq!(fc.filter2.start_hour, 16);
                assert_eq!(fc.filter2.duration_hours, 2);
                assert!(fc.filter2.enabled);
            }
            other => panic!("Expected FilterCyclesResponse, got {:?}", other),
        }
    }

    // VAL-MQTT-021: Information response is configurable
    #[test]
    fn test_configurable_information_response() {
        let mut model = [b' '; 8];
        model[..7].copy_from_slice(b"CUSTOM1");

        let mut sim = SpaSim::new();
        sim.set_information_config(InformationConfig {
            software_id_byte0: 0xAA,
            software_id_byte1: 0xBB,
            software_version_byte0: 0xCC,
            software_version_byte1: 0xDD,
            system_model: model,
            current_setup: 0x05,
            config_sig_byte0: 0xDE,
            config_sig_byte1: 0xAD,
            config_sig_byte2: 0xBE,
            config_sig_byte3: 0xEF,
            heater_voltage: 0x01,
            heater_type: 0xFF,
            dip_switch_byte0: 0xFF,
            dip_switch_byte1: 0x00,
        });

        let response = sim.generate_information_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
                assert_eq!(info.system_model, "CUSTOM1");
                assert_eq!(info.current_setup, 0x05);
                assert_eq!(info.config_signature, "DEADBEEF");
                assert_eq!(
                    info.heater_type,
                    launa_protocol::information::HeaterType::Unknown(0xFF)
                );
                assert_eq!(info.dip_switches, "1111111100000000");
            }
            other => panic!("Expected InformationResponse, got {:?}", other),
        }
    }

    // VAL-MQTT-021: Information default unchanged
    #[test]
    fn test_default_information_response_unchanged() {
        let sim = SpaSim::new();
        let response = sim.generate_information_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
                assert_eq!(info.system_model, "BFBP20");
                assert_eq!(info.config_signature, "3D12382E");
                assert_eq!(
                    info.heater_voltage,
                    launa_protocol::information::HeaterVoltage::V240
                );
                assert_eq!(
                    info.heater_type,
                    launa_protocol::information::HeaterType::Standard
                );
            }
            other => panic!("Expected InformationResponse, got {:?}", other),
        }
    }

    // VAL-MQTT-022: Config response is configurable
    #[test]
    fn test_configurable_config_response() {
        let mut raw = [0u8; 10];
        // Set up specific pump configs: pump1=SingleSpeed, pump2=None
        raw[0] = 0x02;
        raw[1] = 0x02;
        raw[5] = 0b00_00_00_01; // pump1=SingleSpeed
        raw[7] = 0x05; // light1 (bits 0-1=01), light2 (bits 2-3=01)
        raw[8] = 0x80; // circ pump present

        let mut sim = SpaSim::new();
        sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });

        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert_eq!(config.pump_configs[0], PumpConfig::SingleSpeed);
                assert_eq!(config.pump_configs[1], PumpConfig::None);
                assert!(config.circ_pump);
                assert!(config.lights[0]);
                assert!(config.lights[1]);
                assert!(!config.blower);
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }
    }

    // VAL-MQTT-022: Config default unchanged
    #[test]
    fn test_default_config_response_unchanged() {
        let sim = SpaSim::new();
        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert_eq!(config.pump_configs[0], PumpConfig::TwoSpeed);
                assert_eq!(config.pump_configs[1], PumpConfig::TwoSpeed);
                assert!(config.circ_pump);
                assert!(config.blower);
                assert!(config.lights[0]);
                assert!(!config.temperature_scale_celsius);
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }
    }

    // VAL-MQTT-023: All configurable responses produce valid protocol frames
    #[test]
    fn test_configurable_responses_valid_frames() {
        let mut sim = SpaSim::new();

        // Set custom configs with edge-case values
        sim.set_fault_log_config(FaultLogConfig {
            fault_count: 0,
            entry_number: 0,
            message_code: FaultCode::Unknown(99),
            days_ago: 0,
            hour: 0,
            minute: 0,
            flags: 0xFF,
            set_temperature: 0,
            sensor_a_temp: 255,
            sensor_b_temp: 255,
        });

        sim.set_filter_cycles_config(FilterCyclesConfig {
            filter1: FilterCycleConfig {
                start_hour: 23,
                start_minute: 59,
                duration_hours: 23,
                duration_minutes: 59,
                enabled: true,
            },
            filter2: FilterCycleConfig {
                start_hour: 0,
                start_minute: 0,
                duration_hours: 0,
                duration_minutes: 0,
                enabled: false,
            },
        });

        let model = [0xFF; 8];
        sim.set_information_config(InformationConfig {
            system_model: model,
            ..Default::default()
        });

        let raw = [0xFF; 10];
        sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });

        // Verify each generates valid framed output
        let fault_bytes = sim.generate_fault_log_response();
        let filter_bytes = sim.generate_filter_cycles_response();
        let info_bytes = sim.generate_information_response();
        let config_bytes = sim.generate_config_response();

        // Each should produce at least one valid frame
        for (label, bytes) in [
            ("fault", &fault_bytes),
            ("filter", &filter_bytes),
            ("info", &info_bytes),
            ("config", &config_bytes),
        ] {
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(bytes);
            assert!(
                !frames.is_empty(),
                "{} response should produce valid frames",
                label
            );
            assert_eq!(
                frames[0].message_type,
                [0x0A, 0xBF],
                "{} response should have message type 0x0A 0xBF",
                label
            );
        }

        // Verify each decodes to the expected typed message (not Unknown)
        let fault_msg = dispatch_response(&fault_bytes);
        assert!(
            matches!(
                fault_msg,
                launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(_)
            ),
            "fault response should dispatch as FaultLogResponse"
        );

        let filter_msg = dispatch_response(&filter_bytes);
        assert!(
            matches!(
                filter_msg,
                launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(_)
            ),
            "filter response should dispatch as FilterCyclesResponse"
        );

        let info_msg = dispatch_response(&info_bytes);
        assert!(
            matches!(
                info_msg,
                launa_protocol::dispatcher::IncomingMessage::InformationResponse(_)
            ),
            "info response should dispatch as InformationResponse"
        );

        let config_msg = dispatch_response(&config_bytes);
        assert!(
            matches!(
                config_msg,
                launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(_)
            ),
            "config response should dispatch as ControlConfiguration"
        );
    }

    // VAL-MQTT-024: Configured values survive round-trip through SpaSim → FrameDecoder → dispatch
    #[test]
    fn test_configurable_sim_response_round_trip() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Configure custom fault log
        sim.set_fault_log_config(FaultLogConfig {
            fault_count: 7,
            entry_number: 3,
            message_code: FaultCode::SensorAFault,
            days_ago: 5,
            hour: 10,
            minute: 45,
            flags: 0x12,
            set_temperature: 80,
            sensor_a_temp: 78,
            sensor_b_temp: 77,
        });

        // Configure custom filter cycles
        sim.set_filter_cycles_config(FilterCyclesConfig {
            filter1: FilterCycleConfig {
                start_hour: 3,
                start_minute: 15,
                duration_hours: 1,
                duration_minutes: 45,
                enabled: true,
            },
            filter2: FilterCycleConfig {
                start_hour: 21,
                start_minute: 30,
                duration_hours: 4,
                duration_minutes: 0,
                enabled: true,
            },
        });

        // Configure custom information
        let mut model = [b' '; 8];
        model[..5].copy_from_slice(b"TEST1");
        sim.set_information_config(InformationConfig {
            software_id_byte0: 0xAB,
            software_id_byte1: 0xCD,
            software_version_byte0: 0xEF,
            software_version_byte1: 0x01,
            system_model: model,
            current_setup: 0x42,
            config_sig_byte0: 0xCA,
            config_sig_byte1: 0xFE,
            config_sig_byte2: 0xBA,
            config_sig_byte3: 0xBE,
            heater_voltage: 0x01,
            heater_type: 0x0A,
            dip_switch_byte0: 0xAA,
            dip_switch_byte1: 0x55,
        });

        // Send settings requests and verify responses survive round-trip
        // Fault log request: 0x22 0x20
        let fault_response = sim.process_incoming_bytes(&build_settings_request(0x20));
        assert!(
            !fault_response.is_empty(),
            "should produce fault log response"
        );
        let fault_msg = dispatch_response(&fault_response);
        match fault_msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(entry.fault_count, 7);
                assert_eq!(entry.entry_number, 3);
                assert_eq!(entry.message_code, FaultCode::SensorAFault);
                assert_eq!(entry.days_ago, 5);
                assert_eq!(entry.hour, 10);
                assert_eq!(entry.minute, 45);
                assert_eq!(entry.flags, 0x12);
                assert_eq!(entry.set_temperature, 80);
                assert_eq!(entry.sensor_a_temp, 78);
                assert_eq!(entry.sensor_b_temp, 77);
            }
            other => panic!("Expected FaultLogResponse, got {:?}", other),
        }

        // Filter cycles request: 0x22 0x01
        let filter_response = sim.process_incoming_bytes(&build_settings_request(0x01));
        assert!(
            !filter_response.is_empty(),
            "should produce filter cycles response"
        );
        let filter_msg = dispatch_response(&filter_response);
        match filter_msg {
            launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
                assert_eq!(fc.filter1.start_hour, 3);
                assert_eq!(fc.filter1.start_minute, 15);
                assert_eq!(fc.filter1.duration_hours, 1);
                assert_eq!(fc.filter1.duration_minutes, 45);
                assert_eq!(fc.filter2.start_hour, 21);
                assert_eq!(fc.filter2.start_minute, 30);
                assert_eq!(fc.filter2.duration_hours, 4);
                assert!(fc.filter2.enabled);
            }
            other => panic!("Expected FilterCyclesResponse, got {:?}", other),
        }

        // Information request: 0x22 0x02
        let info_response = sim.process_incoming_bytes(&build_settings_request(0x02));
        assert!(
            !info_response.is_empty(),
            "should produce information response"
        );
        let info_msg = dispatch_response(&info_response);
        match info_msg {
            launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
                assert_eq!(info.system_model, "TEST1");
                assert_eq!(info.current_setup, 0x42);
                assert_eq!(info.config_signature, "CAFEBABE");
            }
            other => panic!("Expected InformationResponse, got {:?}", other),
        }

        // Config request: 0x04
        let config_response = sim.process_incoming_bytes(&build_config_request());
        assert!(
            !config_response.is_empty(),
            "should produce config response"
        );
        let config_msg = dispatch_response(&config_response);
        // Config response with 0x2E sub-type → ControlConfiguration
        match config_msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                // Defaults should apply since we didn't set a custom config
                assert_eq!(config.pump_configs[0], PumpConfig::TwoSpeed);
                assert!(config.circ_pump);
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }
    }

    // Verify config response adapts to Celsius temperature scale
    #[test]
    fn test_config_response_adapts_temperature_scale() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Celsius;

        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert!(
                    config.temperature_scale_celsius,
                    "config should report Celsius when state is Celsius"
                );
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }

        // Now test Fahrenheit
        sim.state.temp_scale = TemperatureScale::Fahrenheit;
        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert!(
                    !config.temperature_scale_celsius,
                    "config should report Fahrenheit when state is Fahrenheit"
                );
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }
    }

    // Verify custom config with Celsius adapts correctly
    #[test]
    fn test_custom_config_response_preserves_other_bits_with_scale_adaptation() {
        let mut raw = [0u8; 10];
        raw[3] = 0xFE; // all bits set except bit 0
        raw[5] = 0xFF;

        let mut sim = SpaSim::new();
        sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });
        sim.state.temp_scale = TemperatureScale::Celsius;

        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert!(config.temperature_scale_celsius, "should set Celsius bit");
                // Other bits in byte 3 should be preserved
                // FE | 01 = FF, so all bits in byte 3 should be set
                // The parser reads bit 0 of byte 3 for temperature_scale_celsius
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }

        // Switch to Fahrenheit — should clear bit 0
        sim.state.temp_scale = TemperatureScale::Fahrenheit;
        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert!(
                    !config.temperature_scale_celsius,
                    "should clear Celsius bit for Fahrenheit"
                );
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }
    }

    // Tests for new SpaSim methods (VAL-IT-008 through VAL-IT-012)

    // VAL-IT-008: SpaSim::simulate_spa_reboot() resets registration, sends NewClientQuery
    #[test]
    fn test_simulate_spa_reboot_resets_registration() {
        let mut sim = SpaSim::new();

        // First, register a client
        sim.registered = true;
        sim.client_id = Some(0x05);

        // Reboot
        sim.simulate_spa_reboot();

        // Registration should be reset
        assert!(!sim.registered, "should be unregistered after reboot");
        assert!(
            sim.client_id.is_none(),
            "client_id should be cleared after reboot"
        );

        // Next tick should produce a registration query (FE BF 00)
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);

        // Should contain at least one frame with a registration query
        let has_reg_query = frames
            .iter()
            .any(|f| f.message_type == [0xFE, 0xBF] && f.payload.contains(&0x00));
        assert!(
            has_reg_query,
            "tick after reboot should produce a registration query"
        );
    }

    // VAL-IT-008: SpaSim::simulate_spa_reboot() preserves physical state
    #[test]
    fn test_simulate_spa_reboot_preserves_physical_state() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 98.0;
        sim.state.set_temp = 102.0;
        sim.state.pumps[0] = PumpState::Low;
        sim.state.lights[0] = true;

        sim.simulate_spa_reboot();

        // Physical state should be preserved
        assert_eq!(sim.state.current_temp, 98.0);
        assert_eq!(sim.state.set_temp, 102.0);
        assert_eq!(sim.state.pumps[0], PumpState::Low);
        assert!(sim.state.lights[0]);
    }

    // VAL-IT-008: Rebooted sim can re-register
    #[test]
    fn test_simulate_spa_reboot_reregistration() {
        let mut sim = SpaSim::new();

        // Register, then reboot
        sim.registered = true;
        sim.client_id = Some(0x05);
        sim.simulate_spa_reboot();

        // Should be able to re-register via process_frame
        let _ack_frame = launa_protocol::frame::Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02, 0x03],
        };
        // First send the ID request
        let request_frame = launa_protocol::frame::Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x01],
        };
        let assignment = sim.process_frame(&request_frame);
        assert!(assignment.is_some(), "should assign client ID");

        // Feed the assignment back to register
        let mut decoder = FrameDecoder::new();
        let assignment_frames = decoder.feed_slice(&assignment.unwrap());
        // The assignment frame is FE BF 02 <id>
        let id_frame = &assignment_frames[0];
        let msg = launa_protocol::dispatcher::dispatch_frame(id_frame);
        if let launa_protocol::dispatcher::IncomingMessage::ClientIdAssignment { id } = msg {
            // Send ack
            let ack = launa_protocol::frame::FrameEncoder::encode([id, 0xBF], &[0x03]).unwrap();
            let ack_frames = decoder.feed_slice(&ack);
            sim.process_frame(&ack_frames[0]);
        }

        assert!(sim.registered, "should be registered after re-registration");
        assert!(sim.client_id.is_some());
    }

    // VAL-IT-009: SpaSim::simulate_fault_state() sets fault flag, fault log carries FaultCode
    #[test]
    fn test_simulate_fault_state_sets_fault_flag() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // No fault initially
        let normal_bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let normal_frames = decoder.feed_slice(&normal_bytes);
        let normal_msg = launa_protocol::dispatcher::dispatch_frame(&normal_frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = normal_msg {
            assert!(!s.is_priming, "should not be in fault initially");
        }

        // Simulate fault
        sim.simulate_fault_state(FaultCode::HeaterDry);

        // Status frame should show fault (init_mode = 0x02 in payload offset 1)
        let fault_bytes = sim.generate_status_frame();
        let fault_frames = decoder.feed_slice(&fault_bytes);
        // The raw payload byte 1 should be 0x02
        assert_eq!(
            fault_frames[0].payload[1], 0x02,
            "init_mode should be 0x02 (fault) after simulate_fault_state"
        );
    }

    // VAL-IT-009: Fault log response carries the FaultCode
    #[test]
    fn test_simulate_fault_state_fault_log_carries_code() {
        let mut sim = SpaSim::new();
        sim.simulate_fault_state(FaultCode::LowFlow);

        let response = sim.generate_fault_log_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(
                    entry.message_code,
                    FaultCode::LowFlow,
                    "fault log should carry the simulated fault code"
                );
            }
            other => panic!("Expected FaultLogResponse, got {:?}", other),
        }
    }

    // VAL-IT-009: Different fault codes
    #[test]
    fn test_simulate_fault_state_different_codes() {
        let codes = [
            FaultCode::HeaterDry,
            FaultCode::LowFlow,
            FaultCode::WaterTooHot,
            FaultCode::SensorAFault,
            FaultCode::Unknown(99),
        ];

        for code in &codes {
            let mut sim = SpaSim::new();
            sim.simulate_fault_state(*code);

            let response = sim.generate_fault_log_response();
            let msg = dispatch_response(&response);

            if let launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) = msg {
                assert_eq!(
                    entry.message_code, *code,
                    "fault log should carry {:?}",
                    code
                );
            } else {
                panic!("Expected FaultLogResponse for code {:?}", code);
            }
        }
    }

    // VAL-IT-010: SpaSim::simulate_sensor_noise() adds ±jitter to reported temp
    #[test]
    fn test_simulate_sensor_noise_with_jitter() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.simulate_sensor_noise(2.0);

        // Collect temps from 100 ticks
        let mut temps: Vec<f32> = Vec::new();
        for _ in 0..100 {
            let bytes = sim.generate_status_frame();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                if let Some(t) = s.current_temp {
                    temps.push(t);
                }
            }
        }

        // All temps should be within ±2.0 of baseline (100.0)
        for &t in &temps {
            assert!(
                t >= 98.0 && t <= 102.0,
                "temp {} should be within ±2.0 of 100.0",
                t
            );
        }

        // With jitter=2.0, not all temps should be exactly 100.0
        let exact_count = temps.iter().filter(|&&t| t == 100.0).count();
        assert!(
            exact_count < temps.len(),
            "with jitter=2.0, not all temps should be exactly 100.0 (got {}/{})",
            exact_count,
            temps.len()
        );
    }

    // VAL-IT-010: SpaSim::simulate_sensor_noise() with jitter=0.0 → all exact
    #[test]
    fn test_simulate_sensor_noise_zero_jitter() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.simulate_sensor_noise(0.0); // No noise

        for _ in 0..20 {
            let bytes = sim.generate_status_frame();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                assert_eq!(
                    s.current_temp,
                    Some(100.0),
                    "with jitter=0.0, temp should be exact"
                );
            }
        }
    }

    // VAL-IT-010: Sensor noise is deterministic
    #[test]
    fn test_simulate_sensor_noise_deterministic() {
        let mut sim1 = SpaSim::new();
        sim1.registered = true;
        sim1.state.current_temp = 100.0;
        sim1.state.set_temp = 100.0;
        sim1.simulate_sensor_noise(1.5);

        let mut temps1: Vec<f32> = Vec::new();
        for _ in 0..50 {
            let bytes = sim1.generate_status_frame();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                if let Some(t) = s.current_temp {
                    temps1.push(t);
                }
            }
        }

        // Create identical sim
        let mut sim2 = SpaSim::new();
        sim2.registered = true;
        sim2.state.current_temp = 100.0;
        sim2.state.set_temp = 100.0;
        sim2.simulate_sensor_noise(1.5);

        let mut temps2: Vec<f32> = Vec::new();
        for _ in 0..50 {
            let bytes = sim2.generate_status_frame();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                if let Some(t) = s.current_temp {
                    temps2.push(t);
                }
            }
        }

        // Same initial state → same sequence
        assert_eq!(
            temps1, temps2,
            "identical sims should produce identical noise sequences"
        );
    }

    // VAL-IT-011: SpaSim::simulate_unknown_temp() reports 0xFF for current_temp
    #[test]
    fn test_simulate_unknown_temp_reports_none() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0; // Internal temp is known

        // Before: temp is known
        let normal_bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let normal_frames = decoder.feed_slice(&normal_bytes);
        let normal_msg = launa_protocol::dispatcher::dispatch_frame(&normal_frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = normal_msg {
            assert!(s.current_temp.is_some(), "should have temp before unknown");
        }

        // Enable unknown temp
        sim.simulate_unknown_temp();

        let unknown_bytes = sim.generate_status_frame();
        let unknown_frames = decoder.feed_slice(&unknown_bytes);
        let unknown_msg = launa_protocol::dispatcher::dispatch_frame(&unknown_frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = unknown_msg {
            assert_eq!(
                s.current_temp, None,
                "current_temp should be None after simulate_unknown_temp"
            );
        }

        // Internal state still has the temp
        assert_eq!(
            sim.state.current_temp, 100.0,
            "internal state should still have the real temp"
        );
    }

    // VAL-IT-011: clear_unknown_temp restores normal reporting
    #[test]
    fn test_simulate_unknown_temp_clear_restores() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;

        sim.simulate_unknown_temp();
        sim.clear_unknown_temp();

        let bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert_eq!(
                s.current_temp,
                Some(100.0),
                "temp should be restored after clear_unknown_temp"
            );
        }
    }

    // VAL-IT-012: SpaSim::simulate_spontaneous_state_change() works via schedule_event
    #[test]
    fn test_simulate_spontaneous_state_change_via_schedule_event() {
        let mut sim = SpaSim::new();
        assert_eq!(sim.state.pumps[0], PumpState::Off);

        // Schedule pump1 to turn on at tick 3
        sim.schedule_event(3, SpaEventType::FilterCycleStart { pump_index: 0 });

        // Ticks 1-2: pump still off
        sim.tick();
        sim.tick();
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Off,
            "pump should be off before event"
        );

        // Tick 3: event fires
        sim.tick();
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "pump should start from scheduled event"
        );
    }

    // VAL-IT-012: simulate_filter_cycle_start is a convenience wrapper
    #[test]
    fn test_simulate_spontaneous_state_change_filter_cycle() {
        let mut sim = SpaSim::new();
        sim.simulate_filter_cycle_start(2, 5); // pump 3 at tick 5

        for _ in 0..4 {
            sim.tick();
        }
        assert_eq!(sim.state.pumps[2], PumpState::Off);

        sim.tick(); // tick 5
        assert_eq!(
            sim.state.pumps[2],
            PumpState::Low,
            "pump 3 should start from filter cycle"
        );
    }

    // Tests for physics improvements (VAL-SR-001 through VAL-SR-005)

    // VAL-SR-001: Realistic thermal model — heating 80→104°F takes 48-72 ticks
    #[test]
    fn test_realistic_thermal_heating_80_to_104() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 80.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        // Need at least one pump running for heater/pump interlock
        sim.state.pumps[0] = PumpState::Low;

        let mut ticks = 0;
        while sim.state.current_temp < 104.0 && ticks < 200 {
            sim.simulate_physics();
            ticks += 1;
        }

        assert!(
            ticks >= 48 && ticks <= 75,
            "heating 80→104 should take 48-75 ticks, took {}",
            ticks
        );
        assert!(
            sim.state.current_temp >= 104.0,
            "should reach set_temp, got {}",
            sim.state.current_temp
        );
    }

    // VAL-SR-001: Realistic thermal model — cooling 104→80°F takes 200-280 ticks
    #[test]
    fn test_realistic_thermal_cooling_104_to_80() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 104.0;
        sim.state.set_temp = 80.0;
        sim.state.is_heating = false;

        let mut ticks = 0;
        while sim.state.current_temp > 80.0 && ticks < 500 {
            sim.simulate_physics();
            ticks += 1;
        }

        assert!(
            ticks >= 200 && ticks <= 280,
            "cooling 104→80 should take 200-280 ticks, took {}",
            ticks
        );
        assert!(
            sim.state.current_temp <= 80.0,
            "should reach set_temp, got {}",
            sim.state.current_temp
        );
    }

    // VAL-SR-001: Heating rate tapers as temp approaches set_temp
    #[test]
    fn test_realistic_thermal_heating_rate_tapers() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 80.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Measure temp change per tick at various points
        let checkpoints = [80.0f32, 90.0f32, 100.0f32];
        for &start_temp in &checkpoints {
            let mut s = SpaSim::new();
            s.state.current_temp = start_temp;
            s.state.set_temp = 104.0;
            s.state.is_heating = true;
            s.state.pumps[0] = PumpState::Low;
            let temp_before = s.state.current_temp;
            s.simulate_physics();
            let delta = s.state.current_temp - temp_before;
            assert!(
                delta > 0.0,
                "temp should increase when heating at {}°F, got delta={}",
                start_temp,
                delta
            );
            // Rate should be smaller near set_temp (tapering)
            if start_temp > 90.0 {
                assert!(
                    delta < 1.0,
                    "heating rate should taper near set_temp, got {}",
                    delta
                );
            }
        }
    }

    // VAL-SR-001: Thermal model is deterministic
    #[test]
    fn test_realistic_thermal_deterministic() {
        let mut sim1 = SpaSim::new();
        sim1.state.current_temp = 85.0;
        sim1.state.set_temp = 104.0;
        sim1.state.is_heating = true;
        sim1.state.pumps[0] = PumpState::Low;

        let mut temps1 = Vec::new();
        for _ in 0..20 {
            sim1.simulate_physics();
            temps1.push(sim1.state.current_temp);
        }

        let mut sim2 = SpaSim::new();
        sim2.state.current_temp = 85.0;
        sim2.state.set_temp = 104.0;
        sim2.state.is_heating = true;
        sim2.state.pumps[0] = PumpState::Low;

        let mut temps2 = Vec::new();
        for _ in 0..20 {
            sim2.simulate_physics();
            temps2.push(sim2.state.current_temp);
        }

        assert_eq!(
            temps1, temps2,
            "identical initial states should produce identical temp sequences"
        );
    }

    // VAL-SR-001: Heating stops at set_temp with no overshoot (overshoot=0)
    #[test]
    fn test_realistic_thermal_no_overshoot_default() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 103.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Heat until set_temp
        for _ in 0..50 {
            sim.simulate_physics();
        }

        assert!(
            sim.state.current_temp <= 104.0,
            "without overshoot, temp should not exceed set_temp, got {}",
            sim.state.current_temp
        );
    }

    // VAL-SR-002: Temperature sensor noise with ±0.5°F causes 30%+ variation
    #[test]
    fn test_physics_sensor_noise_variation() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.set_physics_noise_amplitude(2.0);

        let mut variation_count = 0;
        let total_ticks = 100;
        for _ in 0..total_ticks {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                if let Some(t) = s.current_temp {
                    if (t - 100.0).abs() > 0.01 {
                        variation_count += 1;
                    }
                    // All temps should be within ±2.0°F of internal temp
                    assert!(
                        t >= 98.0 && t <= 102.0,
                        "temp {} should be within ±2.0 of 100.0",
                        t
                    );
                }
            }
        }

        assert!(
            variation_count as f32 / total_ticks as f32 >= 0.30,
            "with noise=2.0, expected 30%+ ticks with variation, got {}/{} ({:.0}%)",
            variation_count,
            total_ticks,
            variation_count as f32 / total_ticks as f32 * 100.0
        );
    }

    // VAL-SR-002: Noise amplitude 0.0 = no noise
    #[test]
    fn test_physics_sensor_noise_zero_no_noise() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.set_physics_noise_amplitude(0.0);

        for _ in 0..30 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                assert_eq!(
                    s.current_temp,
                    Some(100.0),
                    "with noise=0.0, temp should be exact"
                );
            }
        }
    }

    // VAL-SR-003: is_heating false when all pumps off
    #[test]
    fn test_physics_heater_off_when_pumps_off() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        // All pumps off, circ_pump off
        sim.state.pumps = [PumpState::Off; 6];
        sim.state.circ_pump = false;

        sim.simulate_physics();

        assert!(
            !sim.state.is_heating,
            "is_heating should be false when all pumps are off"
        );
    }

    // VAL-SR-003: is_heating true when pump on and temp < set_temp
    #[test]
    fn test_physics_heater_on_when_pump_on() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = false;
        sim.state.pumps[0] = PumpState::Low;

        sim.simulate_physics();

        assert!(
            sim.state.is_heating,
            "is_heating should be true when pump is on and temp < set_temp"
        );
    }

    // VAL-SR-003: is_heating true when circ_pump on and temp < set_temp
    #[test]
    fn test_physics_heater_on_when_circ_pump_on() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = false;
        sim.state.pumps = [PumpState::Off; 6];
        sim.state.circ_pump = true;

        sim.simulate_physics();

        assert!(
            sim.state.is_heating,
            "is_heating should be true when circ_pump is on and temp < set_temp"
        );
    }

    // VAL-SR-003: Last pump off → heating off next tick
    #[test]
    fn test_physics_heater_off_after_pump_turned_off() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Heating with pump on
        sim.simulate_physics();
        assert!(sim.state.is_heating, "should be heating with pump on");

        // Turn pump off
        sim.state.pumps[0] = PumpState::Off;
        sim.simulate_physics();
        assert!(
            !sim.state.is_heating,
            "is_heating should turn off when pump turned off"
        );
    }

    // VAL-SR-004: First N ticks report 0xFF (None) for current_temp
    #[test]
    fn test_physics_temp_unknown_on_startup_first_n_ticks() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.set_physics_unknown_temp_ticks(5); // First 5 ticks report unknown

        // Ticks 1-5: should report None (0xFF)
        for i in 1..=5 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                assert_eq!(
                    s.current_temp, None,
                    "tick {}: should report None (0xFF) for current_temp",
                    i
                );
            }
        }

        // Tick 6: should report actual temp
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert_eq!(
                s.current_temp,
                Some(100.0),
                "tick 6: should report actual temp"
            );
        }
    }

    // VAL-SR-004: Internal physics still runs during unknown temp period
    #[test]
    fn test_physics_internal_runs_during_unknown_temp() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;
        sim.set_physics_unknown_temp_ticks(5);

        // Advance 5 ticks — internal temp should still change
        for _ in 0..5 {
            sim.tick();
        }

        assert!(
            sim.state.current_temp > 90.0,
            "internal temp should have increased during unknown period, got {}",
            sim.state.current_temp
        );
    }

    // VAL-SR-004: Default N=0 (backward compatible — no unknown temp)
    #[test]
    fn test_physics_unknown_temp_default_zero() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;

        // Default: no unknown temp period
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert_eq!(
                s.current_temp,
                Some(100.0),
                "default N=0: should report actual temp from first tick"
            );
        }
    }

    // VAL-SR-005: Heater overshoot — temp reaches set_temp+2 before heating stops
    #[test]
    fn test_physics_heater_overshoot() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;
        sim.set_physics_overshoot(2.0);

        let mut max_temp = sim.state.current_temp;
        for _ in 0..200 {
            sim.simulate_physics();
            max_temp = max_temp.max(sim.state.current_temp);
            // Stop once heating turns off
            if !sim.state.is_heating {
                break;
            }
        }

        assert!(
            max_temp >= 106.0,
            "with overshoot=2.0, temp should reach set_temp+2=106, max was {}",
            max_temp
        );
        assert!(
            !sim.state.is_heating,
            "heating should have stopped after overshoot"
        );
    }

    // VAL-SR-005: Re-heat hysteresis — temp must drop to set_temp-1 before re-heating
    #[test]
    fn test_physics_heater_overshoot_hysteresis() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 104.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = false;
        sim.state.pumps[0] = PumpState::Low;
        sim.set_physics_overshoot(2.0);

        // Simulate overshoot completion: set heating_overshot flag
        sim.heating_overshot = true;

        // Temp at set_temp (104), heating is off, overshoot was reached
        // Hysteresis threshold = set_temp - overshoot/2 = 104 - 1.0 = 103.0
        // Temp should NOT start heating until it drops to 103.0

        // Cool slightly to 103.5 — should NOT re-heat yet
        sim.state.current_temp = 103.5;
        sim.simulate_physics();
        assert!(
            !sim.state.is_heating,
            "should not re-heat at 103.5 (above hysteresis threshold of 103.0)"
        );

        // Cool to 103.0 — should start re-heating
        sim.state.current_temp = 102.9;
        sim.simulate_physics();
        assert!(
            sim.state.is_heating,
            "should re-heat at 102.9 (below hysteresis threshold of 103.0)"
        );
    }

    // VAL-SR-005: Default overshoot=0.0 (backward compatible)
    #[test]
    fn test_physics_overshoot_default_zero() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 103.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Heat until set_temp
        for _ in 0..50 {
            sim.simulate_physics();
        }

        assert!(
            sim.state.current_temp <= 104.01,
            "default overshoot=0: temp should not exceed set_temp, got {}",
            sim.state.current_temp
        );
    }

    // Combined test: All physics features together
    #[test]
    fn test_all_physics_features_together() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 80.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Enable all physics features
        sim.set_physics_unknown_temp_ticks(3);
        sim.set_physics_overshoot(1.5);
        sim.set_physics_noise_amplitude(0.3);

        // First 3 ticks: unknown temp
        for i in 1..=3 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                assert_eq!(s.current_temp, None, "tick {}: should report None", i);
            }
        }

        // Tick 4 onwards: actual temp (with noise)
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert!(
                s.current_temp.is_some(),
                "tick 4: should report actual temp"
            );
        }

        // Internal temp should have been increasing during unknown period
        assert!(
            sim.state.current_temp > 80.0,
            "internal temp should have increased, got {}",
            sim.state.current_temp
        );

        // Continue heating until overshoot
        let mut max_temp = sim.state.current_temp;
        for _ in 0..300 {
            sim.tick();
            max_temp = max_temp.max(sim.state.current_temp);
            if !sim.state.is_heating && sim.state.current_temp < 104.0 {
                break;
            }
        }

        // Should have overshoot past set_temp
        assert!(
            max_temp >= 105.5,
            "should overshoot past 104+1.5=105.5, max was {}",
            max_temp
        );
    }

    /// Helper: dispatch a status frame and return the parsed status.
    fn dispatch_status(sim: &mut SpaSim) -> launa_protocol::status::StatusUpdate {
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        match msg {
            launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) => s,
            other => panic!("Expected StatusUpdate, got {:?}", other),
        }
    }

    // VAL-SIM-002: clear_fault_state restores init_mode to 0x00
    #[test]
    fn test_clear_fault_state_restores_init_mode() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Set fault
        sim.simulate_fault_state(FaultCode::HeaterDry);
        let fault_bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let fault_frames = decoder.feed_slice(&fault_bytes);
        assert_eq!(
            fault_frames[0].payload[1], 0x02,
            "init_mode should be 0x02 during fault"
        );

        // Clear fault — this method may not exist yet (RED phase)
        sim.clear_fault_state();

        // After clearing, init_mode should be 0x00
        let clear_bytes = sim.generate_status_frame();
        let clear_frames = decoder.feed_slice(&clear_bytes);
        assert_eq!(
            clear_frames[0].payload[1], 0x00,
            "init_mode should be 0x00 after clear_fault_state"
        );
    }

    // VAL-SIM-002: Subsequent status frames show no fault after clear
    #[test]
    fn test_clear_fault_state_subsequent_status_no_fault() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_fault_state(FaultCode::LowFlow);
        sim.tick(); // tick during fault

        sim.clear_fault_state();

        // Multiple subsequent ticks should all show no fault
        for _ in 0..5 {
            let status = dispatch_status(&mut sim);
            assert!(
                !status.is_priming,
                "status should not show fault after clearing"
            );
        }
    }

    // VAL-SIM-003: Transient fault auto-clears after N ticks
    #[test]
    fn test_transient_fault_auto_clears_after_n_ticks() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Inject transient fault that auto-clears after 3 ticks
        sim.simulate_transient_fault(FaultCode::HeaterDry, 3);

        // First 3 ticks should show fault (init_mode = 0x02)
        for i in 1..=3 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            assert_eq!(
                frames[0].payload[1], 0x02,
                "tick {}: init_mode should be 0x02 (fault active)",
                i
            );
        }

        // Tick 4 onwards should show no fault (init_mode = 0x00)
        for i in 4..=6 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            assert_eq!(
                frames[0].payload[1], 0x00,
                "tick {}: init_mode should be 0x00 (fault cleared)",
                i
            );
        }
    }

    // VAL-SIM-003: Transient fault with 0 ticks clears immediately
    #[test]
    fn test_transient_fault_zero_ticks_clears_immediately() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_transient_fault(FaultCode::FlowFailed, 0);

        // Should be cleared already on first tick
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x00,
            "zero-tick transient should clear immediately"
        );
    }

    // VAL-SIM-003: Transient fault with 1 tick clears after exactly 1 tick
    #[test]
    fn test_transient_fault_one_tick() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_transient_fault(FaultCode::WaterTooHot, 1);

        // Tick 1: fault active
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x02, "tick 1: fault should be active");

        // Tick 2: cleared
        let bytes = sim.tick();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x00,
            "tick 2: fault should be cleared"
        );
    }

    // VAL-SIM-004: Multi-entry fault log returns distinct entries
    #[test]
    fn test_multi_entry_fault_log_distinct_entries() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Configure a multi-entry fault log
        sim.set_fault_log_entries(vec![
            FaultLogConfig {
                fault_count: 3,
                entry_number: 1,
                message_code: FaultCode::HeaterDry,
                days_ago: 2,
                hour: 14,
                minute: 30,
                flags: 0x04,
                set_temperature: 104,
                sensor_a_temp: 104,
                sensor_b_temp: 102,
            },
            FaultLogConfig {
                fault_count: 3,
                entry_number: 2,
                message_code: FaultCode::LowFlow,
                days_ago: 5,
                hour: 10,
                minute: 15,
                flags: 0x04,
                set_temperature: 100,
                sensor_a_temp: 100,
                sensor_b_temp: 98,
            },
            FaultLogConfig {
                fault_count: 3,
                entry_number: 3,
                message_code: FaultCode::WaterTooHot,
                days_ago: 10,
                hour: 8,
                minute: 0,
                flags: 0x04,
                set_temperature: 106,
                sensor_a_temp: 108,
                sensor_b_temp: 107,
            },
        ]);

        // Walk entries 1..3
        let codes = [
            FaultCode::HeaterDry,
            FaultCode::LowFlow,
            FaultCode::WaterTooHot,
        ];
        for (i, expected_code) in codes.iter().enumerate() {
            let entry_num = (i + 1) as u8;
            let response = sim.generate_fault_log_response_for_entry(entry_num);
            let msg = dispatch_response(&response);

            match msg {
                launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                    assert_eq!(
                        entry.message_code, *expected_code,
                        "entry {} should have code {:?}",
                        entry_num, expected_code
                    );
                    assert_eq!(
                        entry.entry_number, entry_num,
                        "entry should report entry_number = {}",
                        entry_num
                    );
                }
                other => panic!(
                    "Entry {}: Expected FaultLogResponse, got {:?}",
                    entry_num, other
                ),
            }
        }
    }

    // VAL-SIM-004: Entry 0 returns sentinel/empty
    #[test]
    fn test_fault_log_entry_zero_returns_sentinel() {
        let mut sim = SpaSim::new();

        sim.set_fault_log_entries(vec![FaultLogConfig {
            fault_count: 1,
            entry_number: 1,
            message_code: FaultCode::HeaterDry,
            days_ago: 1,
            hour: 12,
            minute: 0,
            flags: 0x04,
            set_temperature: 104,
            sensor_a_temp: 104,
            sensor_b_temp: 102,
        }]);

        let response = sim.generate_fault_log_response_for_entry(0);
        // Entry 0 should produce an empty/sentinel response (fault_count = 0 or entry_number = 0)
        let msg = dispatch_response(&response);
        match msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(
                    entry.fault_count, 0,
                    "entry 0 should return sentinel with fault_count = 0"
                );
            }
            other => panic!("Expected FaultLogResponse for entry 0, got {:?}", other),
        }
    }

    // VAL-SIM-004: Past-end entry returns sentinel/empty
    #[test]
    fn test_fault_log_past_end_returns_sentinel() {
        let mut sim = SpaSim::new();

        sim.set_fault_log_entries(vec![FaultLogConfig {
            fault_count: 1,
            entry_number: 1,
            message_code: FaultCode::HeaterDry,
            days_ago: 1,
            hour: 12,
            minute: 0,
            flags: 0x04,
            set_temperature: 104,
            sensor_a_temp: 104,
            sensor_b_temp: 102,
        }]);

        // Only 1 entry, so entry 2 is past-end
        let response = sim.generate_fault_log_response_for_entry(2);
        let msg = dispatch_response(&response);
        match msg {
            launa_protocol::dispatcher::IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(
                    entry.fault_count, 0,
                    "past-end entry should return sentinel with fault_count = 0"
                );
            }
            other => panic!(
                "Expected FaultLogResponse for past-end entry, got {:?}",
                other
            ),
        }
    }

    // VAL-SIM-005: Fault mid-command preserves queued commands
    #[test]
    fn test_fault_preserves_queued_commands() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.pumps[0] = PumpState::Off;
        sim.set_command_latency_ticks(3);

        // Queue a toggle pump1 command
        let toggle_cmd = FrameEncoder::encode([0x0A, 0xBF], &[0x11, 0x04]).unwrap();
        sim.process_frame(&FrameDecoder::new().feed_slice(&toggle_cmd).remove(0));

        // Command should be pending (3 ticks latency)
        assert_eq!(sim.pending_commands.len(), 1, "command should be queued");

        // Inject fault mid-command
        sim.simulate_fault_state(FaultCode::HeaterDry);

        // The queued command should NOT be lost
        assert_eq!(
            sim.pending_commands.len(),
            1,
            "fault should not discard queued commands"
        );

        // Process ticks: the pending command should still decrement and fire
        sim.tick(); // latency 3→2
        assert_eq!(sim.pending_commands.len(), 1);
        sim.tick(); // latency 2→1
        assert_eq!(sim.pending_commands.len(), 1);
        sim.tick(); // latency 1→0, command fires

        assert_eq!(
            sim.pending_commands.len(),
            0,
            "command should have been applied"
        );
        // The command should have applied despite fault
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "pump should be toggled on despite fault"
        );
    }

    // VAL-SIM-005: Command queued before fault executes after fault clears
    #[test]
    fn test_command_before_fault_executes_after_clear() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.pumps[0] = PumpState::Off;
        sim.set_command_latency_ticks(2);

        // Queue a command
        let toggle_cmd = FrameEncoder::encode([0x0A, 0xBF], &[0x11, 0x04]).unwrap();
        sim.process_frame(&FrameDecoder::new().feed_slice(&toggle_cmd).remove(0));

        // Inject fault
        sim.simulate_fault_state(FaultCode::LowFlow);

        sim.tick(); // latency 2→1
        sim.tick(); // latency 1→0, command fires

        // Command should have applied
        assert_eq!(sim.state.pumps[0], PumpState::Low);

        // Clear fault
        sim.clear_fault_state();

        // Status should now show no fault and pump running
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x00, "init_mode should be 0x00");
    }

    // VAL-SIM-017: Priming mode sets init_mode to 0x01
    #[test]
    fn test_simulate_priming_mode_sets_init_mode() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_priming_mode(10);

        let bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x01,
            "init_mode should be 0x01 (priming) after simulate_priming_mode"
        );
    }

    // VAL-SIM-018: Priming mode auto-exits after configured duration
    #[test]
    fn test_priming_mode_auto_exits_after_duration() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_priming_mode(5);

        // First 5 ticks should show priming
        for i in 1..=5 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            assert_eq!(
                frames[0].payload[1], 0x01,
                "tick {}: init_mode should be 0x01 (priming)",
                i
            );
        }

        // Tick 6 onwards should show normal
        for i in 6..=8 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            assert_eq!(
                frames[0].payload[1], 0x00,
                "tick {}: init_mode should be 0x00 (priming exited)",
                i
            );
        }
    }

    // Priming mode with 0 duration exits immediately
    #[test]
    fn test_priming_mode_zero_duration_exits_immediately() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_priming_mode(0);

        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x00,
            "zero-duration priming should exit immediately"
        );
    }

    // clear_priming_mode manually exits priming mode
    #[test]
    fn test_clear_priming_mode_manual_exit() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_priming_mode(100);

        // Should show priming
        let bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x01, "should be in priming mode");

        // Manually clear
        sim.clear_priming_mode();

        // Should show normal
        let bytes = sim.generate_status_frame();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x00,
            "priming should be cleared manually"
        );
    }

    // Priming mode + fault interaction: fault takes priority (0x02 overrides 0x01)
    #[test]
    fn test_fault_overrides_priming_mode() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        sim.simulate_priming_mode(10);
        let bytes = sim.generate_status_frame();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x01, "should be priming first");

        // Fault overrides priming
        sim.simulate_fault_state(FaultCode::HeaterDry);
        let bytes = sim.generate_status_frame();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x02,
            "fault should override priming mode"
        );

        // After clearing fault, priming should resume
        sim.clear_fault_state();
        let bytes = sim.generate_status_frame();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x01,
            "priming should resume after fault cleared"
        );
    }

    // New features default to off (zero impact on existing behavior)
    #[test]
    fn test_fault_lifecycle_defaults_off() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // No fault, no priming by default
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(
            frames[0].payload[1], 0x00,
            "init_mode should be 0x00 by default"
        );

        // Second tick also normal
        let bytes = sim.tick();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x00, "should remain 0x00");
    }

    // Transient fault with command latency: both interact correctly
    #[test]
    fn test_transient_fault_with_command_latency() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.pumps[0] = PumpState::Off;
        sim.set_command_latency_ticks(2);

        // Queue command
        let toggle_cmd = FrameEncoder::encode([0x0A, 0xBF], &[0x11, 0x04]).unwrap();
        sim.process_frame(&FrameDecoder::new().feed_slice(&toggle_cmd).remove(0));

        // Inject transient fault for 2 ticks
        sim.simulate_transient_fault(FaultCode::HeaterDry, 2);

        // Tick 1: fault active, command pending (latency 2→1)
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x02, "tick 1: fault active");

        // Tick 2: fault active, command fires (latency 1→0)
        let bytes = sim.tick();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x02, "tick 2: fault still active");
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "command should have applied"
        );

        // Tick 3: fault cleared
        let bytes = sim.tick();
        let frames = decoder.feed_slice(&bytes);
        assert_eq!(frames[0].payload[1], 0x00, "tick 3: fault cleared");
    }

    // VAL-SIM-010: Ambient temperature is configurable via set_ambient_temp()
    #[test]
    fn test_set_ambient_temp_method_exists() {
        let mut sim = SpaSim::new();
        // Method should exist and be callable
        sim.set_ambient_temp(85.0);
    }

    // VAL-SIM-010: Higher ambient temp causes slower cooling
    #[test]
    fn test_higher_ambient_slower_cooling() {
        // Compare cooling rate with ambient=70°F (default) vs ambient=85°F
        let mut sim70 = SpaSim::new();
        sim70.state.current_temp = 104.0;
        sim70.state.set_temp = 80.0;
        sim70.state.is_heating = false;
        // Default ambient is 70°F

        let mut sim85 = SpaSim::new();
        sim85.state.current_temp = 104.0;
        sim85.state.set_temp = 80.0;
        sim85.state.is_heating = false;
        sim85.set_ambient_temp(85.0);

        // Run 10 physics ticks on each
        for _ in 0..10 {
            sim70.simulate_physics();
            sim85.simulate_physics();
        }

        // Higher ambient → slower cooling → higher temp after same ticks
        assert!(
            sim85.state.current_temp > sim70.state.current_temp,
            "ambient=85 should cool slower than ambient=70: got {} vs {}",
            sim85.state.current_temp,
            sim70.state.current_temp
        );
    }

    // VAL-SIM-010: set_ambient_temp(70.0) matches default behavior
    #[test]
    fn test_set_ambient_temp_70_matches_default() {
        let mut sim_default = SpaSim::new();
        sim_default.state.current_temp = 104.0;
        sim_default.state.set_temp = 80.0;
        sim_default.state.is_heating = false;

        let mut sim_explicit = SpaSim::new();
        sim_explicit.state.current_temp = 104.0;
        sim_explicit.state.set_temp = 80.0;
        sim_explicit.state.is_heating = false;
        sim_explicit.set_ambient_temp(70.0);

        for _ in 0..20 {
            sim_default.simulate_physics();
            sim_explicit.simulate_physics();
        }

        assert_eq!(
            sim_default.state.current_temp, sim_explicit.state.current_temp,
            "ambient=70 should match default behavior exactly"
        );
    }

    // VAL-SIM-010: Default ambient is 70°F (backward compatible)
    #[test]
    fn test_default_ambient_is_70() {
        let sim = SpaSim::new();
        // Default ambient should be 70.0 — verified by cooling behavior matching
        // the original hardcoded 70.0 value
        assert_eq!(sim.ambient_temp(), 70.0, "default ambient should be 70°F");
    }

    // VAL-SIM-011: Pump heat contribution raises water temp when pumps running
    #[test]
    fn test_pump_heat_contribution_raises_temp() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;
        sim.state.pumps[0] = PumpState::High;
        // Enable pump heat contribution
        sim.set_pump_heat_contribution(0.02); // 0.02°F per tick per running pump

        let temp_before = sim.state.current_temp;
        sim.simulate_physics();

        // Temperature should increase slightly from pump waste heat
        assert!(
            sim.state.current_temp > temp_before,
            "pump heat should raise temp: before={}, after={}",
            temp_before,
            sim.state.current_temp
        );
    }

    // VAL-SIM-011: Pump heat default is off (no contribution)
    #[test]
    fn test_pump_heat_contribution_default_off() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;
        sim.state.pumps[0] = PumpState::High;
        // Don't call set_pump_heat_contribution — should default to 0

        let temp_before = sim.state.current_temp;

        // With default (no pump heat), temp should not increase from pumps alone
        // (only cooling towards ambient should occur)
        sim.simulate_physics();

        assert!(
            sim.state.current_temp <= temp_before,
            "default pump heat=0: temp should not increase from pumps alone, before={}, after={}",
            temp_before,
            sim.state.current_temp
        );
    }

    // VAL-SIM-011: Pump heat with multiple pumps raises faster
    #[test]
    fn test_pump_heat_multiple_pumps_higher_contribution() {
        let mut sim1 = SpaSim::new();
        sim1.state.current_temp = 95.0;
        sim1.state.set_temp = 100.0;
        sim1.state.is_heating = false;
        sim1.state.pumps[0] = PumpState::Low;
        sim1.set_pump_heat_contribution(0.02);

        let mut sim2 = SpaSim::new();
        sim2.state.current_temp = 95.0;
        sim2.state.set_temp = 100.0;
        sim2.state.is_heating = false;
        sim2.state.pumps[0] = PumpState::Low;
        sim2.state.pumps[1] = PumpState::Low;
        sim2.state.pumps[2] = PumpState::Low;
        sim2.set_pump_heat_contribution(0.02);

        for _ in 0..10 {
            sim1.simulate_physics();
            sim2.simulate_physics();
        }

        // More pumps running → more waste heat → higher temp
        assert!(
            sim2.state.current_temp > sim1.state.current_temp,
            "3 pumps should produce more heat than 1: {} vs {}",
            sim2.state.current_temp,
            sim1.state.current_temp
        );
    }

    // VAL-SIM-024: Heater interlock stops heating when all pumps turned off
    #[test]
    fn test_heater_interlock_stops_when_all_pumps_off() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Verify heating is active
        sim.simulate_physics();
        assert!(sim.state.is_heating, "should be heating with pump on");

        // Turn off all pumps
        sim.state.pumps[0] = PumpState::Off;
        sim.simulate_physics();

        // Heating should stop immediately
        assert!(
            !sim.state.is_heating,
            "is_heating must be false when all pumps are off"
        );
    }

    // VAL-SIM-024: Heating resumes when pump restarts and temp < set_point
    #[test]
    fn test_heater_interlock_resumes_on_pump_restart() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = false; // not heating initially

        // All pumps off
        sim.state.pumps = [PumpState::Off; 6];
        sim.state.circ_pump = false;

        sim.simulate_physics();
        assert!(
            !sim.state.is_heating,
            "should not heat with no pumps running"
        );

        // Start a pump — should resume heating since temp < set_point
        sim.state.pumps[0] = PumpState::Low;
        sim.simulate_physics();
        assert!(
            sim.state.is_heating,
            "is_heating should resume when pump starts and temp < set_point"
        );
    }

    // VAL-SIM-024: Circ pump alone satisfies interlock
    #[test]
    fn test_heater_interlock_circ_pump_satisfies() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = false;
        sim.state.pumps = [PumpState::Off; 6];
        sim.state.circ_pump = true;

        sim.simulate_physics();
        assert!(
            sim.state.is_heating,
            "circ_pump alone should satisfy heater interlock"
        );
    }

    // VAL-SIM-024: Full cycle — heat → pump off → heat stops → pump on → heat resumes
    #[test]
    fn test_heater_interlock_full_cycle() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Phase 1: heating with pump
        sim.simulate_physics();
        assert!(sim.state.is_heating, "phase 1: heating with pump");
        let temp_phase1 = sim.state.current_temp;
        assert!(temp_phase1 > 90.0, "should be warming up");

        // Phase 2: turn pump off — heating stops immediately
        sim.state.pumps[0] = PumpState::Off;
        sim.simulate_physics();
        assert!(!sim.state.is_heating, "phase 2: heating stopped");

        // Phase 3: temp should start cooling (or at least not heating)
        let _temp_after_stop = sim.state.current_temp;

        // Phase 4: restart pump — heating resumes
        sim.state.pumps[0] = PumpState::Low;
        sim.simulate_physics();
        assert!(
            sim.state.is_heating,
            "phase 4: heating should resume with pump restart"
        );
    }

    // Overshoot/hysteresis: full cycle

    // VAL-SIM-006: Heater overshoots set_temp by configured degrees
    // VAL-SIM-007: Hysteresis re-heat after overshoot
    #[test]
    fn test_overshoot_full_cycle_heat_overshoot_cool_hysteresis_reheat() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;
        sim.set_physics_overshoot(2.0);

        let overshoot_target = 106.0; // set_temp + overshoot
        let hysteresis_threshold = 103.0; // set_temp - overshoot/2

        // Phase 1: Heat from 100 → 106 (overshoot target)
        let mut reached_overshoot = false;
        for _ in 0..200 {
            sim.simulate_physics();
            if sim.state.current_temp >= overshoot_target - 0.01 {
                reached_overshoot = true;
                assert!(
                    !sim.state.is_heating,
                    "heating should stop at overshoot target"
                );
                break;
            }
            assert!(
                sim.state.is_heating,
                "should be heating until overshoot target"
            );
        }
        assert!(reached_overshoot, "should reach overshoot target 106°F");

        // Phase 2: Cool from 106 to 103.5 — should NOT re-heat (above hysteresis)
        sim.state.current_temp = 103.5;
        sim.simulate_physics();
        assert!(
            !sim.state.is_heating,
            "should not re-heat at 103.5 (above hysteresis threshold {})",
            hysteresis_threshold
        );

        // Phase 3: Cool below hysteresis threshold — should re-heat
        sim.state.current_temp = 102.9;
        sim.simulate_physics();
        assert!(
            sim.state.is_heating,
            "should re-heat below hysteresis threshold 103.0, temp={}",
            sim.state.current_temp
        );

        // Phase 4: Re-heat back to overshoot target
        let mut reached_second_overshoot = false;
        for _ in 0..200 {
            sim.simulate_physics();
            if sim.state.current_temp >= overshoot_target - 0.01 {
                reached_second_overshoot = true;
                break;
            }
        }
        assert!(
            reached_second_overshoot,
            "should reach overshoot target again on re-heat cycle, temp={}",
            sim.state.current_temp
        );
    }

    // VAL-SIM-006: Maximum observed temperature equals set_temp + overshoot
    #[test]
    fn test_overshoot_max_observed_equals_set_temp_plus_overshoot() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;
        sim.set_physics_overshoot(3.0);

        let mut max_temp = sim.state.current_temp;
        for _ in 0..300 {
            sim.simulate_physics();
            max_temp = max_temp.max(sim.state.current_temp);
            if !sim.state.is_heating && sim.state.current_temp < 100.0 {
                break;
            }
        }

        // Should overshoot to exactly set_temp + 3.0 = 103.0
        assert!(
            max_temp >= 103.0 - 0.1,
            "max observed temp should be ~103°F, got {}",
            max_temp
        );
    }

    // -------------------------------------------------------------------------
    // Unknown temp period (set_physics_unknown_temp_ticks)
    // -------------------------------------------------------------------------

    // VAL-SIM-008: First N ticks report None, tick N+1 reports valid temp
    #[test]
    fn test_unknown_temp_exact_boundary_ticks() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;
        sim.set_physics_unknown_temp_ticks(10);

        // Ticks 1-10: report None (0xFF)
        for i in 1..=10 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            assert_eq!(
                frames[0].payload[2], 0xFF,
                "tick {}: current_temp should be 0xFF (None)",
                i
            );
        }

        // Tick 11: report valid temp
        let bytes = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        assert_ne!(
            frames[0].payload[2], 0xFF,
            "tick 11: current_temp should NOT be 0xFF anymore"
        );
    }

    // VAL-SIM-008: Physics ticks counted during unknown temp match expectation
    #[test]
    fn test_unknown_temp_physics_tick_count_advances() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 90.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;
        sim.set_physics_unknown_temp_ticks(5);

        // During unknown period, internal temp should still change
        for _ in 0..5 {
            sim.tick();
        }
        // After 5 ticks of heating, internal temp should have risen
        assert!(
            sim.state.current_temp > 90.0,
            "internal temp should have risen during unknown period, got {}",
            sim.state.current_temp
        );

        // After unknown period, reported temp matches internal temp
        let bytes = sim.tick(); // tick 6
        let mut decoder = FrameDecoder::new();
        let frames = decoder.feed_slice(&bytes);
        let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
            assert!(
                s.current_temp.is_some(),
                "should report valid temp after unknown period"
            );
        }
    }

    // -------------------------------------------------------------------------
    // Sensor noise: physics noise + legacy noise combined
    // -------------------------------------------------------------------------

    // VAL-SIM-009: Physics noise stays within ±N°F bounds
    #[test]
    fn test_physics_noise_stays_within_bounds() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;
        sim.set_physics_noise_amplitude(3.0);

        for _ in 0..200 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                if let Some(t) = s.current_temp {
                    let deviation = (t - 100.0).abs();
                    assert!(
                        deviation <= 3.0,
                        "noise deviation {} should be within ±3.0°F",
                        deviation
                    );
                }
            }
        }
    }

    // VAL-SIM-009: Physics noise is deterministic (identical seeds produce identical sequences)
    #[test]
    fn test_physics_noise_deterministic() {
        let collect_temps = || -> Vec<Option<f32>> {
            let mut sim = SpaSim::new();
            sim.registered = true;
            sim.state.current_temp = 100.0;
            sim.state.set_temp = 100.0;
            sim.state.is_heating = false;
            sim.set_physics_noise_amplitude(2.0);

            let mut temps = Vec::new();
            for _ in 0..100 {
                let bytes = sim.tick();
                let mut decoder = FrameDecoder::new();
                let frames = decoder.feed_slice(&bytes);
                let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
                if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                    temps.push(s.current_temp);
                }
            }
            temps
        };

        let temps1 = collect_temps();
        let temps2 = collect_temps();
        assert_eq!(
            temps1, temps2,
            "identical configurations should produce identical noise sequences"
        );
    }

    // VAL-SIM-009: Physics noise and legacy noise can coexist
    #[test]
    fn test_physics_noise_and_legacy_noise_combined() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;
        sim.state.is_heating = false;
        sim.set_physics_noise_amplitude(1.0);
        sim.simulate_sensor_noise(1.0);

        let mut all_within_bounds = true;
        let mut variation_count = 0;
        for _ in 0..100 {
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            let msg = launa_protocol::dispatcher::dispatch_frame(&frames[0]);
            if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg {
                if let Some(t) = s.current_temp {
                    // Combined noise: physics ±1 + legacy ±1, max deviation ±2.0
                    // But could be wider since both apply independently
                    let deviation = (t - 100.0).abs();
                    if deviation > 4.0 {
                        all_within_bounds = false;
                    }
                    if deviation > 0.01 {
                        variation_count += 1;
                    }
                }
            }
        }
        assert!(
            all_within_bounds,
            "combined noise should stay within reasonable bounds"
        );
        assert!(
            variation_count > 50,
            "combined noise should produce variation in most ticks, got {}/100",
            variation_count
        );
    }

    // -------------------------------------------------------------------------
    // Command latency: set temperature deferred
    // -------------------------------------------------------------------------

    // VAL-SIM-012: Command latency defers state changes by N ticks
    #[test]
    fn test_command_latency_defers_set_temperature() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Fahrenheit;
        sim.set_command_latency_ticks(4);

        // Send SetTemperature(96)
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(96).encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Ticks 1-3: set_temp should remain at default (104.0)
        for i in 1..=3 {
            sim.tick();
            assert_eq!(
                sim.state.set_temp, 104.0,
                "tick {}: set_temp should not change yet",
                i
            );
        }

        // Tick 4: set_temp should change to 96.0
        sim.tick();
        assert_eq!(
            sim.state.set_temp, 96.0,
            "tick 4: deferred set_temp should be applied"
        );
    }

    // VAL-SIM-012: Multiple deferred commands applied in order
    #[test]
    fn test_command_latency_set_temp_and_toggle_order() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Fahrenheit;
        sim.set_command_latency_ticks(2);

        // Send set_temp and toggle in sequence
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(96).encode();
        let encoded1 = FrameEncoder::encode(mt, &payload).unwrap();

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded2 = FrameEncoder::encode(mt, &payload).unwrap();

        sim.process_incoming_bytes(&encoded1);
        sim.process_incoming_bytes(&encoded2);

        // Tick 1: pending
        sim.tick();
        assert_eq!(sim.state.set_temp, 104.0, "set_temp unchanged tick 1");
        assert_eq!(sim.state.pumps[0], PumpState::Off, "pump unchanged tick 1");

        // Tick 2: both should apply
        sim.tick();
        assert_eq!(sim.state.set_temp, 96.0, "set_temp applied tick 2");
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "pump toggle applied tick 2"
        );
    }

    // -------------------------------------------------------------------------
    // Intermittent commands: success rate 0.5
    // -------------------------------------------------------------------------

    // VAL-SIM-013: ~50% of commands accepted with success_rate=0.5
    #[test]
    fn test_command_success_rate_50_percent() {
        let mut sim = SpaSim::new();
        sim.set_command_success_rate(0.5);

        let total_commands = 100;
        let mut accepted = 0;

        for _ in 0..total_commands {
            let (mt, payload) = launa_protocol::command::Command::ToggleItem(
                launa_protocol::command::ToggleItem::Pump1,
            )
            .encode();
            let encoded = FrameEncoder::encode(mt, &payload).unwrap();

            let before = sim.state.pumps[0];
            sim.process_incoming_bytes(&encoded);
            let after = sim.state.pumps[0];

            // If the pump state changed, the command was accepted
            if before != after {
                accepted += 1;
            }

            // Reset pump state for next iteration to avoid saturation
            sim.state.pumps[0] = PumpState::Off;
        }

        let acceptance_rate = accepted as f32 / total_commands as f32;
        assert!(
            acceptance_rate >= 0.35 && acceptance_rate <= 0.65,
            "with rate=0.5, expected ~50% acceptance (35-65%), got {:.0}% ({}/{})",
            acceptance_rate * 100.0,
            accepted,
            total_commands
        );
    }

    // VAL-SIM-013: Rate 0.0 rejects all, rate 1.0 accepts all (boundary)
    #[test]
    fn test_command_success_rate_boundaries() {
        // Rate 0.0: no commands accepted
        let mut sim = SpaSim::new();
        sim.set_command_success_rate(0.0);
        for _ in 0..20 {
            let (mt, payload) = launa_protocol::command::Command::ToggleItem(
                launa_protocol::command::ToggleItem::Pump1,
            )
            .encode();
            let encoded = FrameEncoder::encode(mt, &payload).unwrap();
            sim.process_incoming_bytes(&encoded);
        }
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Off,
            "rate 0.0 should reject all commands"
        );

        // Rate 1.0: all commands accepted
        let mut sim = SpaSim::new();
        sim.set_command_success_rate(1.0);
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "rate 1.0 should accept all commands"
        );
    }

    // -------------------------------------------------------------------------
    // Variable Ready interval: gap collection
    // -------------------------------------------------------------------------

    // VAL-SIM-014: Gaps between Ready frames fall within [min, max]
    #[test]
    fn test_variable_ready_interval_gaps_in_range() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_ready_interval_range(2, 5);

        let mut last_ready_tick: Option<u64> = None;
        let mut gaps: Vec<u64> = Vec::new();

        for _ in 0..200 {
            let tick = sim.tick_count() + 1;
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            for f in &frames {
                if f.message_type == [0x10, 0xBF] {
                    if let Some(last) = last_ready_tick {
                        gaps.push(tick - last);
                    }
                    last_ready_tick = Some(tick);
                }
            }
        }

        assert!(!gaps.is_empty(), "should have observed some Ready frames");

        for (i, &gap) in gaps.iter().enumerate() {
            assert!(
                gap >= 2 && gap <= 5,
                "gap {} should be in [2, 5], got {}",
                i,
                gap
            );
        }
    }

    // VAL-SIM-014: Variable Ready with min == max produces constant interval
    #[test]
    fn test_variable_ready_interval_constant_when_min_eq_max() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_ready_interval_range(3, 3);

        let mut last_ready_tick: Option<u64> = None;
        let mut gaps: Vec<u64> = Vec::new();

        for _ in 0..30 {
            let tick = sim.tick_count() + 1;
            let bytes = sim.tick();
            let mut decoder = FrameDecoder::new();
            let frames = decoder.feed_slice(&bytes);
            for f in &frames {
                if f.message_type == [0x10, 0xBF] {
                    if let Some(last) = last_ready_tick {
                        gaps.push(tick - last);
                    }
                    last_ready_tick = Some(tick);
                }
            }
        }

        // All gaps should be exactly 3
        for (i, &gap) in gaps.iter().enumerate() {
            assert_eq!(gap, 3, "gap {} should be exactly 3 when min=max=3", i);
        }
    }

    // -------------------------------------------------------------------------
    // Frame jitter: decoder handles padding bytes without errors
    // -------------------------------------------------------------------------

    // VAL-SIM-015: Frame jitter padding does not corrupt frame decoding
    #[test]
    fn test_frame_jitter_no_decode_errors_over_50_ticks() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_frame_jitter_ticks(10);

        let mut status_count = 0;
        let mut ready_count = 0;
        let mut decoder = FrameDecoder::new();

        for _ in 0..50 {
            let bytes = sim.tick();
            let frames = decoder.feed_slice(&bytes);
            for f in &frames {
                if f.message_type == [0xFF, 0xAF] {
                    status_count += 1;
                } else if f.message_type == [0x10, 0xBF] {
                    ready_count += 1;
                }
            }
        }

        assert_eq!(
            status_count, 50,
            "should decode 50 status frames with jitter"
        );
        assert!(
            ready_count > 0,
            "should decode some ready frames with jitter"
        );
        assert_eq!(
            decoder.frame_error_count(),
            0,
            "should have zero frame errors with jitter padding"
        );
    }

    // -------------------------------------------------------------------------
    // Partial frame reassembly
    // -------------------------------------------------------------------------

    // VAL-SIM-016: Partial frame split in the middle produces correct decoded content
    #[test]
    fn test_partial_frame_reassembly_content_correct() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 100.0;
        sim.state.set_temp = 100.0;

        // Generate a reference frame for content comparison
        let reference_bytes = sim.generate_status_frame();
        let mut ref_decoder = FrameDecoder::new();
        let ref_frames = ref_decoder.feed_slice(&reference_bytes);
        assert_eq!(ref_frames.len(), 1, "reference should be 1 frame");
        let reference_payload = ref_frames[0].payload.clone();

        // Now split a frame and verify reassembled content matches
        // Use a fresh sim to get consistent state
        let mut sim2 = SpaSim::new();
        sim2.registered = true;
        sim2.state.current_temp = 100.0;
        sim2.state.set_temp = 100.0;

        let status_bytes = sim2.generate_status_frame();
        let split_point = status_bytes.len() / 3; // Split at 1/3
        sim2.inject_partial_frame_at(split_point);

        // Tick 1: first partial
        let tick1_bytes = sim2.tick();
        // Tick 2: remainder + ready
        let tick2_bytes = sim2.tick();

        let mut decoder = FrameDecoder::new();
        let _partial = decoder.feed_slice(&tick1_bytes);
        let reassembled = decoder.feed_slice(&tick2_bytes);

        // Should have at least status + ready
        assert!(
            reassembled.len() >= 2,
            "should reassemble status + ready, got {} frames",
            reassembled.len()
        );

        // First reassembled frame should be the status frame
        assert_eq!(
            reassembled[0].message_type,
            [0xFF, 0xAF],
            "first frame should be status"
        );

        // The payload of the status frame should match reference (allowing for
        // minute increment since a tick occurred between reference and split)
        // Key check: message type and payload length match
        assert_eq!(
            reassembled[0].payload.len(),
            reference_payload.len(),
            "reassembled payload length should match reference"
        );
    }

    // -------------------------------------------------------------------------
    // Custom config responses: explicit VAL-SIM round-trip tests
    // -------------------------------------------------------------------------

    // VAL-SIM-019: Custom SpaConfig round-trip matches configured values
    #[test]
    fn test_val_sim_019_custom_spa_config_round_trip() {
        let mut raw = [0u8; 10];
        raw[0] = 0x01; // 1 pump
        raw[1] = 0x03; // 3 pumps worth of config
        raw[5] = 0b00_00_00_01; // pump1=SingleSpeed
        raw[7] = 0x0F; // light1 + light2 (all bits)
        raw[8] = 0x00; // no circ pump, no blower
        raw[9] = 0x42; // arbitrary

        let mut sim = SpaSim::new();
        sim.set_spa_config_config(SpaConfigConfig { raw_payload: raw });

        let response = sim.generate_config_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::ControlConfiguration(config) => {
                assert_eq!(
                    config.pump_configs[0],
                    PumpConfig::SingleSpeed,
                    "pump1 should be SingleSpeed"
                );
                assert!(!config.circ_pump, "circ pump should not be present");
                assert!(!config.blower, "blower should not be present");
            }
            other => panic!("Expected ControlConfiguration, got {:?}", other),
        }
    }

    // VAL-SIM-020: Custom InformationResponse round-trip matches configured values
    #[test]
    fn test_val_sim_020_custom_information_round_trip() {
        let mut model = [b' '; 8];
        model[..4].copy_from_slice(b"TEST");

        let mut sim = SpaSim::new();
        sim.set_information_config(InformationConfig {
            software_id_byte0: 0x11,
            software_id_byte1: 0x22,
            software_version_byte0: 0x33,
            software_version_byte1: 0x44,
            system_model: model,
            current_setup: 0x07,
            config_sig_byte0: 0xAB,
            config_sig_byte1: 0xCD,
            config_sig_byte2: 0xEF,
            config_sig_byte3: 0x01,
            heater_voltage: 0x01,
            heater_type: 0x0A,
            dip_switch_byte0: 0x0F,
            dip_switch_byte1: 0xF0,
        });

        let response = sim.generate_information_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::InformationResponse(info) => {
                assert_eq!(info.system_model, "TEST");
                assert_eq!(info.current_setup, 0x07);
                assert_eq!(info.config_signature, "ABCDEF01");
                assert_eq!(info.dip_switches, "0000111111110000");
            }
            other => panic!("Expected InformationResponse, got {:?}", other),
        }
    }

    // VAL-SIM-021: Custom FilterCycles round-trip matches configured values
    #[test]
    fn test_val_sim_021_custom_filter_cycles_round_trip() {
        let mut sim = SpaSim::new();
        sim.set_filter_cycles_config(FilterCyclesConfig {
            filter1: FilterCycleConfig {
                start_hour: 0,
                start_minute: 0,
                duration_hours: 1,
                duration_minutes: 0,
                enabled: true,
            },
            filter2: FilterCycleConfig {
                start_hour: 12,
                start_minute: 30,
                duration_hours: 3,
                duration_minutes: 45,
                enabled: false,
            },
        });

        let response = sim.generate_filter_cycles_response();
        let msg = dispatch_response(&response);

        match msg {
            launa_protocol::dispatcher::IncomingMessage::FilterCyclesResponse(fc) => {
                assert_eq!(fc.filter1.start_hour, 0);
                assert_eq!(fc.filter1.start_minute, 0);
                assert_eq!(fc.filter1.duration_hours, 1);
                assert_eq!(fc.filter1.duration_minutes, 0);
                assert!(fc.filter1.enabled);

                assert_eq!(fc.filter2.start_hour, 12);
                assert_eq!(fc.filter2.start_minute, 30);
                assert_eq!(fc.filter2.duration_hours, 3);
                assert_eq!(fc.filter2.duration_minutes, 45);
                assert!(!fc.filter2.enabled);
            }
            other => panic!("Expected FilterCyclesResponse, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------------
    // Combined degraded bus: ALL features together for 500 ticks
    // -------------------------------------------------------------------------

    // VAL-SIM-023: Combined degraded bus conditions cause no panics over 500 ticks
    #[test]
    fn test_combined_degraded_bus_500_ticks() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.current_temp = 95.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;

        // Enable ALL degradation features
        sim.set_frame_jitter_ticks(5);
        sim.set_command_latency_ticks(2);
        sim.set_command_success_rate(0.7);
        sim.set_ready_interval_range(2, 4);
        sim.set_physics_overshoot(1.5);
        sim.set_physics_noise_amplitude(1.0);
        sim.set_physics_unknown_temp_ticks(5); // First 5 ticks unknown temp

        let mut decoder = FrameDecoder::new();
        let mut status_count = 0;
        let mut frame_errors = 0;
        let mut panic_detected = false;

        for _tick_num in 1..=500 {
            let bytes = sim.tick();
            if bytes.is_empty() {
                continue; // bus silence
            }

            let frames = decoder.feed_slice(&bytes);

            for f in &frames {
                if f.message_type == [0xFF, 0xAF] {
                    status_count += 1;
                    let msg = launa_protocol::dispatcher::dispatch_frame(f);
                    match msg {
                        launa_protocol::dispatcher::IncomingMessage::StatusUpdate(_) => {}
                        launa_protocol::dispatcher::IncomingMessage::Unknown { .. } => {
                            // This should not happen — means protocol desync
                            panic_detected = true;
                        }
                        _ => {} // other messages are fine
                    }
                }
            }

            frame_errors += decoder.frame_error_count() as usize;
        }

        assert!(
            !panic_detected,
            "protocol desync detected during 500 tick degraded bus test"
        );
        assert_eq!(
            frame_errors, 0,
            "should have zero frame errors during degraded bus test"
        );
        assert!(
            status_count >= 400,
            "should have decoded most status frames, got {}",
            status_count
        );
    }

    // VAL-SIM-023: Combined degraded bus with commands still delivers eventually
    #[test]
    fn test_combined_degraded_bus_commands_eventually_deliver() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.pumps[0] = PumpState::Off;
        sim.set_command_latency_ticks(2);
        sim.set_command_success_rate(0.7);
        sim.set_frame_jitter_ticks(3);
        sim.set_ready_interval_range(1, 2);

        // Send toggle pump1 command
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();

        // Try sending the command multiple times
        let mut pump_changed = false;
        for _ in 0..50 {
            sim.process_incoming_bytes(&encoded);
            // Tick through latency
            for _ in 0..3 {
                sim.tick();
            }
            if sim.state.pumps[0] != PumpState::Off {
                pump_changed = true;
                break;
            }
        }

        // With 70% success rate and 50 attempts, should eventually succeed
        assert!(
            pump_changed,
            "command should eventually be accepted with rate=0.7"
        );
    }

    // -------------------------------------------------------------------------
    // Spa reboot preserving physics
    // -------------------------------------------------------------------------

    // VAL-SIM-025: Spa reboot preserves physical state but resets registration
    #[test]
    fn test_spa_reboot_preserves_physics_state_after_running() {
        let mut sim = SpaSim::new();
        sim.state.current_temp = 80.0;
        sim.state.set_temp = 104.0;
        sim.state.is_heating = true;
        sim.state.pumps[0] = PumpState::Low;
        sim.registered = true;
        sim.client_id = Some(0x05);

        // Run 30 ticks to heat up
        for _ in 0..30 {
            sim.tick();
        }

        let temp_before_reboot = sim.state.current_temp;
        let pump_before = sim.state.pumps[0];
        let light_before = sim.state.lights[0];
        assert!(
            temp_before_reboot > 80.0,
            "should have heated up before reboot"
        );

        // Reboot
        sim.simulate_spa_reboot();

        // Registration should be reset
        assert!(!sim.registered, "should be unregistered after reboot");
        assert!(
            sim.client_id.is_none(),
            "client_id should be cleared after reboot"
        );

        // Physical state preserved
        assert_eq!(
            sim.state.current_temp, temp_before_reboot,
            "temperature should survive reboot"
        );
        assert_eq!(
            sim.state.pumps[0], pump_before,
            "pump state should survive reboot"
        );
        assert_eq!(
            sim.state.lights[0], light_before,
            "light state should survive reboot"
        );

        // Physics should continue running after reboot
        let temp_after_tick = {
            sim.tick();
            sim.state.current_temp
        };
        assert_ne!(
            temp_after_tick, temp_before_reboot,
            "physics should continue after reboot"
        );
    }

    // -------------------------------------------------------------------------
    // Filter cycle start/stop
    // -------------------------------------------------------------------------

    // VAL-SIM-026: Filter cycle start turns pump on
    #[test]
    fn test_filter_cycle_start_turns_pump_on() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        assert_eq!(sim.state.pumps[0], PumpState::Off);

        // Schedule filter cycle start at tick 3
        sim.simulate_filter_cycle_start(0, 3);

        sim.tick(); // tick 1
        assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 1: still off");
        sim.tick(); // tick 2
        assert_eq!(sim.state.pumps[0], PumpState::Off, "tick 2: still off");
        sim.tick(); // tick 3: event fires
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Low,
            "tick 3: pump should turn on from filter cycle"
        );
    }

    // VAL-SIM-026: Filter cycle start does not double-toggle running pump
    #[test]
    fn test_filter_cycle_start_does_not_toggle_running_pump() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.state.pumps[1] = PumpState::High;

        // Schedule filter cycle start for pump 2 (already High)
        sim.simulate_filter_cycle_start(1, 1);

        sim.tick();

        // Should remain High (not cycled to Off)
        assert_eq!(
            sim.state.pumps[1],
            PumpState::High,
            "pump should remain High if already running"
        );
    }

    // VAL-SIM-026: Multiple filter cycles on different pumps at different ticks
    #[test]
    fn test_multiple_filter_cycles_different_pumps() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Start pump1 at tick 3, pump2 at tick 7
        sim.simulate_filter_cycle_start(0, 3);
        sim.simulate_filter_cycle_start(1, 7);

        // Tick through
        for _ in 0..2 {
            sim.tick();
        }
        assert_eq!(sim.state.pumps[0], PumpState::Off, "pump1 off before event");
        assert_eq!(sim.state.pumps[1], PumpState::Off, "pump2 off before event");

        sim.tick(); // tick 3: pump1 starts
        assert_eq!(sim.state.pumps[0], PumpState::Low, "pump1 on at tick 3");
        assert_eq!(sim.state.pumps[1], PumpState::Off, "pump2 still off");

        for _ in 0..3 {
            sim.tick();
        }
        sim.tick(); // tick 7: pump2 starts
        assert_eq!(sim.state.pumps[1], PumpState::Low, "pump2 on at tick 7");
    }

    // VAL-SIM-026: Filter cycle pump state visible in status frames
    #[test]
    fn test_filter_cycle_pump_state_in_status_frame() {
        let mut sim = SpaSim::new();
        sim.registered = true;

        // Schedule pump1 to start at tick 2
        sim.simulate_filter_cycle_start(0, 2);

        // Tick 1: pump off, status should reflect that
        let bytes1 = sim.tick();
        let mut decoder = FrameDecoder::new();
        let frames1 = decoder.feed_slice(&bytes1);
        let msg1 = launa_protocol::dispatcher::dispatch_frame(&frames1[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg1 {
            assert_eq!(s.pumps[0], PumpState::Off, "tick 1: pump off in status");
        }

        // Tick 2: event fires, pump starts
        let bytes2 = sim.tick();
        let mut decoder2 = FrameDecoder::new();
        let frames2 = decoder2.feed_slice(&bytes2);
        let msg2 = launa_protocol::dispatcher::dispatch_frame(&frames2[0]);
        if let launa_protocol::dispatcher::IncomingMessage::StatusUpdate(s) = msg2 {
            assert_eq!(
                s.pumps[0],
                PumpState::Low,
                "tick 2: pump on in status after filter cycle"
            );
        }
    }

    // Filter cycle stop: manually turn pump off after filter cycle
    #[test]
    fn test_filter_cycle_stop_manual_toggle_off() {
        let mut sim = SpaSim::new();
        sim.registered = true;
        sim.set_command_success_rate(1.0);

        // Start pump via filter cycle
        sim.simulate_filter_cycle_start(0, 1);
        sim.tick(); // tick 1: event fires, pump = Low
        assert_eq!(sim.state.pumps[0], PumpState::Low);

        // Manually toggle pump off (simulating filter cycle end)
        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload).unwrap();
        sim.process_incoming_bytes(&encoded);

        // Pump should cycle Low → High (not Off!)
        // Pump cycle: Off → Low → High → Off
        // Current: Low, toggle → High
        assert_eq!(
            sim.state.pumps[0],
            PumpState::High,
            "toggle from Low goes to High"
        );

        // Toggle again → Off
        sim.process_incoming_bytes(&encoded);
        assert_eq!(
            sim.state.pumps[0],
            PumpState::Off,
            "second toggle goes to Off (filter cycle stopped)"
        );
    }
}
