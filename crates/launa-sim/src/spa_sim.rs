//! Simulated Balboa BP6013G1 spa mainboard.
//!
//! Generates realistic RS-485 byte streams identical to what a real spa controller
//! would send. Processes incoming commands and updates internal state accordingly.
//! Designed to be connected to a `SpaController` via a `SimTransport`.
//!
//! The sim uses Rust types natively (enums, f32 temps) and only converts to raw
//! bytes at the frame generation boundary.

use launa_protocol::frame::{Frame, FrameDecoder, FrameEncoder};
use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};

/// Simulated spa state using native Rust types.
///
/// All values are in real units (f32 temperatures, proper enums). Conversion
/// to the wire format happens only in `generate_status_frame()`.
#[derive(Debug, Clone)]
pub struct SpaState {
    /// Current water temperature in real units (°F or °C).
    pub current_temp: f32,
    /// Target temperature in real units.
    pub set_temp: f32,
    /// Active heating mode.
    pub heating_mode: HeatingMode,
    /// Temperature scale (affects wire encoding).
    pub temp_scale: TemperatureScale,
    /// Whether the heater element is currently active.
    pub is_heating: bool,
    /// Temperature range (high/low).
    pub temp_range: TempRange,
    /// Pump states (indexed 0-5, where index 0 = Pump 1).
    pub pumps: [PumpState; 6],
    /// Circulation pump on/off.
    pub circ_pump: bool,
    /// Blower on/off.
    pub blower: bool,
    /// Light states (indexed 0-1, where index 0 = Light 1).
    pub lights: [bool; 2],
    /// Mister on/off.
    pub mister: bool,
    /// Clock hour (0-23).
    pub hour: u8,
    /// Clock minute (0-59).
    pub minute: u8,
    /// Whether the spa is in priming mode.
    pub priming: bool,
    /// Whether the spa is in hold mode.
    pub hold: bool,
}

impl Default for SpaState {
    fn default() -> Self {
        SpaState {
            current_temp: 100.0,
            set_temp: 104.0,
            heating_mode: HeatingMode::Ready,
            temp_scale: TemperatureScale::Fahrenheit,
            is_heating: true,
            temp_range: TempRange::High,
            pumps: [PumpState::Off; 6],
            circ_pump: false,
            blower: false,
            lights: [false; 2],
            mister: false,
            hour: 14,
            minute: 30,
            priming: false,
            hold: false,
        }
    }
}

impl SpaState {
    /// Encode a temperature to the raw wire value.
    /// Fahrenheit: direct. Celsius: multiply by 2.
    fn encode_temp(temp: f32, scale: TemperatureScale) -> u8 {
        let raw = match scale {
            TemperatureScale::Fahrenheit => temp,
            TemperatureScale::Celsius => temp * 2.0,
        };
        raw.round() as u8
    }

    /// Decode a raw wire temperature to real units.
    fn decode_temp(raw: u8, scale: TemperatureScale) -> f32 {
        match scale {
            TemperatureScale::Fahrenheit => raw as f32,
            TemperatureScale::Celsius => raw as f32 / 2.0,
        }
    }
}

/// Type of spontaneous event that can be scheduled on the spa simulator.
#[derive(Debug, Clone)]
pub enum SpaEventType {
    /// Start a filter cycle, turning the specified pump on (to Low).
    FilterCycleStart { pump_index: usize },
}

