//! Frame generation helpers for spa simulator responses.
//!
//! Contains the encoding functions that convert Rust types to raw wire-format
//! bytes for status updates, configuration, fault logs, filter cycles,
//! and information responses.

use launa_protocol::frame::FrameEncoder;
use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};

use super::config::{FaultLogConfig, FilterCyclesConfig, InformationConfig, SpaConfigConfig};
use super::state::SpaState;

/// Encode pump state to 2-bit wire representation.
pub(crate) fn pump_state_to_bits(state: PumpState) -> u8 {
    match state {
        PumpState::Off => 0,
        PumpState::Low => 1,
        PumpState::High => 2,
    }
}

/// Cycle pump state: Off → Low → High → Off.
pub(crate) fn cycle_pump(state: PumpState) -> PumpState {
    match state {
        PumpState::Off => PumpState::Low,
        PumpState::Low => PumpState::High,
        PumpState::High => PumpState::Off,
    }
}

/// Cycle heating mode: Ready → Rest → ReadyInRest → Ready.
pub(crate) fn cycle_heating_mode(mode: HeatingMode) -> HeatingMode {
    match mode {
        HeatingMode::Ready => HeatingMode::Rest,
        HeatingMode::Rest => HeatingMode::ReadyInRest,
        HeatingMode::ReadyInRest => HeatingMode::Ready,
    }
}

/// Flip temperature range: High ↔ Low.
pub(crate) fn flip_temp_range(range: TempRange) -> TempRange {
    match range {
        TempRange::High => TempRange::Low,
        TempRange::Low => TempRange::High,
    }
}

