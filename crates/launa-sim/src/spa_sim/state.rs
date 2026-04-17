//! Spa state types and event definitions.
//!
//! `SpaState` holds the simulated spa's current state in native Rust types
//! (f32 temperatures, proper enums). Conversion to wire format happens only
//! at the frame generation boundary.

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
    pub(crate) fn encode_temp(temp: f32, scale: TemperatureScale) -> u8 {
        let raw = match scale {
            TemperatureScale::Fahrenheit => temp,
            TemperatureScale::Celsius => temp * 2.0,
        };
        raw.round() as u8
    }

    /// Decode a raw wire temperature to real units.
    pub(crate) fn decode_temp(raw: u8, scale: TemperatureScale) -> f32 {
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