/// A scheduled spontaneous event that fires at a specific tick.
#[derive(Debug, Clone)]
pub struct SpaEvent {
    pub tick: u64,
    pub event_type: SpaEventType,
}

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
        self.ready_rng_state = self
            .ready_rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.ready_rng_state
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
    pub fn tick(&mut self) -> Vec<u8> {
        self.tick_count += 1;

        // Process deferred commands (decrement timers, apply expired)
        self.process_pending_commands();

        // Bus silence: suppress all output
        if self.bus_silence_remaining > 0 {
            self.bus_silence_remaining -= 1;
            return Vec::new();
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

        // Send status update
        let status_bytes = self.generate_status_frame();

        // Duplicate frame injection: send status frame twice
        if self.duplicate_next {
            self.duplicate_next = false;
            output.extend_from_slice(&status_bytes);
            output.extend_from_slice(&status_bytes);
        } else {
            output.extend_from_slice(&status_bytes);
        }

        // Send ready indicator at randomized intervals
        if self.ready_countdown > 0 {
            self.ready_countdown -= 1;
        }
        if self.ready_countdown == 0 {
            output.extend_from_slice(&self.generate_ready_frame());
            self.ready_countdown = self.next_ready_interval();
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
    fn simulate_physics(&mut self) {
        // Temperature approach: ±1° per tick in real units
        if self.state.current_temp < self.state.set_temp && self.state.is_heating {
            self.state.current_temp = (self.state.current_temp + 1.0).min(self.state.set_temp);
        } else if self.state.current_temp > self.state.set_temp {
            self.state.current_temp = (self.state.current_temp - 1.0).max(self.state.set_temp);
        } else if self.state.current_temp == self.state.set_temp {
            if self.state.is_heating {
                self.state.is_heating = false;
            }
        }

        // Advance clock
        self.state.minute += 1;
        if self.state.minute >= 60 {
            self.state.minute = 0;
            self.state.hour = (self.state.hour + 1) % 24;
        }
    }

    /// Generate a complete framed status update.
    ///
    /// This is the boundary where Rust types → raw wire bytes.
    /// If corrupt frame injection is enabled, the last payload byte is flipped.
    pub fn generate_status_frame(&mut self) -> Vec<u8> {
        let mut payload = [0u8; 24];

        // Offset 0: Spa State (0x00=Running, 0x05=Hold)
        if self.state.hold {
            payload[0] = 0x05;
        }
        // Offset 1: Init Mode (0x00=Idle, 0x01=Priming)
        if self.state.priming {
            payload[1] = 0x01;
        }
        // Offset 2: Current Temperature
        payload[2] = SpaState::encode_temp(self.state.current_temp, self.state.temp_scale);
        // Offset 3: Hour, Offset 4: Minute
        payload[3] = self.state.hour;
        payload[4] = self.state.minute;

        // Offset 5: Heating Mode (0=Ready, 1=Rest, 3=Ready-in-Rest)
        payload[5] |= match self.state.heating_mode {
            HeatingMode::Ready => 0,
            HeatingMode::Rest => 1,
            HeatingMode::ReadyInRest => 3,
        };

        // Offset 9: Flags (temp scale bit 0, 24h time bit 1, filter mode bits 2-3)
        if matches!(self.state.temp_scale, TemperatureScale::Celsius) {
            payload[9] |= 0x01;
        }
        payload[9] |= 0x02; // 24h format

        // Offset 10: Flags (temp range bit 2, heating state bits 4-5)
        if self.state.is_heating {
            payload[10] |= 0x30;
        }
        if matches!(self.state.temp_range, TempRange::High) {
            payload[10] |= 0x04;
        }

        // Offset 11: Pumps 1-4 (2 bits each)
        payload[11] = pump_state_to_bits(self.state.pumps[0])
            | (pump_state_to_bits(self.state.pumps[1]) << 2)
            | (pump_state_to_bits(self.state.pumps[2]) << 4)
            | (pump_state_to_bits(self.state.pumps[3]) << 6);

        // Offset 12: Pump5 bits 0-1, Pump6 bits 2-3
        payload[12] = pump_state_to_bits(self.state.pumps[4])
            | (pump_state_to_bits(self.state.pumps[5]) << 2);

        // Offset 13: Circ pump (bit 1), Blower (bits 2-3)
        if self.state.circ_pump {
            payload[13] |= 0x02;
        }
        if self.state.blower {
            payload[13] |= 0x0C;
        }
        // Offset 14: Lights (bits 0-1 = Light1, bits 2-3 = Light2)
        if self.state.lights[0] {
            payload[14] |= 0x03;
        }
        if self.state.lights[1] {
            payload[14] |= 0x0C;
        }
        // Offset 15: Mister (0=off, 1=on)
        if self.state.mister {
            payload[15] = 0x01;
        }

        // Offset 20: Set Temperature
        payload[20] = SpaState::encode_temp(self.state.set_temp, self.state.temp_scale);

        let mut frame = FrameEncoder::encode([0xFF, 0xAF], &payload);

        // Corrupt frame injection: flip last byte of the encoded frame
        if self.inject_corrupt_next {
            self.inject_corrupt_next = false;
            if !frame.is_empty() {
                let last = frame.len() - 1;
                frame[last] ^= 0xFF;
            }
        }

        frame
    }

    /// Generate a `Ready` frame (`10 BF 06`).
    pub fn generate_ready_frame(&self) -> Vec<u8> {
        FrameEncoder::encode([0x10, 0xBF], &[0x06])
    }

    /// Generate a registration query (`FE BF 00`).
    pub fn generate_registration_query(&self) -> Vec<u8> {
        FrameEncoder::encode([0xFE, 0xBF], &[0x00])
    }

    /// Generate a client ID assignment (`FE BF 02 <ID>`).
    fn generate_client_id_assignment(&self, id: u8) -> Vec<u8> {
        FrameEncoder::encode([0xFE, 0xBF], &[0x02, id])
    }

    /// Generate a configuration response.
    pub fn generate_config_response(&self) -> Vec<u8> {
        let mut config_payload = [0u8; 10];
        config_payload[0] = 0x02;
        config_payload[1] = 0x02;
        if matches!(self.state.temp_scale, TemperatureScale::Celsius) {
            config_payload[3] |= 0x01;
        }
        config_payload[5] = 0x02 | (0x02 << 2);
        config_payload[7] |= 0x01;
        config_payload[8] |= 0x80;
        config_payload[8] |= 0x01;

        let mut full_payload = vec![0x2E];
        full_payload.extend_from_slice(&config_payload);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate an information response.
    pub fn generate_information_response(&self) -> Vec<u8> {
        let mut info_data = [0u8; 21];
        info_data[0] = 0x64;
        info_data[1] = 0xDC;
        info_data[2] = 0x11;
        info_data[3] = 0x00;
        let model: &[u8] = b"BFBP20  ";
        info_data[4..12].copy_from_slice(model);
        info_data[12] = 0x01;
        info_data[13] = 0x3D;
        info_data[14] = 0x12;
        info_data[15] = 0x38;
        info_data[16] = 0x2E;
        info_data[17] = 0x01;
        info_data[18] = 0x0A;
        info_data[19] = 0x04;
        info_data[20] = 0x00;

        let mut full_payload = vec![0x24];
        full_payload.extend_from_slice(&info_data);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a fault log response.
    pub fn generate_fault_log_response(&self) -> Vec<u8> {
        let fault_data: [u8; 10] = [0x03, 0x01, 0x1B, 0x02, 0x0E, 0x1E, 0x04, 0x68, 0x68, 0x66];
        let mut full_payload = vec![0x28];
        full_payload.extend_from_slice(&fault_data);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
    }

    /// Generate a filter cycles response.
    pub fn generate_filter_cycles_response(&self) -> Vec<u8> {
        let filter_data: [u8; 8] = [0x08, 0x00, 0x04, 0x00, 0x90, 0x00, 0x02, 0x00];
        let mut full_payload = vec![0x23];
        full_payload.extend_from_slice(&filter_data);
        FrameEncoder::encode([0x0A, 0xBF], &full_payload)
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

/// Apply a toggle command by raw protocol item code to the given state.
///
/// This is the boundary where raw bytes → Rust type mutations.
fn apply_toggle_by_code(state: &mut SpaState, item_code: u8) {
    match item_code {
        0x04..=0x09 => {
            let idx = (item_code - 0x04) as usize;
            if idx < 6 {
                state.pumps[idx] = cycle_pump(state.pumps[idx]);
            }
        }
        0x0C => state.blower = !state.blower,
        0x11 => state.lights[0] = !state.lights[0],
        0x12 => state.lights[1] = !state.lights[1],
        0x3C => state.hold = !state.hold,
        0x51 => state.heating_mode = cycle_heating_mode(state.heating_mode),
        0x50 => state.temp_range = flip_temp_range(state.temp_range),
        _ => {}
    }
}

// -- Helper functions for type-safe state transitions --

fn pump_state_to_bits(state: PumpState) -> u8 {
    match state {
        PumpState::Off => 0,
        PumpState::Low => 1,
        PumpState::High => 2,
    }
}

fn cycle_pump(state: PumpState) -> PumpState {
    match state {
        PumpState::Off => PumpState::Low,
        PumpState::Low => PumpState::High,
        PumpState::High => PumpState::Off,
    }
}

fn cycle_heating_mode(mode: HeatingMode) -> HeatingMode {
    match mode {
        HeatingMode::Ready => HeatingMode::Rest,
        HeatingMode::Rest => HeatingMode::ReadyInRest,
        HeatingMode::ReadyInRest => HeatingMode::Ready,
    }
}

fn flip_temp_range(range: TempRange) -> TempRange {
    match range {
        TempRange::High => TempRange::Low,
        TempRange::Low => TempRange::High,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        for _ in 0..5 {
            sim.simulate_physics();
        }
        assert_eq!(sim.state.current_temp, 100.0);
        assert!(
            sim.state.is_heating,
            "still heating on the tick that reaches target"
        );

        sim.simulate_physics();
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
        assert_eq!(sim.state.current_temp, 104.0);
    }

    #[test]
    fn test_process_toggle_via_bytes() {
        let mut sim = SpaSim::new();

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump1,
        )
        .encode();
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);
        sim.process_incoming_bytes(&encoded);

        assert_eq!(sim.state.set_temp, 100.0);
    }

    #[test]
    fn test_set_temp_decoded_celsius() {
        let mut sim = SpaSim::new();
        sim.state.temp_scale = TemperatureScale::Celsius;

        // SetTemperature sends raw value 80 (= 40°C on wire)
        let (mt, payload) = launa_protocol::command::Command::SetTemperature(80).encode();
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);

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
        let encoded1 = FrameEncoder::encode(mt, &payload);

        let (mt, payload) = launa_protocol::command::Command::ToggleItem(
            launa_protocol::command::ToggleItem::Pump2,
        )
        .encode();
        let encoded2 = FrameEncoder::encode(mt, &payload);

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
        let encoded = FrameEncoder::encode(mt, &payload);
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
        let encoded = FrameEncoder::encode(mt, &payload);
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
}
