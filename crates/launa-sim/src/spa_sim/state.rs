//! Spa state types and event definitions.
//!
//! `SpaState` holds the simulated spa's current state using the `Temperature`
//! type for all temperature fields, eliminating scale ambiguity.

use launa_protocol::status::{HeatingMode, PumpState, TempRange, TemperatureScale};
use launa_protocol::Temperature;

/// Simulated spa state using native Rust types.
///
/// Temperature fields use `Temperature` which carries the scale, ensuring
/// comparisons between temperatures are always scale-aware.
#[derive(Debug, Clone)]
pub struct SpaState {
    /// Current water temperature.
    pub current_temp: Temperature,
    /// Target temperature (active range's value).
    pub set_temp: Temperature,
    /// Saved set temperature for the High range (independent of Low).
    pub set_temp_high: Temperature,
    /// Saved set temperature for the Low range (independent of High).
    pub set_temp_low: Temperature,
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
    /// Light states (indexed 0-3, where index 0 = Light 1).
    pub lights: [bool; 4],
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
            current_temp: Temperature::fahrenheit(100.0),
            set_temp: Temperature::fahrenheit(104.0),
            set_temp_high: Temperature::fahrenheit(104.0),
            set_temp_low: Temperature::fahrenheit(80.0),
            heating_mode: HeatingMode::Ready,
            temp_scale: TemperatureScale::Fahrenheit,
            is_heating: false,
            temp_range: TempRange::High,
            pumps: [PumpState::Off; 6],
            circ_pump: false,
            blower: false,
            lights: [false; 4],
            mister: false,
            hour: 14,
            minute: 30,
            priming: false,
            hold: false,
        }
    }
}

impl SpaState {
    /// Set the target temperature and persist to the active range's saved value.
    pub(crate) fn set_target_temp(&mut self, temp: Temperature) {
        self.set_temp = temp;
        match self.temp_range {
            TempRange::High => self.set_temp_high = temp,
            TempRange::Low => self.set_temp_low = temp,
            _ => {}
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
