//! MQTT state change detection.
//!
//! Compares two `StatusUpdate` instances to determine whether the spa state
//! has changed enough to warrant publishing to the MQTT broker. This avoids
//! republishing identical state on every status frame (~1 per second).

use launa_protocol::status::StatusUpdate;

/// Returns `true` if the two status updates differ in any published field.
///
/// Compared fields:
/// - `current_temp`, `set_temp`, `is_heating`
/// - `pumps`, `lights`, `blower`, `circ_pump`, `mister`
/// - `is_hold`, `heating_mode`, `temp_range`, `hold_timer_minutes`
///
/// If `prev` is `None` (first publish or after mode change reset), returns `true`.
pub fn status_changed(prev: Option<&StatusUpdate>, current: &StatusUpdate) -> bool {
    match prev {
        None => true,
        Some(prev) => status_fields_differ(prev, current),
    }
}

/// Compare all published fields of two status updates.
fn status_fields_differ(prev: &StatusUpdate, current: &StatusUpdate) -> bool {
    prev.current_temp != current.current_temp
        || prev.set_temp != current.set_temp
        || prev.is_heating != current.is_heating
        || prev.pumps != current.pumps
        || prev.lights != current.lights
        || prev.blower != current.blower
        || prev.circ_pump != current.circ_pump
        || prev.mister != current.mister
        || prev.is_hold != current.is_hold
        || prev.heating_mode != current.heating_mode
        || prev.temp_range != current.temp_range
        || prev.hold_timer_minutes != current.hold_timer_minutes
}

#[cfg(test)]
mod tests {
    use super::*;
    use launa_protocol::status::{
        HeatingMode, PumpState, StatusUpdate, TempRange, TemperatureScale, TimeFormat,
    };
    use launa_protocol::Temperature;

    fn sample_status() -> StatusUpdate {
        StatusUpdate {
            current_temp: Some(Temperature::fahrenheit(100.0)),
            set_temp: Temperature::fahrenheit(104.0),
            hour: 14,
            minute: 30,
            heating_mode: HeatingMode::Ready,
            temperature_scale: TemperatureScale::Fahrenheit,
            time_format: TimeFormat::Hour24,
            filter_mode: 0,
            is_heating: true,
            temp_range: TempRange::High,
            pumps: [
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
                PumpState::Off,
            ],
            circ_pump: false,
            blower: false,
            mister: false,
            lights: [false, false],
            is_priming: false,
            is_hold: false,
            notification_type: 0,
            panel_locked: false,
            settings_lock: false,
            m8_cycle_time: 0,
            sensor_a_temp: None,
            sensor_b_temp: None,
            hold_timer_minutes: None,
        }
    }

    // --- Base cases ---

    #[test]
    fn test_none_prev_always_changed() {
        let status = sample_status();
        assert!(status_changed(None, &status));
    }

    #[test]
    fn test_identical_states_not_changed() {
        let status = sample_status();
        assert!(!status_changed(Some(&status), &status));
    }

    // --- Individual field changes ---

    #[test]
    fn test_current_temp_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.current_temp = Some(Temperature::fahrenheit(101.0));
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_current_temp_none_to_some_detected() {
        let mut base = sample_status();
        base.current_temp = None;
        let mut changed = base.clone();
        changed.current_temp = Some(Temperature::fahrenheit(100.0));
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_current_temp_some_to_none_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.current_temp = None;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_set_temp_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.set_temp = Temperature::fahrenheit(106.0);
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_is_heating_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.is_heating = false;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_pump_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.pumps[0] = PumpState::Low;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_pump_high_vs_low_detected() {
        let mut base = sample_status();
        base.pumps[0] = PumpState::Low;
        let mut changed = base.clone();
        changed.pumps[0] = PumpState::High;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_lights_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.lights[0] = true;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_light2_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.lights[1] = true;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_blower_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.blower = true;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_circ_pump_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.circ_pump = true;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_mister_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.mister = true;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_is_hold_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.is_hold = true;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_heating_mode_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.heating_mode = HeatingMode::Rest;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_heating_mode_ready_in_rest_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.heating_mode = HeatingMode::ReadyInRest;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_temp_range_change_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.temp_range = TempRange::Low;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_hold_timer_minutes_none_to_some_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.hold_timer_minutes = Some(30);
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_hold_timer_minutes_some_to_none_detected() {
        let mut base = sample_status();
        base.hold_timer_minutes = Some(30);
        let mut changed = base.clone();
        changed.hold_timer_minutes = None;
        assert!(status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_hold_timer_minutes_value_change_detected() {
        let mut base = sample_status();
        base.hold_timer_minutes = Some(30);
        let mut changed = base.clone();
        changed.hold_timer_minutes = Some(20);
        assert!(status_changed(Some(&base), &changed));
    }

    // --- Non-published fields should NOT trigger change ---

    #[test]
    fn test_hour_change_not_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.hour = 15;
        assert!(!status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_minute_change_not_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.minute = 45;
        assert!(!status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_notification_type_change_not_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.notification_type = 4;
        assert!(!status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_is_priming_change_not_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.is_priming = true;
        assert!(!status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_panel_locked_change_not_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.panel_locked = true;
        assert!(!status_changed(Some(&base), &changed));
    }

    #[test]
    fn test_sensor_a_temp_change_not_detected() {
        let base = sample_status();
        let mut changed = base.clone();
        changed.sensor_a_temp = Some(Temperature::fahrenheit(98.0));
        assert!(!status_changed(Some(&base), &changed));
    }
}
