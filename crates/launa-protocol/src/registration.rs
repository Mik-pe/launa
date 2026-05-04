//! Client registration protocol types and state machine.
//!
//! The Balboa BP6013G1 spa controller assigns channel IDs to devices on the
//! RS-485 bus. The protocol supports two paths:
//!
//! ## New Client Registration
//! 1. Spa sends `NewClientQuery` (FE BF 00)
//! 2. Client responds `NewClientResponse` (FE BF 01 02 <hash_hi> <hash_lo>)
//! 3. Spa replies `ClientIdAssignment` (FE BF 02 <channel> <hash_hi> <hash_lo>)
//! 4. Client acknowledges `ClientIdAck` (<channel> BF 03)
//!
//! ## Existing Client Reconnection
//! 1. Client sends `ExistingClientRequest` (FE BF 04 <channel> <hash_hi> <hash_lo>)
//! 2. Spa replies `ExistingClientResponse` (<channel> BF 05 04 <channel> <hash_hi> <hash_lo>)
//! 3. Client is now registered on the previous channel
//!
//! The `client_hash` (2 bytes) identifies each device uniquely so multiple
//! clients on the same bus can distinguish which assignment is theirs.

extern crate alloc;

use crate::frame::{FrameEncoder, FrameError};

// ---------------------------------------------------------------------------
// Channel type
// ---------------------------------------------------------------------------

/// Valid client channel range (CTS-based, most common).
pub const CLIENT_CTS_RANGE: core::ops::RangeInclusive<u8> = 0x10..=0x2F;
/// Client channel range without ClearToSend.
pub const CLIENT_NO_CTS_RANGE: core::ops::RangeInclusive<u8> = 0x30..=0x3F;

/// A typed Balboa RS-485 channel identifier.
///
/// The Balboa protocol uses the first byte of the message type (`XX BF`) to
/// identify which channel a frame is addressed to. Well-known channel values:
///
/// | Value    | Meaning                          |
/// |----------|----------------------------------|
/// | `0x0A`   | WiFi module                      |
/// | `0x10-0x2F` | Client with ClearToSend       |
/// | `0x30-0x3F` | Client without ClearToSend    |
/// | `0xFE`   | Multicast channel assignment     |
/// | `0xFF`   | Multicast broadcast (status)     |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// WiFi module channel (0x0A).
    WifiModule,
    /// Client with ClearToSend support (0x10-0x2F).
    /// The spa sends `<channel> BF 06` (ClearToSend) before this client can send.
    Client(u8),
    /// Client without ClearToSend (0x30-0x3F).
    ClientNoCTS(u8),
    /// Multicast channel assignment (0xFE) — used during registration.
    MulticastChannelAssignment,
    /// Multicast broadcast (0xFF) — status frames.
    MulticastBroadcast,
    /// Unknown/reserved channel value.
    Unknown(u8),
}

impl Channel {
    /// Whether this channel uses ClearToSend flow control.
    pub fn has_cts(&self) -> bool {
        matches!(self, Channel::Client(_))
    }

    /// Whether this is a client channel (with or without CTS).
    pub fn is_client(&self) -> bool {
        matches!(self, Channel::Client(_) | Channel::ClientNoCTS(_))
    }

    /// Create a CTS client channel from a 0-based index.
    /// Returns `None` if the index would place the channel outside `0x10-0x2F`.
    pub fn new_client_channel(index: usize) -> Option<Self> {
        let raw = 0x10u8.checked_add(index as u8)?;
        if CLIENT_CTS_RANGE.contains(&raw) {
            Some(Channel::Client(raw))
        } else {
            None
        }
    }
}

impl From<u8> for Channel {
    fn from(value: u8) -> Self {
        match value {
            0x0A => Channel::WifiModule,
            0xFE => Channel::MulticastChannelAssignment,
            0xFF => Channel::MulticastBroadcast,
            v if CLIENT_CTS_RANGE.contains(&v) => Channel::Client(v),
            v if CLIENT_NO_CTS_RANGE.contains(&v) => Channel::ClientNoCTS(v),
            v => Channel::Unknown(v),
        }
    }
}

impl From<Channel> for u8 {
    fn from(channel: Channel) -> u8 {
        match channel {
            Channel::WifiModule => 0x0A,
            Channel::Client(v) => v,
            Channel::ClientNoCTS(v) => v,
            Channel::MulticastChannelAssignment => 0xFE,
            Channel::MulticastBroadcast => 0xFF,
            Channel::Unknown(v) => v,
        }
    }
}

