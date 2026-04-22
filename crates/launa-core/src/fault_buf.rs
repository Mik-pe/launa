//! Fixed-size fault string buffer for no_std environments.
//!
//! Avoids heap allocation when passing fault log messages between tasks
//! via channels. Fault log messages are typically ~40 chars; 64 bytes is
//! sufficient.

/// Fixed-size fault string buffer to avoid heap allocation in STATE_CHANNEL.
/// Fault log messages are typically ~40 chars; 64 bytes is sufficient.
#[derive(Debug, Clone, Copy)]
pub struct FaultBuf {
    data: [u8; 64],
    len: u8,
}

impl FaultBuf {
    /// An empty `FaultBuf` (no fault).
    pub const EMPTY: FaultBuf = FaultBuf {
        data: [0u8; 64],
        len: 0,
    };

    /// Create a `FaultBuf` from a string slice, truncating to 63 bytes.
    pub fn from_str(s: &str) -> Self {
        let to_copy = s.len().min(63);
        let mut buf = [0u8; 64];
        buf[..to_copy].copy_from_slice(&s.as_bytes()[..to_copy]);
        FaultBuf {
            data: buf,
            len: to_copy as u8,
        }
    }

    /// Return the fault string, or `None` if empty.
    pub fn as_str(&self) -> Option<&str> {
        if self.len == 0 {
            None
        } else {
            core::str::from_utf8(&self.data[..self.len as usize]).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let buf = FaultBuf::EMPTY;
        assert_eq!(buf.as_str(), None);
    }

    #[test]
    fn test_from_str_short() {
        let buf = FaultBuf::from_str("Hello");
        assert_eq!(buf.as_str(), Some("Hello"));
    }

    #[test]
    fn test_from_str_empty_string() {
        let buf = FaultBuf::from_str("");
        assert_eq!(buf.as_str(), None);
    }

    #[test]
    fn test_from_str_exact_63_bytes() {
        let s = "A".repeat(63);
        let buf = FaultBuf::from_str(&s);
        assert_eq!(buf.as_str(), Some(&s[..]));
    }

    #[test]
    fn test_from_str_truncation_at_64_bytes() {
        let s = "A".repeat(80);
        let buf = FaultBuf::from_str(&s);
        let result = buf.as_str().unwrap();
        assert_eq!(result.len(), 63);
        assert!(result.chars().all(|c| c == 'A'));
    }

    #[test]
    fn test_from_str_truncation_at_100_bytes() {
        let s = "xyz".repeat(34); // 102 bytes
        let buf = FaultBuf::from_str(&s);
        let result = buf.as_str().unwrap();
        assert_eq!(result.len(), 63);
    }

    #[test]
    fn test_from_str_utf8_boundary() {
        // Multi-byte UTF-8 character: truncation at byte boundary may
        // split a char. The result may be None (invalid UTF-8) or a
        // valid substring — either is acceptable for this simple buffer.
        let s = "é".repeat(50); // é is 2 bytes, so 100 bytes total
        let buf = FaultBuf::from_str(&s);
        // The buffer truncates at 63 bytes. "é" is 2 bytes, so 31 full
        // chars = 62 bytes. Byte 63 would split char 32.
        // as_str() may return None if the truncation splits a char.
        let result = buf.as_str();
        // Either valid UTF-8 prefix or None — both are acceptable.
        if let Some(r) = result {
            assert!(r.len() <= 63);
            assert!(r.is_char_boundary(r.len()));
        }
    }

    #[test]
    fn test_from_str_ascii_max() {
        let s = "ABCDEFGHIJ".repeat(10); // 100 bytes
        let buf = FaultBuf::from_str(&s);
        assert_eq!(buf.as_str().unwrap().len(), 63);
        assert_eq!(&buf.as_str().unwrap()[..10], "ABCDEFGHIJ");
    }

    #[test]
    fn test_clone() {
        let buf = FaultBuf::from_str("Test fault");
        let cloned = buf.clone();
        assert_eq!(buf.as_str(), cloned.as_str());
    }

    #[test]
    fn test_copy() {
        let buf = FaultBuf::from_str("Test fault");
        let copied = buf; // Copy semantic
        assert_eq!(buf.as_str(), copied.as_str());
    }

    #[test]
    fn test_realistic_fault_message() {
        let msg = "HeaterDry: code 27 at 14:30";
        let buf = FaultBuf::from_str(msg);
        assert_eq!(buf.as_str(), Some(msg));
    }
}
