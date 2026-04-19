//! On-device self-test simulator.
//!
//! When self-test mode is enabled via MQTT (`launa_spa/command/self_test`),
//! this module provides a mutable mock spa state that responds to commands
//! from Home Assistant. Commands modify the in-memory state instead of being
//! sent over RS-485, allowing end-to-end testing of the HA integration
//! without a physical spa connected.

use launa_protocol::command::{Command, ToggleItem};
use launa_protocol::status::{
    HeatingMode, PumpState, StatusUpdate, TempRange, TemperatureScale, TimeFormat,
};

/// Mock spa state for self-test mode.
///
/// Holds a `StatusUpdate` that represents the simulated spa. Commands from
/// HA are applied to this state, which is then published to MQTT each tick.
pub(crate) struct SelfTestState {
    status: StatusUpdate,
}

impl SelfTestState {
    /// Create a new self-test state with default simulated values.
    ///
    /// Simulates a spa at 37.5°C, set point 38°C, in Ready/High range mode,
    /// with heating active. All pumps/lights/accessories are off.
    pub fn new() -> Self {
        SelfTestState {
            status: StatusUpdate {
                current_temp: Some(37.5),
                set_temp: 38.0,
                hour: 12,
                minute: 0,
                heating_mode: HeatingMode::Ready,
                temperature_scale: TemperatureScale::Celsius,
                time_format: TimeFormat::Hour24,
                filter_mode: 0,
                is_heating: true,
                temp_range: TempRange::High,
                pumps: [PumpState::Off; 6],
                circ_pump: false,
                blower: false,
                mister: false,
                lights: [false; 2],
                is_priming: false,
                is_hold: false,
                notification_type: 0,
                panel_locked: false,
                settings_lock: false,
                m8_cycle_time: 0,
                sensor_a_temp: Some(37.5),
                sensor_b_temp: None,
                hold_timer_minutes: None,
            },
        }
    }

    /// Apply a command to the simulated state.
    ///
    /// Returns `true` if the command was handled (state changed),
    /// `false` if the command is not simulated.
    pub fn apply_command(&mut self, cmd: &Command) -> bool {
        match cmd {
            Command::ToggleItem(item) => self.apply_toggle(*item),
            Command::SetTemperature(temp) => {
                let divisor = match self.status.temperature_scale {
                    TemperatureScale::Celsius => 2.0,
                    TemperatureScale::Fahrenheit => 1.0,
                    _ => 1.0,
                };
                self.status.set_temp = *temp as f32 / divisor;
                true
            }
            Command::SetTemperatureScale(celsius) => {
                let new_scale = if *celsius {
                    TemperatureScale::Celsius
                } else {
                    TemperatureScale::Fahrenheit
                };
                if self.status.temperature_scale != new_scale {
                    self.status.temperature_scale = new_scale;
                }
                true
            }
            Command::SetTime {
                hour,
                minute,
                is_24h,
            } => {
                self.status.hour = *hour;
                self.status.minute = *minute;
                self.status.time_format = if *is_24h {
                    TimeFormat::Hour24
                } else {
                    TimeFormat::Hour12
                };
                true
            }
            // These commands don't affect visible state in self-test
            Command::ConfigurationRequest
            | Command::SettingsRequest(_)
            | Command::FilterCyclesRequest
            | Command::InformationRequest
            | Command::FaultLogRequest { .. }
            | Command::NothingToSend { .. }
            | Command::SelfTest(_) => false,
            _ => false,
        }
    }

    fn apply_toggle(&mut self, item: ToggleItem) -> bool {
        match item {
            ToggleItem::Pump1
            | ToggleItem::Pump2
            | ToggleItem::Pump3
            | ToggleItem::Pump4
            | ToggleItem::Pump5
            | ToggleItem::Pump6 => {
                if let Some(idx) = item.pump_index() {
                    self.status.pumps[idx] = cycle_pump_state(self.status.pumps[idx]);
                    return true;
                }
                false
            }
            ToggleItem::Light1 | ToggleItem::Light2 => {
                if let Some(idx) = item.light_index() {
                    if idx < self.status.lights.len() {
                        self.status.lights[idx] = !self.status.lights[idx];
                        return true;
                    }
                }
                false
            }
            ToggleItem::Blower => {
                self.status.blower = !self.status.blower;
                true
            }
            ToggleItem::Mister => {
                self.status.mister = !self.status.mister;
                true
            }
            ToggleItem::CirculationPump => {
                self.status.circ_pump = !self.status.circ_pump;
                true
            }
            ToggleItem::HoldMode => {
                self.status.is_hold = !self.status.is_hold;
                if self.status.is_hold {
                    self.status.hold_timer_minutes = Some(15);
                    self.status.sensor_a_temp = None;
                } else {
                    self.status.hold_timer_minutes = None;
                    self.status.sensor_a_temp = Some(self.status.current_temp.unwrap_or(37.5));
                }
                true
            }
            ToggleItem::HeatingMode => {
                self.status.heating_mode = match self.status.heating_mode {
                    HeatingMode::Ready => HeatingMode::Rest,
                    HeatingMode::Rest => HeatingMode::ReadyInRest,
                    HeatingMode::ReadyInRest => HeatingMode::Ready,
                    _ => HeatingMode::Ready,
                };
                self.status.is_heating = matches!(self.status.heating_mode, HeatingMode::Ready);
                true
            }
            ToggleItem::TemperatureRange => {
                self.status.temp_range = match self.status.temp_range {
                    TempRange::High => TempRange::Low,
                    TempRange::Low => TempRange::High,
                    _ => TempRange::High,
                };
                true
            }
            // These toggles don't have a meaningful visual effect in self-test
            ToggleItem::Aux1
            | ToggleItem::Aux2
            | ToggleItem::SoakMode
            | ToggleItem::NormalOperation
            | ToggleItem::ClearNotification
            | ToggleItem::Light3
            | ToggleItem::Light4 => false,
            _ => false,
        }
    }

    /// Get the current simulated status for publishing.
    pub fn status(&self) -> &StatusUpdate {
        &self.status
    }
}

/// Cycle a pump state: Off -> Low -> High -> Off.
fn cycle_pump_state(state: PumpState) -> PumpState {
    match state {
        PumpState::Off => PumpState::Low,
        PumpState::Low => PumpState::High,
        PumpState::High => PumpState::Off,
        _ => PumpState::Off,
    }
}
