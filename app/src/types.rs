//! Shared types used across app modules.

use launa_protocol::status::StatusUpdate;

/// State update message sent from the main loop to the MQTT task via STATE_CHANNEL.
///
/// Replaces the previous 5-tuple `(StatusUpdate, FaultBuf, bool, bool, bool)` with
/// named fields for readability.
pub(crate) struct StateMessage {
    pub status: StatusUpdate,
    pub fault: FaultBuf,
    pub recovering_from_stale: bool,
    pub self_test: bool,
    pub sniff_mode: bool,
}

/// Fixed-size fault string buffer to avoid heap allocation in STATE_CHANNEL.
/// Fault log messages are typically ~40 chars; 64 bytes is sufficient.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FaultBuf {
    data: [u8; 64],
    len: u8,
}

impl FaultBuf {
    pub(crate) const EMPTY: FaultBuf = FaultBuf {
        data: [0u8; 64],
        len: 0,
    };

    pub(crate) fn from_str(s: &str) -> Self {
        let to_copy = s.len().min(63);
        let mut buf = [0u8; 64];
        buf[..to_copy].copy_from_slice(&s.as_bytes()[..to_copy]);
        FaultBuf {
            data: buf,
            len: to_copy as u8,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        if self.len == 0 {
            None
        } else {
            core::str::from_utf8(&self.data[..self.len as usize]).ok()
        }
    }
}
