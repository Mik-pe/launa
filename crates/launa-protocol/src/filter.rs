/// Filter cycles response parser for `0A BF 23` messages.
///
/// Payload layout:
/// ```text
/// Offset: 0  1  2  3  4  5  6  7
/// Field:  1H 1M 1D 1E 2H 2M 2D 2E
/// ```
/// - Filter 2 start hour (offset 4): high bit = enable flag

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterCycles {
    pub filter1: FilterCycle,
    pub filter2: FilterCycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterCycle {
    /// Start hour (0-23)
    pub start_hour: u8,
    /// Start minute (0-59)
    pub start_minute: u8,
    /// Duration hours
    pub duration_hours: u8,
    /// Duration minutes
    pub duration_minutes: u8,
    /// Whether this filter cycle is enabled.
    /// Only meaningful for filter 2 (encoded in high bit of start_hour).
    /// Always true for filter 1.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
    UnexpectedLength(usize),
}

impl FilterCycles {
    /// Parse filter cycles from the frame payload.
    /// Message type is `0A BF 23`.
    /// Payload is 8 bytes (offsets 0-7).
    pub fn parse(payload: &[u8]) -> Result<Self, FilterError> {
        if payload.len() < 8 {
            return Err(FilterError::UnexpectedLength(payload.len()));
        }

        // Filter 1: offsets 0-3
        let filter1 = FilterCycle {
            start_hour: payload[0],
            start_minute: payload[1],
            duration_hours: payload[2],
            duration_minutes: payload[3],
            enabled: true, // Filter 1 is always enabled
        };

        // Filter 2: offsets 4-7
        // High bit of byte 4 is the enable flag for filter 2
        let f2_enabled = payload[4] & 0x80 != 0;
        let f2_start_hour = payload[4] & 0x7F;

        let filter2 = FilterCycle {
            start_hour: f2_start_hour,
            start_minute: payload[5],
            duration_hours: payload[6],
            duration_minutes: payload[7],
            enabled: f2_enabled,
        };

        Ok(FilterCycles {
            filter1,
            filter2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filter_cycles() {
        // Filter 1: start 08:00, duration 4h00m
        // Filter 2: start 16:00, duration 2h00m, enabled
        let payload: &[u8] = &[
            0x08, 0x00, 0x04, 0x00, // Filter 1
            0x90, 0x00, 0x02, 0x00, // Filter 2 (0x90 = enabled + hour 16)
        ];

        let cycles = FilterCycles::parse(payload).unwrap();
        assert_eq!(cycles.filter1.start_hour, 8);
        assert_eq!(cycles.filter1.start_minute, 0);
        assert_eq!(cycles.filter1.duration_hours, 4);
        assert_eq!(cycles.filter1.duration_minutes, 0);
        assert!(cycles.filter1.enabled);

        assert_eq!(cycles.filter2.start_hour, 16);
        assert_eq!(cycles.filter2.start_minute, 0);
        assert_eq!(cycles.filter2.duration_hours, 2);
        assert_eq!(cycles.filter2.duration_minutes, 0);
        assert!(cycles.filter2.enabled);
    }

    #[test]
    fn test_parse_filter_cycles_filter2_disabled() {
        // Filter 2: start 16:00, disabled (high bit = 0)
        let payload: &[u8] = &[
            0x08, 0x00, 0x04, 0x00, // Filter 1
            0x10, 0x00, 0x02, 0x00, // Filter 2 (0x10 = no enable + hour 16)
        ];

        let cycles = FilterCycles::parse(payload).unwrap();
        assert_eq!(cycles.filter2.start_hour, 16);
        assert!(!cycles.filter2.enabled);
    }

    #[test]
    fn test_parse_filter_cycles_too_short() {
        let payload = [0u8; 4];
        let result = FilterCycles::parse(&payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_filter_cycles_with_minutes() {
        // Filter 1: start 07:30, duration 2h15m
        // Filter 2: start 14:45, duration 3h30m, enabled
        let payload: &[u8] = &[
            0x07, 0x1E, 0x02, 0x0F, // Filter 1
            0x8E, 0x2D, 0x03, 0x1E, // Filter 2 (0x80 | 14 = 0x8E)
        ];

        let cycles = FilterCycles::parse(payload).unwrap();
        assert_eq!(cycles.filter1.start_hour, 7);
        assert_eq!(cycles.filter1.start_minute, 30);
        assert_eq!(cycles.filter1.duration_hours, 2);
        assert_eq!(cycles.filter1.duration_minutes, 15);

        assert_eq!(cycles.filter2.start_hour, 14);
        assert_eq!(cycles.filter2.start_minute, 45);
        assert_eq!(cycles.filter2.duration_hours, 3);
        assert_eq!(cycles.filter2.duration_minutes, 30);
        assert!(cycles.filter2.enabled);
    }
}
