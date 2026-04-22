//! Scale-aware temperature type that eliminates °F/°C comparison bugs.
//!
//! `Temperature` wraps a value with its scale, providing conversion, comparison,
//! and wire encode/decode. All arithmetic and comparisons convert to a common
//! scale internally, so mixing Celsius and Fahrenheit values is safe.

use core::fmt;

use crate::status::TemperatureScale;

/// A temperature value with its scale baked in.
///
/// # Wire encoding
///
/// The Balboa protocol encodes temperatures as raw bytes:
/// - Fahrenheit: direct (100°F → byte 100)
/// - Celsius: doubled (38°C → byte 76)
///
/// Use [`Temperature::from_wire`] and [`Temperature::to_wire`] for encode/decode.
#[derive(Debug, Clone, Copy)]
pub struct Temperature {
    value: f32,
    scale: TemperatureScale,
}

impl Temperature {
    /// Create a Celsius temperature.
    pub const fn celsius(value: f32) -> Self {
        Self {
            value,
            scale: TemperatureScale::Celsius,
        }
    }

    /// Create a Fahrenheit temperature.
    pub const fn fahrenheit(value: f32) -> Self {
        Self {
            value,
            scale: TemperatureScale::Fahrenheit,
        }
    }

    /// Decode a temperature from a raw wire byte.
    ///
    /// Fahrenheit: `raw` is the value directly.
    /// Celsius: `raw` is the value × 2 (e.g., 38°C → 76).
    pub fn from_wire(raw: u8, scale: TemperatureScale) -> Self {
        let value = match scale {
            TemperatureScale::Fahrenheit => raw as f32,
            TemperatureScale::Celsius => raw as f32 / 2.0,
        };
        Self { value, scale }
    }

    /// Encode this temperature to a raw wire byte.
    ///
    /// Fahrenheit: direct cast.
    /// Celsius: multiply by 2 and round.
    pub fn to_wire(self) -> u8 {
        let raw = match self.scale {
            TemperatureScale::Fahrenheit => self.value,
            TemperatureScale::Celsius => self.value * 2.0,
        };
        (raw + 0.5) as u8
    }

    /// The value in Celsius, converting if necessary.
    pub fn to_celsius(self) -> f32 {
        match self.scale {
            TemperatureScale::Celsius => self.value,
            TemperatureScale::Fahrenheit => (self.value - 32.0) * 5.0 / 9.0,
        }
    }

    /// The value in Fahrenheit, converting if necessary.
    pub fn to_fahrenheit(self) -> f32 {
        match self.scale {
            TemperatureScale::Fahrenheit => self.value,
            TemperatureScale::Celsius => self.value * 9.0 / 5.0 + 32.0,
        }
    }

    /// Convert to a different scale.
    pub fn convert(self, target: TemperatureScale) -> Self {
        if self.scale == target {
            return self;
        }
        match target {
            TemperatureScale::Celsius => Temperature::celsius(self.to_celsius()),
            TemperatureScale::Fahrenheit => Temperature::fahrenheit(self.to_fahrenheit()),
        }
    }

    /// The scale of this temperature.
    pub fn scale(self) -> TemperatureScale {
        self.scale
    }

    /// The raw numeric value in whatever scale this temperature was created in.
    ///
    /// Use this for display/serialization where you already know the scale context.
    /// For comparisons and arithmetic, prefer `to_celsius()` or `to_fahrenheit()`.
    pub fn raw_value(self) -> f32 {
        self.value
    }

    /// Set the raw value, keeping the same scale.
    pub fn set_raw_value(&mut self, value: f32) {
        self.value = value;
    }
}

impl PartialEq for Temperature {
    fn eq(&self, other: &Self) -> bool {
        self.to_fahrenheit() == other.to_fahrenheit()
    }
}

impl PartialOrd for Temperature {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.to_fahrenheit().partial_cmp(&other.to_fahrenheit())
    }
}

impl fmt::Display for Temperature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fahrenheit_round_trip() {
        let temp = Temperature::fahrenheit(100.0);
        assert_eq!(temp.to_wire(), 100);
        assert_eq!(
            Temperature::from_wire(100, TemperatureScale::Fahrenheit),
            temp
        );
    }

    #[test]
    fn test_celsius_round_trip() {
        let temp = Temperature::celsius(38.0);
        assert_eq!(temp.to_wire(), 76);
        assert_eq!(Temperature::from_wire(76, TemperatureScale::Celsius), temp);
    }

    #[test]
    fn test_celsius_half_degree() {
        let temp = Temperature::celsius(37.5);
        assert_eq!(temp.to_wire(), 75); // 37.5 * 2 = 75
        let decoded = Temperature::from_wire(75, TemperatureScale::Celsius);
        assert_eq!(decoded, Temperature::celsius(37.5));
    }

    #[test]
    fn test_conversion_f_to_c() {
        let temp = Temperature::fahrenheit(100.0);
        let c = temp.to_celsius();
        assert!((c - 37.7778).abs() < 0.01, "100°F = 37.78°C, got {c}");
    }

    #[test]
    fn test_conversion_c_to_f() {
        let temp = Temperature::celsius(38.0);
        let f = temp.to_fahrenheit();
        assert!((f - 100.4).abs() < 0.01, "38°C = 100.4°F, got {f}");
    }

    #[test]
    fn test_convert_method() {
        let temp = Temperature::fahrenheit(100.0);
        let celsius = temp.convert(TemperatureScale::Celsius);
        assert_eq!(celsius.scale(), TemperatureScale::Celsius);
        assert!((celsius.raw_value() - 37.7778).abs() < 0.01);
    }

    #[test]
    fn test_convert_same_scale_noop() {
        let temp = Temperature::fahrenheit(100.0);
        let same = temp.convert(TemperatureScale::Fahrenheit);
        assert_eq!(same.raw_value(), 100.0);
    }

    #[test]
    fn test_equality_same_scale() {
        assert_eq!(
            Temperature::fahrenheit(100.0),
            Temperature::fahrenheit(100.0)
        );
        assert_eq!(Temperature::celsius(38.0), Temperature::celsius(38.0));
    }

    #[test]
    fn test_equality_cross_scale() {
        // 32°F = 0°C
        assert_eq!(Temperature::fahrenheit(32.0), Temperature::celsius(0.0));
        // 100.4°F ≈ 38°C
        let f = Temperature::fahrenheit(100.4);
        let c = Temperature::celsius(38.0);
        assert_eq!(f, c);
    }

    #[test]
    fn test_ordering() {
        assert!(Temperature::fahrenheit(100.0) > Temperature::fahrenheit(80.0));
        assert!(Temperature::celsius(40.0) > Temperature::celsius(30.0));
        assert!(Temperature::fahrenheit(100.0) > Temperature::celsius(20.0));
    }

    #[test]
    fn test_raw_value() {
        assert_eq!(Temperature::celsius(38.0).raw_value(), 38.0);
        assert_eq!(Temperature::fahrenheit(100.0).raw_value(), 100.0);
    }

    #[test]
    fn test_scale() {
        assert_eq!(Temperature::celsius(0.0).scale(), TemperatureScale::Celsius);
        assert_eq!(
            Temperature::fahrenheit(0.0).scale(),
            TemperatureScale::Fahrenheit
        );
    }
}