impl From<&Channel> for u8 {
    fn from(channel: &Channel) -> u8 {
        u8::from(*channel)
    }
}

impl core::fmt::Display for Channel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Channel::WifiModule => write!(f, "WiFi(0x0A)"),
            Channel::Client(v) => write!(f, "Client(0x{:02X})", v),
            Channel::ClientNoCTS(v) => write!(f, "ClientNoCTS(0x{:02X})", v),
            Channel::MulticastChannelAssignment => write!(f, "Reg(0xFE)"),
            Channel::MulticastBroadcast => write!(f, "Broadcast(0xFF)"),
            Channel::Unknown(v) => write!(f, "Unknown(0x{:02X})", v),
        }
    }
}

// ---------------------------------------------------------------------------
// Typed registration messages
// ---------------------------------------------------------------------------

/// A parsed registration protocol message from the RS-485 bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationMessage {
    /// Spa asks: "any new clients?" — FE BF 00
    NewClientQuery,

    /// Client responds: "I'm here, assign me an ID" — FE BF 01 <device_type> <hash_hi> <hash_lo>
    NewClientResponse {
        device_type: u8,
        client_hash: [u8; 2],
    },

    /// Spa assigns a channel to a specific client — FE BF 02 <channel> <hash_hi> <hash_lo>
    ClientIdAssignment { channel: u8, client_hash: [u8; 2] },

    /// Client acknowledges the assigned channel — <channel> BF 03
    ClientIdAck { channel: u8 },

    /// Client requests reconnection on a previously-assigned channel.
    /// Known forms:
    /// - FE BF 04 (empty payload, as in jasta's implementation)
    /// - FE BF 04 <channel> <hash_hi> <hash_lo> (with identity, unverified)
    ExistingClientRequest {
        channel: Option<u8>,
        client_hash: Option<[u8; 2]>,
    },

    /// Spa confirms reconnection — <channel> BF 05 04 <channel> <hash_hi> <hash_lo>
    ExistingClientResponse { channel: u8, client_hash: [u8; 2] },

    /// Spa tells a specific client it can send — <channel> BF 06
    ClearToSend { channel: u8 },
}

/// Error when parsing a registration message from raw frame bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationParseError {
    /// Payload too short for the identified message type.
    TruncatedPayload,
    /// The frame is not a registration-related message.
    NotRegistrationFrame,
    /// Unknown registration sub-type byte.
    UnknownSubType(u8),
}

impl RegistrationMessage {
    /// Parse a registration message from raw frame type and payload bytes.
    ///
    /// Returns `Err(NotRegistrationFrame)` for frames that aren't part of the
    /// registration protocol (caller should dispatch to other handlers).
    pub fn parse(msg_type: [u8; 2], payload: &[u8]) -> Result<Self, RegistrationParseError> {
        match msg_type {
            // FE BF frames: registration broadcast channel
            [0xFE, 0xBF] => {
                if payload.is_empty() {
                    return Err(RegistrationParseError::TruncatedPayload);
                }
                match payload[0] {
                    0x00 => Ok(RegistrationMessage::NewClientQuery),
                    0x01 => {
                        if payload.len() < 4 {
                            return Err(RegistrationParseError::TruncatedPayload);
                        }
                        Ok(RegistrationMessage::NewClientResponse {
                            device_type: payload[1],
                            client_hash: [payload[2], payload[3]],
                        })
                    }
                    0x02 => {
                        if payload.len() < 4 {
                            // Legacy: some spa firmware sends only FE BF 02 <channel>
                            // without the hash. Accept 2-byte form for compatibility.
                            if payload.len() < 2 {
                                return Err(RegistrationParseError::TruncatedPayload);
                            }
                            return Ok(RegistrationMessage::ClientIdAssignment {
                                channel: payload[1],
                                client_hash: [0x00, 0x00],
                            });
                        }
                        Ok(RegistrationMessage::ClientIdAssignment {
                            channel: payload[1],
                            client_hash: [payload[2], payload[3]],
                        })
                    }
                    0x04 => {
                        // Two known forms:
                        // - FE BF 04 (empty payload — jasta's implementation)
                        // - FE BF 04 <channel> <hash_hi> <hash_lo> (with identity)
                        if payload.len() >= 4 {
                            Ok(RegistrationMessage::ExistingClientRequest {
                                channel: Some(payload[1]),
                                client_hash: Some([payload[2], payload[3]]),
                            })
                        } else {
                            // Empty or minimal payload
                            Ok(RegistrationMessage::ExistingClientRequest {
                                channel: None,
                                client_hash: None,
                            })
                        }
                    }
                    other => Err(RegistrationParseError::UnknownSubType(other)),
                }
            }
            // <channel> BF frames: channel-specific messages
            [channel, 0xBF] if channel != 0xFE && channel != 0xFF && channel != 0x0A => {
                if payload.is_empty() {
                    return Err(RegistrationParseError::TruncatedPayload);
                }
                match payload[0] {
                    0x03 => Ok(RegistrationMessage::ClientIdAck { channel }),
                    0x05 => {
                        // ExistingClientResponse: <channel> BF 05 04 <channel> <hash_hi> <hash_lo>
                        // Payload: [0x05, 0x04, <channel>, <hash_hi>, <hash_lo>] = 5 bytes
                        if payload.len() < 5 || payload[1] != 0x04 {
                            return Err(RegistrationParseError::TruncatedPayload);
                        }
                        Ok(RegistrationMessage::ExistingClientResponse {
                            channel: payload[2],
                            client_hash: [payload[3], payload[4]],
                        })
                    }
                    0x06 => Ok(RegistrationMessage::ClearToSend { channel }),
                    _ => Err(RegistrationParseError::NotRegistrationFrame),
                }
            }
            _ => Err(RegistrationParseError::NotRegistrationFrame),
        }
    }