/// Apply a toggle command by raw protocol item code to the given state.
///
/// This is the boundary where raw bytes → Rust type mutations.
pub(crate) fn apply_toggle_by_code(state: &mut SpaState, item_code: u8) {
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

/// Generate a complete framed status update.
///
/// This is the boundary where Rust types → raw wire bytes.
/// If corrupt frame injection is enabled, the last payload byte is flipped.
pub(crate) fn generate_status_frame(
    state: &SpaState,
    priming_remaining_ticks: u64,
    fault_active: bool,
    report_unknown_temp: bool,
    sensor_noise_jitter: f32,
    physics_unknown_temp_ticks: u64,
    physics_tick_count: u64,
    physics_noise_amplitude: f32,
    physics_noise_value: f32,
    ready_rand_value: f64,
    inject_corrupt_next: bool,
) -> Vec<u8> {
    let mut payload = [0u8; 24];

    // Offset 0: Spa State (0x00=Running, 0x05=Hold)
    if state.hold {
        payload[0] = 0x05;
    }
    // Offset 1: Init Mode (0x00=Idle, 0x01=Priming, 0x02=Fault)
    if priming_remaining_ticks > 0 {
        payload[1] = 0x01;
    }
    if fault_active {
        payload[1] = 0x02;
    }
    // Offset 2: Current Temperature
    let in_unknown_period =
        physics_unknown_temp_ticks > 0 && physics_tick_count <= physics_unknown_temp_ticks;
    if report_unknown_temp || in_unknown_period {
        payload[2] = 0xFF; // Unknown temperature
    } else {
        let mut reported_temp = state.current_temp;
        // Apply physics-model noise (if configured)
        if physics_noise_amplitude > 0.0 {
            reported_temp += physics_noise_value;
        }
        // Apply legacy sensor_noise_jitter (if configured)
        if sensor_noise_jitter > 0.0 {
            let normalized = (ready_rand_value / (i64::MAX as f64)) as f32;
            let jitter = normalized * sensor_noise_jitter;
            reported_temp += jitter;
        }
        payload[2] = SpaState::encode_temp(reported_temp, state.temp_scale);
    }
    // Offset 3: Hour, Offset 4: Minute
    payload[3] = state.hour;
    payload[4] = state.minute;

    // Offset 5: Heating Mode (0=Ready, 1=Rest, 3=Ready-in-Rest)
    payload[5] |= match state.heating_mode {
        HeatingMode::Ready => 0,
        HeatingMode::Rest => 1,
        HeatingMode::ReadyInRest => 3,
    };

    // Offset 9: Flags (temp scale bit 0, 24h time bit 1, filter mode bits 2-3)
    if matches!(state.temp_scale, TemperatureScale::Celsius) {
        payload[9] |= 0x01;
    }
    payload[9] |= 0x02; // 24h format

    // Offset 10: Flags (temp range bit 2, heating state bits 4-5)
    if state.is_heating {
        payload[10] |= 0x30;
    }
    if matches!(state.temp_range, TempRange::High) {
        payload[10] |= 0x04;
    }

    // Offset 11: Pumps 1-4 (2 bits each)
    payload[11] = pump_state_to_bits(state.pumps[0])
        | (pump_state_to_bits(state.pumps[1]) << 2)
        | (pump_state_to_bits(state.pumps[2]) << 4)
        | (pump_state_to_bits(state.pumps[3]) << 6);

    // Offset 12: Pump5 bits 0-1, Pump6 bits 2-3
    payload[12] = pump_state_to_bits(state.pumps[4]) | (pump_state_to_bits(state.pumps[5]) << 2);

    // Offset 13: Circ pump (bit 1), Blower (bits 2-3)
    if state.circ_pump {
        payload[13] |= 0x02;
    }
    if state.blower {
        payload[13] |= 0x0C;
    }
    // Offset 14: Lights (bits 0-1 = Light1, bits 2-3 = Light2)
    if state.lights[0] {
        payload[14] |= 0x03;
    }
    if state.lights[1] {
        payload[14] |= 0x0C;
    }
    // Offset 15: Mister (0=off, 1=on)
    if state.mister {
        payload[15] = 0x01;
    }

    // Offset 20: Set Temperature
    payload[20] = SpaState::encode_temp(state.set_temp, state.temp_scale);

    let mut frame = FrameEncoder::encode([0xFF, 0xAF], &payload).unwrap();

    // Corrupt frame injection: flip a byte in the middle of the encoded frame
    // to guarantee a CRC mismatch on decode. Corrupting the end marker doesn't
    // work because the parser finds a valid frame in the buffer before it.
    if inject_corrupt_next {
        // Corrupt a byte in the middle of the frame body (index 5 is well past the
        // start marker and length byte, safely inside the payload area).
        // Skip the start marker (index 0) and target a payload byte.
        if frame.len() > 6 {
            frame[5] ^= 0xFF;
        }
    }

    frame
}

/// Generate a `Ready` frame (`10 BF 06`).
pub(crate) fn generate_ready_frame() -> Vec<u8> {
    FrameEncoder::encode([0x10, 0xBF], &[0x06]).unwrap()
}

/// Generate a registration query (`FE BF 00`).
pub(crate) fn generate_registration_query() -> Vec<u8> {
    FrameEncoder::encode([0xFE, 0xBF], &[0x00]).unwrap()
}

/// Generate a client ID assignment (`FE BF 02 <ID>`).
pub(crate) fn generate_client_id_assignment(id: u8) -> Vec<u8> {
    FrameEncoder::encode([0xFE, 0xBF], &[0x02, id]).unwrap()
}

/// Generate a configuration response.
pub(crate) fn generate_config_response(
    state: &SpaState,
    spa_config_config: &SpaConfigConfig,
) -> Vec<u8> {
    let mut config_payload = spa_config_config.raw_payload;

    // Adapt temperature scale bit from current state
    if matches!(state.temp_scale, TemperatureScale::Celsius) {
        config_payload[3] |= 0x01;
    } else {
        config_payload[3] &= !0x01;
    }

    let mut full_payload = vec![0x2E];
    full_payload.extend_from_slice(&config_payload);
    FrameEncoder::encode([0x0A, 0xBF], &full_payload).unwrap()
}

/// Generate an information response.
pub(crate) fn generate_information_response(information_config: &InformationConfig) -> Vec<u8> {
    let cfg = information_config;
    let mut info_data = [0u8; 21];
    info_data[0] = cfg.software_id_byte0;
    info_data[1] = cfg.software_id_byte1;
    info_data[2] = cfg.software_version_byte0;
    info_data[3] = cfg.software_version_byte1;
    info_data[4..12].copy_from_slice(&cfg.system_model);
    info_data[12] = cfg.current_setup;
    info_data[13] = cfg.config_sig_byte0;
    info_data[14] = cfg.config_sig_byte1;
    info_data[15] = cfg.config_sig_byte2;
    info_data[16] = cfg.config_sig_byte3;
    info_data[17] = cfg.heater_voltage;
    info_data[18] = cfg.heater_type;
    info_data[19] = cfg.dip_switch_byte0;
    info_data[20] = cfg.dip_switch_byte1;

    let mut full_payload = vec![0x24];
    full_payload.extend_from_slice(&info_data);
    FrameEncoder::encode([0x0A, 0xBF], &full_payload).unwrap()
}

/// Generate a fault log response (entry 1 by default).
pub(crate) fn generate_fault_log_response(fault_log_config: &FaultLogConfig) -> Vec<u8> {
    generate_fault_log_response_for_entry(fault_log_config, &[], 1)
}

/// Generate a fault log response for a specific entry number.
///
/// Entry numbers are 1-based. Entry 0 or entries past the end of the
/// fault_log_entries list return a sentinel response with fault_count=0.
/// When no multi-entry fault log is configured, falls back to the single
/// fault_log_config for entry 1.
pub(crate) fn generate_fault_log_response_for_entry(
    fault_log_config: &FaultLogConfig,
    fault_log_entries: &[FaultLogConfig],
    entry_number: u8,
) -> Vec<u8> {
    if !fault_log_entries.is_empty() {
        // Multi-entry mode
        if entry_number == 0 || entry_number as usize > fault_log_entries.len() {
            // Sentinel: fault_count = 0
            let sentinel_data: [u8; 10] = [0; 10];
            let mut full_payload = vec![0x28];
            full_payload.extend_from_slice(&sentinel_data);
            return FrameEncoder::encode([0x0A, 0xBF], &full_payload).unwrap();
        }
        let cfg = &fault_log_entries[entry_number as usize - 1];
        let fault_data: [u8; 10] = [
            cfg.fault_count,
            cfg.entry_number,
            cfg.message_code.code(),
            cfg.days_ago,
            cfg.hour,
            cfg.minute,
            cfg.flags,
            cfg.set_temperature,
            cfg.sensor_a_temp,
            cfg.sensor_b_temp,
        ];
        let mut full_payload = vec![0x28];
        full_payload.extend_from_slice(&fault_data);
        return FrameEncoder::encode([0x0A, 0xBF], &full_payload).unwrap();
    }
    // Legacy single-entry mode
    let cfg = fault_log_config;
    let fault_data: [u8; 10] = [
        cfg.fault_count,
        cfg.entry_number,
        cfg.message_code.code(),
        cfg.days_ago,
        cfg.hour,
        cfg.minute,
        cfg.flags,
        cfg.set_temperature,
        cfg.sensor_a_temp,
        cfg.sensor_b_temp,
    ];
    let mut full_payload = vec![0x28];
    full_payload.extend_from_slice(&fault_data);
    FrameEncoder::encode([0x0A, 0xBF], &full_payload).unwrap()
}

/// Generate a filter cycles response.
pub(crate) fn generate_filter_cycles_response(
    filter_cycles_config: &FilterCyclesConfig,
) -> Vec<u8> {
    let cfg = filter_cycles_config;
    let f1 = &cfg.filter1;
    let f2 = &cfg.filter2;
    let f2_start_hour = if f2.enabled {
        f2.start_hour | 0x80
    } else {
        f2.start_hour & 0x7F
    };
    let filter_data: [u8; 8] = [
        f1.start_hour,
        f1.start_minute,
        f1.duration_hours,
        f1.duration_minutes,
        f2_start_hour,
        f2.start_minute,
        f2.duration_hours,
        f2.duration_minutes,
    ];
    let mut full_payload = vec![0x23];
    full_payload.extend_from_slice(&filter_data);
    FrameEncoder::encode([0x0A, 0xBF], &full_payload).unwrap()
}