    /// Encode this message into a raw frame suitable for transmission.
    pub fn encode(&self) -> Result<alloc::vec::Vec<u8>, FrameError> {
        let (msg_type, payload) = self.encode_parts()?;
        FrameEncoder::encode(msg_type, &payload)
    }

    /// Returns the (msg_type, payload) pair without frame encoding.
    /// Useful when the caller wraps in a FrameEncoder separately.
    pub fn encode_parts(&self) -> Result<([u8; 2], alloc::vec::Vec<u8>), FrameError> {
        match self {
            RegistrationMessage::NewClientQuery => Ok(([0xFE, 0xBF], alloc::vec![0x00])),
            RegistrationMessage::NewClientResponse {
                device_type,
                client_hash,
            } => Ok((
                [0xFE, 0xBF],
                alloc::vec![0x01, *device_type, client_hash[0], client_hash[1]],
            )),
            RegistrationMessage::ClientIdAssignment {
                channel,
                client_hash,
            } => Ok((
                [0xFE, 0xBF],
                alloc::vec![0x02, *channel, client_hash[0], client_hash[1]],
            )),
            RegistrationMessage::ClientIdAck { channel } => {
                Ok(([*channel, 0xBF], alloc::vec![0x03]))
            }
            RegistrationMessage::ExistingClientRequest {
                channel,
                client_hash,
            } => {
                let mut payload = alloc::vec![0x04];
                if let Some(ch) = channel {
                    payload.push(*ch);
                    if let Some(hash) = client_hash {
                        payload.push(hash[0]);
                        payload.push(hash[1]);
                    }
                }
                Ok(([0xFE, 0xBF], payload))
            }
            RegistrationMessage::ExistingClientResponse {
                channel,
                client_hash,
            } => Ok((
                [*channel, 0xBF],
                alloc::vec![0x05, 0x04, *channel, client_hash[0], client_hash[1]],
            )),
            RegistrationMessage::ClearToSend { channel } => {
                Ok(([*channel, 0xBF], alloc::vec![0x06]))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registration state machine
// ---------------------------------------------------------------------------

/// Client ID registration state.
///
/// Tracks the current phase of the RS-485 bus registration handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    /// Waiting for the spa to send the new-client query frame.
    WaitingForQuery,
    /// Client has requested a new ID; waiting for the spa to assign one.
    WaitingForAssignment,
    /// Client sent an ExistingClientRequest; waiting for the spa to confirm.
    WaitingForExistingResponse,
    /// Registration complete; the client has been assigned a channel.
    Registered { client_id: u8 },
}

/// Action the caller should take after processing a registration message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationAction {
    /// Send a NewClientResponse (FE BF 01 02 <hash>).
    SendNewClientResponse,
    /// Send a ClientIdAck (<channel> BF 03).
    SendClientIdAck { client_id: u8 },
    /// Send an ExistingClientRequest (FE BF 04 <channel> <hash>).
    /// The encoded message is provided so the caller can build the frame.
    SendExistingClientRequest {
        /// The pre-built message to encode and send.
        message: RegistrationMessage,
    },
    /// Registration is complete; no frame to send.
    None,
}

/// State machine that drives the RS-485 registration handshake.
///
/// Validates `client_hash` in assignment responses so multiple devices on
/// the same bus don't accidentally accept each other's channel assignment.
pub struct RegistrationStateMachine {
    state: RegistrationState,
    client_hash: [u8; 2],
    /// Previously assigned channel, used for existing client reconnection.
    /// `None` if this is the first boot or the channel was never persisted.
    previous_channel: Option<u8>,
}

impl RegistrationStateMachine {
    /// Create a new state machine with the given client hash.
    ///
    /// The hash should be derived from unique device identity (e.g. ESP32 MAC).
    pub fn new(client_hash: [u8; 2]) -> Self {
        RegistrationStateMachine {
            state: RegistrationState::WaitingForQuery,
            client_hash,
            previous_channel: None,
        }
    }

    /// Create a state machine that will attempt existing client reconnection
    /// on the given channel before falling back to new-client registration.
    pub fn with_previous_channel(client_hash: [u8; 2], channel: u8) -> Self {
        RegistrationStateMachine {
            state: RegistrationState::WaitingForQuery,
            client_hash,
            previous_channel: Some(channel),
        }
    }

    pub fn state(&self) -> &RegistrationState {
        &self.state
    }

    pub fn is_registered(&self) -> bool {
        matches!(self.state, RegistrationState::Registered { .. })
    }

    pub fn client_id(&self) -> Option<u8> {
        match self.state {
            RegistrationState::Registered { client_id } => Some(client_id),
            _ => None,
        }
    }

    pub fn client_hash(&self) -> [u8; 2] {
        self.client_hash
    }

    pub fn previous_channel(&self) -> Option<u8> {
        self.previous_channel
    }

    /// Process an incoming registration message.
    ///
    /// Returns the action the caller should take (e.g. send a response frame).
    /// The caller should call `RegistrationMessage::parse()` first, then pass
    /// the result here. If parsing returns `NotRegistrationFrame`, skip this.
    pub fn process(&mut self, msg: &RegistrationMessage) -> RegistrationAction {
        match self.state {
            RegistrationState::WaitingForQuery => match msg {
                RegistrationMessage::NewClientQuery => {
                    // If we have a previous channel, try existing client reconnection
                    if let Some(ch) = self.previous_channel {
                        self.state = RegistrationState::WaitingForExistingResponse;
                        return RegistrationAction::SendExistingClientRequest {
                            message: RegistrationMessage::ExistingClientRequest {
                                channel: Some(ch),
                                client_hash: Some(self.client_hash),
                            },
                        };
                    }
                    // Otherwise, do normal new-client flow
                    self.state = RegistrationState::WaitingForAssignment;
                    RegistrationAction::SendNewClientResponse
                }
                _ => RegistrationAction::None,
            },

            RegistrationState::WaitingForAssignment => match msg {
                RegistrationMessage::ClientIdAssignment {
                    channel,
                    client_hash,
                } => {
                    // Validate: only accept assignments for our hash (or legacy 0000)
                    if *client_hash != [0x00, 0x00] && *client_hash != self.client_hash {
                        // Hash mismatch — not our assignment, ignore
                        return RegistrationAction::None;
                    }
                    self.state = RegistrationState::Registered {
                        client_id: *channel,
                    };
                    RegistrationAction::SendClientIdAck {
                        client_id: *channel,
                    }
                }
                _ => RegistrationAction::None,
            },

            RegistrationState::WaitingForExistingResponse => match msg {
                RegistrationMessage::ExistingClientResponse {
                    channel,
                    client_hash,
                } => {
                    // Validate hash — reject responses meant for other devices
                    if *client_hash != self.client_hash {
                        return RegistrationAction::None;
                    }
                    // Spa confirmed our existing client — we're registered
                    self.state = RegistrationState::Registered {
                        client_id: *channel,
                    };
                    // No ACK needed for existing client — the response IS the confirmation
                    RegistrationAction::None
                }
                RegistrationMessage::NewClientQuery => {
                    // Spa didn't recognize our existing client request — fall back
                    self.state = RegistrationState::WaitingForAssignment;
                    RegistrationAction::SendNewClientResponse
                }
                _ => RegistrationAction::None,
            },

            RegistrationState::Registered { .. } => RegistrationAction::None,
        }
    }

    /// Process a raw frame directly (convenience wrapper).
    ///
    /// Parses the frame as a registration message, then runs the state machine.
    /// Returns `None` if the frame is not a registration message.
    pub fn process_raw(&mut self, msg_type: [u8; 2], payload: &[u8]) -> Option<RegistrationAction> {
        match RegistrationMessage::parse(msg_type, payload) {
            Ok(msg) => Some(self.process(&msg)),
            Err(RegistrationParseError::NotRegistrationFrame) => None,
            Err(_) => Some(RegistrationAction::None), // Malformed but registration-related
        }
    }

    /// Reset state (e.g. after a bus error or timeout).
    pub fn reset(&mut self) {
        self.state = RegistrationState::WaitingForQuery;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_HASH: [u8; 2] = [0xF1, 0x73];

    fn new_sm() -> RegistrationStateMachine {
        RegistrationStateMachine::new(TEST_HASH)
    }

    fn new_sm_with_previous(channel: u8) -> RegistrationStateMachine {
        RegistrationStateMachine::with_previous_channel(TEST_HASH, channel)
    }

    // --- RegistrationMessage parse tests ---

    #[test]
    fn test_parse_new_client_query() {
        let msg = RegistrationMessage::parse([0xFE, 0xBF], &[0x00]).unwrap();
        assert_eq!(msg, RegistrationMessage::NewClientQuery);
    }

    #[test]
    fn test_parse_new_client_response() {
        let msg = RegistrationMessage::parse([0xFE, 0xBF], &[0x01, 0x02, 0xF1, 0x73]).unwrap();
        assert_eq!(
            msg,
            RegistrationMessage::NewClientResponse {
                device_type: 0x02,
                client_hash: TEST_HASH,
            }
        );
    }

    #[test]
    fn test_parse_client_id_assignment_with_hash() {
        let msg = RegistrationMessage::parse([0xFE, 0xBF], &[0x02, 0x05, 0xF1, 0x73]).unwrap();
        assert_eq!(
            msg,
            RegistrationMessage::ClientIdAssignment {
                channel: 0x05,
                client_hash: TEST_HASH,
            }
        );
    }

    #[test]
    fn test_parse_client_id_assignment_legacy_no_hash() {
        // Legacy 2-byte form: FE BF 02 <channel>
        let msg = RegistrationMessage::parse([0xFE, 0xBF], &[0x02, 0x05]).unwrap();
        assert_eq!(
            msg,
            RegistrationMessage::ClientIdAssignment {
                channel: 0x05,
                client_hash: [0x00, 0x00],
            }
        );
    }

    #[test]
    fn test_parse_client_id_ack() {
        let msg = RegistrationMessage::parse([0x05, 0xBF], &[0x03]).unwrap();
        assert_eq!(msg, RegistrationMessage::ClientIdAck { channel: 0x05 });
    }

    #[test]
    fn test_parse_existing_client_request() {
        // Full form with channel and hash
        let msg = RegistrationMessage::parse([0xFE, 0xBF], &[0x04, 0x05, 0xF1, 0x73]).unwrap();
        assert_eq!(
            msg,
            RegistrationMessage::ExistingClientRequest {
                channel: Some(0x05),
                client_hash: Some(TEST_HASH),
            }
        );
    }

    #[test]
    fn test_parse_existing_client_request_empty() {
        // Empty payload form (jasta's implementation)
        let msg = RegistrationMessage::parse([0xFE, 0xBF], &[0x04]).unwrap();
        assert_eq!(
            msg,
            RegistrationMessage::ExistingClientRequest {
                channel: None,
                client_hash: None,
            }
        );
    }

    #[test]
    fn test_parse_existing_client_response() {
        let msg =
            RegistrationMessage::parse([0x05, 0xBF], &[0x05, 0x04, 0x05, 0xF1, 0x73]).unwrap();
        assert_eq!(
            msg,
            RegistrationMessage::ExistingClientResponse {
                channel: 0x05,
                client_hash: TEST_HASH,
            }
        );
    }

    #[test]
    fn test_parse_clear_to_send() {
        let msg = RegistrationMessage::parse([0x10, 0xBF], &[0x06]).unwrap();
        assert_eq!(msg, RegistrationMessage::ClearToSend { channel: 0x10 });
    }

    #[test]
    fn test_parse_not_registration_frame() {
        let result = RegistrationMessage::parse([0xFF, 0xAF], &[0x13]);
        assert_eq!(result, Err(RegistrationParseError::NotRegistrationFrame));
    }

    #[test]
    fn test_parse_reserved_types_not_registration() {
        // 0x0A BF → config messages, not registration
        assert_eq!(
            RegistrationMessage::parse([0x0A, 0xBF], &[0x04]),
            Err(RegistrationParseError::NotRegistrationFrame)
        );
        // 0xFF BF → not registration
        assert_eq!(
            RegistrationMessage::parse([0xFF, 0xBF], &[0x06]),
            Err(RegistrationParseError::NotRegistrationFrame)
        );
        // 0xFE BF with unknown sub-type
        assert_eq!(
            RegistrationMessage::parse([0xFE, 0xBF], &[0xFF]),
            Err(RegistrationParseError::UnknownSubType(0xFF))
        );
    }

    #[test]
    fn test_parse_truncated_payload() {
        assert_eq!(
            RegistrationMessage::parse([0xFE, 0xBF], &[]),
            Err(RegistrationParseError::TruncatedPayload)
        );
        assert_eq!(
            RegistrationMessage::parse([0xFE, 0xBF], &[0x01, 0x02]),
            Err(RegistrationParseError::TruncatedPayload)
        );
        assert_eq!(
            RegistrationMessage::parse([0xFE, 0xBF], &[0x02]),
            Err(RegistrationParseError::TruncatedPayload)
        );
        assert_eq!(
            RegistrationMessage::parse([0x05, 0xBF], &[]),
            Err(RegistrationParseError::TruncatedPayload)
        );
    }

    // --- Encode roundtrip tests ---

    #[test]
    fn test_encode_roundtrip() {
        let messages = vec![
            RegistrationMessage::NewClientQuery,
            RegistrationMessage::NewClientResponse {
                device_type: 0x02,
                client_hash: TEST_HASH,
            },
            RegistrationMessage::ClientIdAssignment {
                channel: 0x05,
                client_hash: TEST_HASH,
            },
            RegistrationMessage::ClientIdAck { channel: 0x05 },
            RegistrationMessage::ExistingClientRequest {
                channel: Some(0x05),
                client_hash: Some(TEST_HASH),
            },
            RegistrationMessage::ExistingClientResponse {
                channel: 0x05,
                client_hash: TEST_HASH,
            },
            RegistrationMessage::ClearToSend { channel: 0x10 },
        ];

        for original in &messages {
            let (msg_type, payload) = original.clone().encode_parts().unwrap();
            let parsed = RegistrationMessage::parse(msg_type, &payload).unwrap();
            assert_eq!(&parsed, original, "roundtrip failed for {:?}", original);
        }
    }

    // --- State machine tests ---

    #[test]
    fn test_full_new_client_flow() {
        let mut sm = new_sm();
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);

        // Spa queries → client responds
        let action = sm.process(&RegistrationMessage::NewClientQuery);
        assert_eq!(action, RegistrationAction::SendNewClientResponse);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

        // Spa assigns channel 0x05 with our hash
        let action = sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x05,
            client_hash: TEST_HASH,
        });
        assert_eq!(
            action,
            RegistrationAction::SendClientIdAck { client_id: 0x05 }
        );
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x05));
    }

    #[test]
    fn test_full_new_client_flow_legacy_no_hash() {
        let mut sm = new_sm();

        sm.process(&RegistrationMessage::NewClientQuery);

        // Legacy assignment without hash (0000) — should still be accepted
        let action = sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x05,
            client_hash: [0x00, 0x00],
        });
        assert_eq!(
            action,
            RegistrationAction::SendClientIdAck { client_id: 0x05 }
        );
        assert!(sm.is_registered());
    }

    #[test]
    fn test_hash_mismatch_rejected() {
        let mut sm = new_sm();
        sm.process(&RegistrationMessage::NewClientQuery);

        // Assignment with wrong hash — must be ignored
        let action = sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x05,
            client_hash: [0xAA, 0xBB],
        });
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);
        assert!(!sm.is_registered());
    }

    #[test]
    fn test_existing_client_reconnection() {
        let mut sm = new_sm_with_previous(0x05);
        assert_eq!(sm.previous_channel(), Some(0x05));

        // Spa sends query → SM tries existing client path
        let action = sm.process(&RegistrationMessage::NewClientQuery);
        assert_eq!(
            action,
            RegistrationAction::SendExistingClientRequest {
                message: RegistrationMessage::ExistingClientRequest {
                    channel: Some(0x05),
                    client_hash: Some(TEST_HASH),
                },
            }
        );
        assert_eq!(sm.state(), &RegistrationState::WaitingForExistingResponse);

        // Spa confirms
        let action = sm.process(&RegistrationMessage::ExistingClientResponse {
            channel: 0x05,
            client_hash: TEST_HASH,
        });
        assert_eq!(action, RegistrationAction::None); // No ACK needed
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x05));
    }

    #[test]
    fn test_existing_client_fallback_to_new() {
        let mut sm = new_sm_with_previous(0x05);

        // Spa sends query → SM tries existing client path
        sm.process(&RegistrationMessage::NewClientQuery);
        assert_eq!(sm.state(), &RegistrationState::WaitingForExistingResponse);

        // Spa doesn't recognize us, sends another query → fallback
        let action = sm.process(&RegistrationMessage::NewClientQuery);
        assert_eq!(action, RegistrationAction::SendNewClientResponse);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);

        // Normal assignment completes the flow
        let action = sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x06,
            client_hash: TEST_HASH,
        });
        assert_eq!(
            action,
            RegistrationAction::SendClientIdAck { client_id: 0x06 }
        );
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x06));
    }

    #[test]
    fn test_existing_client_hash_mismatch_rejected() {
        let mut sm = new_sm_with_previous(0x05);

        sm.process(&RegistrationMessage::NewClientQuery);
        assert_eq!(sm.state(), &RegistrationState::WaitingForExistingResponse);

        // Response with wrong hash — must be ignored
        let action = sm.process(&RegistrationMessage::ExistingClientResponse {
            channel: 0x05,
            client_hash: [0xAA, 0xBB],
        });
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForExistingResponse);
        assert!(!sm.is_registered());

        // Correct hash should still work
        let action = sm.process(&RegistrationMessage::ExistingClientResponse {
            channel: 0x05,
            client_hash: TEST_HASH,
        });
        assert_eq!(action, RegistrationAction::None);
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x05));
    }

    #[test]
    fn test_reset() {
        let mut sm = new_sm();
        sm.process(&RegistrationMessage::NewClientQuery);
        sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x05,
            client_hash: TEST_HASH,
        });
        assert!(sm.is_registered());

        sm.reset();
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
        assert!(!sm.is_registered());
    }

    #[test]
    fn test_assignment_in_waiting_for_query_ignored() {
        let mut sm = new_sm();
        let action = sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x05,
            client_hash: TEST_HASH,
        });
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForQuery);
    }

    #[test]
    fn test_query_while_registered_ignored() {
        let mut sm = new_sm();
        sm.process(&RegistrationMessage::NewClientQuery);
        sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x03,
            client_hash: TEST_HASH,
        });
        assert!(sm.is_registered());

        let action = sm.process(&RegistrationMessage::NewClientQuery);
        assert_eq!(action, RegistrationAction::None);
        assert!(sm.is_registered());
        assert_eq!(sm.client_id(), Some(0x03));
    }

    #[test]
    fn test_registered_ignores_all_messages() {
        let mut sm = new_sm();
        sm.process(&RegistrationMessage::NewClientQuery);
        sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x07,
            client_hash: TEST_HASH,
        });

        let action = sm.process(&RegistrationMessage::ClientIdAssignment {
            channel: 0x09,
            client_hash: TEST_HASH,
        });
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.client_id(), Some(0x07));
    }

    #[test]
    fn test_wrong_message_in_waiting_for_assignment_ignored() {
        let mut sm = new_sm();
        sm.process(&RegistrationMessage::NewClientQuery);

        let action = sm.process(&RegistrationMessage::ClearToSend { channel: 0x10 });
        assert_eq!(action, RegistrationAction::None);
        assert_eq!(sm.state(), &RegistrationState::WaitingForAssignment);
    }

    // --- process_raw convenience tests ---

    #[test]
    fn test_process_raw_new_client_flow() {
        let mut sm = new_sm();

        let action = sm.process_raw([0xFE, 0xBF], &[0x00]);
        assert_eq!(action, Some(RegistrationAction::SendNewClientResponse));

        let action = sm.process_raw([0xFE, 0xBF], &[0x02, 0x05, 0xF1, 0x73]);
        assert_eq!(
            action,
            Some(RegistrationAction::SendClientIdAck { client_id: 0x05 })
        );
        assert!(sm.is_registered());
    }

    #[test]
    fn test_process_raw_non_registration_returns_none() {
        let mut sm = new_sm();
        let action = sm.process_raw([0xFF, 0xAF], &[0x13]);
        assert_eq!(action, None);
    }

    #[test]
    fn test_process_raw_legacy_2byte_assignment() {
        let mut sm = new_sm();
        sm.process_raw([0xFE, 0xBF], &[0x00]);

        // Legacy: FE BF 02 <channel> (no hash bytes)
        let action = sm.process_raw([0xFE, 0xBF], &[0x02, 0x05]);
        assert_eq!(
            action,
            Some(RegistrationAction::SendClientIdAck { client_id: 0x05 })
        );
        assert!(sm.is_registered());
    }

    // --- Channel tests ---

    #[test]
    fn test_channel_from_u8() {
        assert_eq!(Channel::from(0x0A), Channel::WifiModule);
        assert_eq!(Channel::from(0x10), Channel::Client(0x10));
        assert_eq!(Channel::from(0x2F), Channel::Client(0x2F));
        assert_eq!(Channel::from(0x30), Channel::ClientNoCTS(0x30));
        assert_eq!(Channel::from(0x3F), Channel::ClientNoCTS(0x3F));
        assert_eq!(Channel::from(0xFE), Channel::MulticastChannelAssignment);
        assert_eq!(Channel::from(0xFF), Channel::MulticastBroadcast);
        assert_eq!(Channel::from(0x05), Channel::Unknown(0x05));
    }

    #[test]
    fn test_channel_into_u8() {
        assert_eq!(u8::from(Channel::WifiModule), 0x0A);
        assert_eq!(u8::from(Channel::Client(0x15)), 0x15);
        assert_eq!(u8::from(Channel::ClientNoCTS(0x33)), 0x33);
        assert_eq!(u8::from(Channel::MulticastChannelAssignment), 0xFE);
        assert_eq!(u8::from(Channel::MulticastBroadcast), 0xFF);
        assert_eq!(u8::from(Channel::Unknown(0x05)), 0x05);
    }

    #[test]
    fn test_channel_has_cts() {
        assert!(Channel::Client(0x10).has_cts());
        assert!(Channel::Client(0x2F).has_cts());
        assert!(!Channel::ClientNoCTS(0x30).has_cts());
        assert!(!Channel::WifiModule.has_cts());
        assert!(!Channel::MulticastChannelAssignment.has_cts());
    }

    #[test]
    fn test_channel_is_client() {
        assert!(Channel::Client(0x10).is_client());
        assert!(Channel::ClientNoCTS(0x30).is_client());
        assert!(!Channel::WifiModule.is_client());
        assert!(!Channel::MulticastBroadcast.is_client());
    }

    #[test]
    fn test_new_client_channel() {
        assert_eq!(Channel::new_client_channel(0), Some(Channel::Client(0x10)));
        assert_eq!(Channel::new_client_channel(31), Some(Channel::Client(0x2F)));
        assert_eq!(Channel::new_client_channel(32), None); // 0x30 is outside CTS range
    }

    #[test]
    fn test_channel_display() {
        assert_eq!(format!("{}", Channel::WifiModule), "WiFi(0x0A)");
        assert_eq!(format!("{}", Channel::Client(0x15)), "Client(0x15)");
        assert_eq!(
            format!("{}", Channel::ClientNoCTS(0x33)),
            "ClientNoCTS(0x33)"
        );
    }

    #[test]
    fn test_channel_roundtrip() {
        for raw in [0x0A, 0x10, 0x15, 0x2F, 0x30, 0x3F, 0xFE, 0xFF, 0x05] {
            assert_eq!(u8::from(Channel::from(raw)), raw);
        }
    }
}
