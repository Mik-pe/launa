/// Message dispatcher that takes a parsed `Frame` and returns a typed `IncomingMessage`.
extern crate alloc;
use alloc::vec::Vec;

use crate::config::SpaConfig;
use crate::fault::FaultLogEntry;
use crate::filter::FilterCycles;
use crate::frame::Frame;
use crate::information::InformationResponse;
use crate::status::StatusUpdate;

/// Typed representation of an incoming Balboa protocol message.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum IncomingMessage {
    StatusUpdate(StatusUpdate),
    ConfigurationResponse(SpaConfig),
    InformationResponse(InformationResponse),
    FaultLogResponse(FaultLogEntry),
    FilterCyclesResponse(FilterCycles),
    ControlConfiguration(SpaConfig),
    /// Spa tells a specific client it can send — `<channel> BF 06`.
    /// The dispatcher passes this as `Ready { channel }` so consumers can
    /// filter by their own client ID.
    Ready {
        channel: u8,
    },
    /// A parsed registration protocol message (FE BF or <channel> BF).
    Registration(crate::registration::RegistrationMessage),
    /// Preferences response (0x0A 0xBF sub-type 0x26).
    /// Payload contains panel preferences data.
    PreferencesResponse {
        payload: Vec<u8>,
    },
    /// Setup Parameters response (0x0A 0xBF sub-type 0x25).
    /// Payload contains setup/tuning parameters.
    SetupParametersResponse {
        payload: Vec<u8>,
    },
    Unknown {
        message_type: [u8; 2],
        payload: Vec<u8>,
    },
}

// Helper: construct Unknown variant

#[inline]
fn unknown_msg(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    IncomingMessage::Unknown {
        message_type: msg_type,
        payload: payload.to_vec(),
    }
}

/// Try to parse `data` with `parse_fn`, mapping success to `ok_variant`
/// and logging a warning + returning Unknown on failure.
#[inline]
fn parse_or_unknown<T, F, E>(
    msg_type: [u8; 2],
    payload: &[u8],
    parse_fn: impl FnOnce(&[u8]) -> Result<T, E>,
    ok_variant: F,
    context: &str,
) -> IncomingMessage
where
    F: FnOnce(T) -> IncomingMessage,
{
    match parse_fn(payload) {
        Ok(value) => ok_variant(value),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse {} from [{:#04X}, {:#04X}] with {} byte payload",
                context,
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
}

// Per-message-type handlers

/// Handle status update frames (message type `FF AF`).
///
/// The Balboa protocol includes a sub-type byte (`0x13`) as the first byte
/// of the payload area. This must be stripped before passing the remaining
/// 24 bytes to `StatusUpdate::parse()`. Frames from the simulator omit this
/// byte (24-byte payload), so both lengths are accepted.
fn handle_status(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    // Real Balboa hardware sends 25 bytes: [0x13, <24 status bytes>].
    // The simulator sends 24 bytes without the 0x13 prefix.
    let status_data = if payload.len() == 25 && payload[0] == 0x13 {
        &payload[1..]
    } else {
        payload
    };
    match StatusUpdate::parse(status_data) {
        Ok(status) => IncomingMessage::StatusUpdate(status),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse StatusUpdate from [{:#04X}, {:#04X}] with {} byte payload",
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
}

/// Handle registration frames (message type `FE BF`).
fn handle_registration(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    match crate::registration::RegistrationMessage::parse(msg_type, payload) {
        Ok(msg) => IncomingMessage::Registration(msg),
        Err(_) => unknown_msg(msg_type, payload),
    }
}

/// Handle `0x0A 0xBF` settings sub-type `0x22` (nested second-byte dispatch).
fn handle_settings_0x22(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    if payload.len() < 2 {
        return unknown_msg(msg_type, payload);
    }
    match payload[1] {
        // Panel settings response — not a dedicated parser
        0x00 => unknown_msg(msg_type, payload),

        // Filter cycles response
        0x01 => {
            if payload.len() > 3 {
                parse_or_unknown(
                    msg_type,
                    payload,
                    |data| FilterCycles::parse(&data[3..]),
                    IncomingMessage::FilterCyclesResponse,
                    "FilterCycles (sub-type 0x22/0x01)",
                )
            } else {
                unknown_msg(msg_type, payload)
            }
        }

        // Information response
        0x02 => {
            if payload.len() > 3 {
                parse_or_unknown(
                    msg_type,
                    payload,
                    |data| InformationResponse::parse(&data[3..]),
                    IncomingMessage::InformationResponse,
                    "InformationResponse (sub-type 0x22/0x02)",
                )
            } else {
                unknown_msg(msg_type, payload)
            }
        }

        // Fault log response
        0x20 => {
            if payload.len() > 3 {
                parse_or_unknown(
                    msg_type,
                    payload,
                    |data| FaultLogEntry::parse(&data[3..]),
                    IncomingMessage::FaultLogResponse,
                    "FaultLogEntry (sub-type 0x22/0x20)",
                )
            } else {
                unknown_msg(msg_type, payload)
            }
        }

        // Other settings sub-types
        _ => unknown_msg(msg_type, payload),
    }
}

/// Handle `0x0A 0xBF` sub-type `0x23` — direct filter cycles response.
fn handle_filter_cycles_direct(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    parse_or_unknown(
        msg_type,
        payload,
        |data| FilterCycles::parse(&data[1..]),
        IncomingMessage::FilterCyclesResponse,
        "FilterCycles (sub-type 0x23)",
    )
}

/// Handle `0x0A 0xBF` sub-type `0x24` — direct information response.
fn handle_information_direct(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    parse_or_unknown(
        msg_type,
        payload,
        |data| InformationResponse::parse(&data[1..]),
        IncomingMessage::InformationResponse,
        "InformationResponse (sub-type 0x24)",
    )
}

/// Handle `0x0A 0xBF` sub-type `0x28` — direct fault log response.
fn handle_fault_log_direct(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    parse_or_unknown(
        msg_type,
        payload,
        |data| FaultLogEntry::parse(&data[1..]),
        IncomingMessage::FaultLogResponse,
        "FaultLogEntry (sub-type 0x28)",
    )
}

/// Handle `0x0A 0xBF` sub-type `0x2E` — control configuration.
fn handle_control_configuration(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    parse_or_unknown(
        msg_type,
        payload,
        |data| SpaConfig::parse(&data[1..]),
        IncomingMessage::ControlConfiguration,
        "SpaConfig (sub-type 0x2E)",
    )
}

/// Handle `0x0A 0xBF` sub-type `0x94` — configuration response.
fn handle_configuration_response(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    parse_or_unknown(
        msg_type,
        payload,
        |data| SpaConfig::parse(&data[1..]),
        IncomingMessage::ConfigurationResponse,
        "SpaConfig (sub-type 0x94)",
    )
}

/// Handle `0x0A 0xBF` frames — dispatch by first payload byte (sub-type).
fn handle_0abf(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    if payload.is_empty() {
        return unknown_msg(msg_type, payload);
    }

    match payload[0] {
        // 0x04 → Configuration Request (outgoing, shouldn't appear as incoming)
        0x04 => unknown_msg(msg_type, payload),

        // 0x07 → Nothing to send (ack)
        0x07 => unknown_msg(msg_type, payload),

        // 0x11 → Toggle item response
        0x11 => unknown_msg(msg_type, payload),

        // 0x20 → Set temperature response
        0x20 => unknown_msg(msg_type, payload),

        // 0x22 → Settings sub-type: look at second byte
        0x22 => handle_settings_0x22(msg_type, payload),

        // 0x23 → Filter cycles response (direct)
        0x23 => handle_filter_cycles_direct(msg_type, payload),

        // 0x24 → Information response (direct)
        0x24 => handle_information_direct(msg_type, payload),

        // 0x25 → Setup Parameters response
        0x25 => IncomingMessage::SetupParametersResponse {
            payload: payload[1..].to_vec(),
        },

        // 0x26 → Preferences response
        0x26 => IncomingMessage::PreferencesResponse {
            payload: payload[1..].to_vec(),
        },

        // 0x28 → Fault log response (direct)
        0x28 => handle_fault_log_direct(msg_type, payload),

        // 0x2E → Control configuration
        0x2E => handle_control_configuration(msg_type, payload),

        // 0x94 → Configuration response
        0x94 => handle_configuration_response(msg_type, payload),

        // Unknown 0A BF sub-type
        _ => unknown_msg(msg_type, payload),
    }
}

// Main dispatcher — thin router

/// Dispatch a parsed frame into a typed `IncomingMessage`.
pub fn dispatch_frame(frame: &Frame) -> IncomingMessage {
    let msg_type = frame.message_type;
    let payload = &frame.payload;

    match msg_type {
        // Status update: FF AF
        [0xFF, 0xAF] => handle_status(msg_type, payload),

        // Registration messages: FE BF
        [0xFE, 0xBF] => handle_registration(msg_type, payload),

        // 0A BF messages: disambiguate by first payload byte
        [0x0A, 0xBF] => handle_0abf(msg_type, payload),

        // XX BF messages: ready-to-send indicator for registered clients.
        // Protocol: "10 BF 06" for unregistered (display panel CTS),
        // "<ID> BF 06" for registered clients (unicast CTS).
        // The second byte 0xBF identifies these as client-addressed messages.
        // Known prefixes (0x0A, 0xFE, 0xFF) are already matched above.
        //
        // Note: ClearToSend (<ch> BF 06), ClientIdAck (<ch> BF 03), and
        // ExistingClientResponse (<ch> BF 05) are also <ch> BF frames, but
        // they are only relevant during the registration handshake and are
        // handled directly by SpaApp via RegistrationMessage::parse() on the
        // raw frame bytes — the dispatcher doesn't need to distinguish them.
        [channel, 0xBF] => IncomingMessage::Ready { channel },

        // Any other message type
        _ => unknown_msg(msg_type, payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{HeatingMode, PumpState, TempRange, TemperatureScale};
    use crate::Temperature;

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn test_dispatch_status_update() {
        let mut payload = vec![0u8; 24];
        payload[2] = 100; // temp
        payload[20] = 104; // set temp

        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::StatusUpdate(s) => {
                assert_eq!(s.current_temp, Some(Temperature::fahrenheit(100.0)));
                assert_eq!(s.set_temp, Temperature::fahrenheit(104.0));
            }
            _ => panic!("Expected StatusUpdate, got {:?}", msg),
        }
    }

    /// Real Balboa hardware includes a 0x13 sub-type prefix in the payload.
    /// The dispatcher must strip it before passing to StatusUpdate::parse().
    #[test]
    fn test_dispatch_status_update_with_0x13_prefix() {
        // Build a 24-byte status payload, then prepend 0x13 to simulate
        // what real Balboa hardware sends on the wire.
        let mut inner = [0u8; 24];
        inner[0] = 0x00; // running
        inner[2] = 100; // current temp = 100°F
        inner[9] = 0x02; // 24h time
        inner[10] = 0x34; // heating + temp range high
        inner[20] = 104; // set temp

        let mut payload = vec![0x13]; // sub-type prefix
        payload.extend_from_slice(&inner);

        assert_eq!(payload.len(), 25);

        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::StatusUpdate(s) => {
                assert_eq!(s.current_temp, Some(Temperature::fahrenheit(100.0)));
                assert_eq!(s.set_temp, Temperature::fahrenheit(104.0));
                assert_eq!(s.temp_range, TempRange::High);
                assert!(s.is_heating);
            }
            _ => panic!("Expected StatusUpdate, got {:?}", msg),
        }
    }

    /// Celsius status with 0x13 prefix — verifies temp scale + wire-value decoding.
    #[test]
    fn test_dispatch_status_celsius_with_0x13_prefix() {
        let mut inner = [0u8; 24];
        inner[0] = 0x00; // running
        inner[2] = 72; // 36°C (wire: 36*2 = 72)
        inner[9] = 0x01; // Celsius bit
        inner[10] = 0x04; // temp range high, no heating
        inner[20] = 80; // set temp = 40°C (wire: 40*2 = 80)

        let mut payload = vec![0x13];
        payload.extend_from_slice(&inner);

        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::StatusUpdate(s) => {
                assert_eq!(s.current_temp, Some(Temperature::celsius(36.0)));
                assert_eq!(s.set_temp, Temperature::celsius(40.0));
                assert_eq!(s.temperature_scale, TemperatureScale::Celsius);
                assert_eq!(s.temp_range, TempRange::High);
            }
            _ => panic!("Expected StatusUpdate, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_ready() {
        let frame = Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(msg, IncomingMessage::Ready { channel: 0x10 });
    }

    /// VAL-PROTO-006: Registered client Ready frame (<ID> BF 06) dispatches as Ready.
    /// After registration, the spa sends ready-to-send as <ID> BF 06 where <ID>
    /// is the assigned client ID (not 0x10).
    #[test]
    fn test_dispatch_ready_registered_client_id_0x02() {
        let frame = Frame {
            message_type: [0x02, 0xBF],
            payload: vec![0x06],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(msg, IncomingMessage::Ready { channel: 0x02 });
    }

    /// VAL-PROTO-006: Various registered client IDs all dispatch as Ready.
    /// Client IDs in the Balboa protocol are typically 0x01-0x09. IDs 0x0A (config),
    /// 0xFE (registration), and 0xFF (status) are reserved message types.
    #[test]
    fn test_dispatch_ready_registered_client_various_ids() {
        for id in [0x01, 0x02, 0x03, 0x05, 0x09, 0x10, 0x20, 0x7F] {
            let frame = Frame {
                message_type: [id, 0xBF],
                payload: vec![0x06],
            };

            let msg = dispatch_frame(&frame);
            assert_eq!(
                msg,
                IncomingMessage::Ready { channel: id },
                "client ID 0x{:02X} should dispatch as Ready",
                id
            );
        }
    }

    /// VAL-PROTO-006: <ID> BF with empty payload dispatches as Ready.
    /// Some implementations may send Ready without a payload byte.
    #[test]
    fn test_dispatch_ready_registered_empty_payload() {
        let frame = Frame {
            message_type: [0x02, 0xBF],
            payload: vec![],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(msg, IncomingMessage::Ready { channel: 0x02 });
    }

    /// VAL-PROTO-006: <ID> BF with non-0x06 payload still dispatches as Ready.
    /// The dispatcher should not gate on the payload byte value for ready-to-send.
    #[test]
    fn test_dispatch_ready_registered_non_06_payload() {
        let frame = Frame {
            message_type: [0x02, 0xBF],
            payload: vec![0x01],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(msg, IncomingMessage::Ready { channel: 0x02 });
    }

    /// VAL-PROTO-006: Reserved message types 0x0A, 0xFE, 0xFF with 0xBF second byte
    /// are NOT dispatched as Ready — they are handled by their specific match arms.
    #[test]
    fn test_dispatch_reserved_types_not_ready() {
        // 0x0A BF with unknown sub-type → Unknown, not Ready
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0xFF], // unknown sub-type
        };
        match dispatch_frame(&frame) {
            IncomingMessage::Unknown { .. } => {}
            other => panic!(
                "Expected Unknown for 0x0A BF with unknown sub-type, got {:?}",
                other
            ),
        }

        // 0xFE BF with 0x00 → Registration(NewClientQuery), not Ready
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x00],
        };
        assert_eq!(
            dispatch_frame(&frame),
            IncomingMessage::Registration(crate::registration::RegistrationMessage::NewClientQuery)
        );

        // 0xFF AF → StatusUpdate (not BF, but verify it's not confused)
        let mut payload = vec![0u8; 24];
        payload[2] = 100;
        payload[20] = 104;
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload,
        };
        match dispatch_frame(&frame) {
            IncomingMessage::StatusUpdate(_) => {}
            other => panic!("Expected StatusUpdate, got {:?}", other),
        }
    }

    #[test]
    fn test_dispatch_new_client_query() {
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x00],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(
            msg,
            IncomingMessage::Registration(crate::registration::RegistrationMessage::NewClientQuery)
        );
    }

    #[test]
    fn test_dispatch_client_id_assignment() {
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02, 0x05],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(
            msg,
            IncomingMessage::Registration(
                crate::registration::RegistrationMessage::ClientIdAssignment {
                    channel: 0x05,
                    client_hash: [0x00, 0x00], // legacy 2-byte form
                }
            )
        );
    }

    #[test]
    fn test_dispatch_client_id_assignment_with_hash() {
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02, 0x05, 0xF1, 0x73],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(
            msg,
            IncomingMessage::Registration(
                crate::registration::RegistrationMessage::ClientIdAssignment {
                    channel: 0x05,
                    client_hash: [0xF1, 0x73],
                }
            )
        );
    }

    #[test]
    fn test_dispatch_client_id_assignment_missing_id() {
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02], // no second byte
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::Unknown {
                message_type,
                payload,
            } => {
                assert_eq!(message_type, [0xFE, 0xBF]);
                assert_eq!(payload, vec![0x02]);
            }
            _ => panic!("Expected Unknown for missing client ID byte, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_information_response() {
        // 0x24 sub-type, followed by 21 bytes of info data
        let mut payload = vec![0x24];
        payload.extend_from_slice(&[
            0x64, 0xDC, 0x11, 0x00, 0x42, 0x46, 0x42, 0x50, 0x32, 0x30, 0x20, 0x20, 0x01, 0x3D,
            0x12, 0x38, 0x2E, 0x01, 0x0A, 0x04, 0x00,
        ]);

        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::InformationResponse(info) => {
                assert_eq!(info.system_model, "BFBP20");
                assert_eq!(info.config_signature, "3D12382E");
            }
            _ => panic!("Expected InformationResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_fault_log_response() {
        // 0x28 sub-type, followed by 10 bytes of fault data
        let payload = vec![
            0x28, 0x03, 0x01, 0x1B, 0x02, 0x0E, 0x1E, 0x04, 0x68, 0x68, 0x66,
        ];

        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::FaultLogResponse(entry) => {
                assert_eq!(entry.fault_count, 3);
                assert_eq!(entry.message_code, crate::fault::FaultCode::HeaterDry);
            }
            _ => panic!("Expected FaultLogResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_filter_cycles_response() {
        // 0x23 sub-type, followed by 8 bytes of filter data
        let payload = vec![0x23, 0x08, 0x00, 0x04, 0x00, 0x90, 0x00, 0x02, 0x00];

        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::FilterCyclesResponse(fc) => {
                assert_eq!(fc.filter1.start_hour, 8);
                assert_eq!(fc.filter2.start_hour, 16);
                assert!(fc.filter2.enabled);
            }
            _ => panic!("Expected FilterCyclesResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_configuration_response() {
        // 0x94 sub-type, followed by config data (need at least 10 bytes)
        let mut config_data = vec![0x02, 0x02, 0x80, 0x00, 0x15, 0x27, 0x10, 0xAB, 0xD2, 0x00];
        let mut payload = vec![0x94];
        payload.append(&mut config_data);

        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::ConfigurationResponse(_) => {}
            _ => panic!("Expected ConfigurationResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_control_configuration() {
        // 0x2E sub-type, followed by config data
        let mut config_data = vec![0x02, 0x02, 0x80, 0x00, 0x15, 0x27, 0x10, 0xAB, 0xD2, 0x00];
        let mut payload = vec![0x2E];
        payload.append(&mut config_data);

        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::ControlConfiguration(_) => {}
            _ => panic!("Expected ControlConfiguration, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_unknown_message_type() {
        let frame = Frame {
            message_type: [0xAB, 0xCD],
            payload: vec![0x01, 0x02, 0x03],
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::Unknown {
                message_type,
                payload,
            } => {
                assert_eq!(message_type, [0xAB, 0xCD]);
                assert_eq!(payload, vec![0x01, 0x02, 0x03]);
            }
            _ => panic!("Expected Unknown, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_unknown_0abf_subtype() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0xFF], // unknown sub-type
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::Unknown { .. } => {}
            _ => panic!("Expected Unknown, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_empty_0abf() {
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![],
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::Unknown { .. } => {}
            _ => panic!("Expected Unknown for empty 0A BF payload, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_preferences_response() {
        // 0x26 sub-type: Preferences response with arbitrary payload
        let payload = vec![0x26, 0x01, 0x02, 0x03, 0x04];
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::PreferencesResponse { payload: data } => {
                assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04]);
            }
            _ => panic!("Expected PreferencesResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_setup_parameters_response() {
        // 0x25 sub-type: Setup Parameters response with arbitrary payload
        let payload = vec![0x25, 0xAA, 0xBB, 0xCC];
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::SetupParametersResponse { payload: data } => {
                assert_eq!(data, vec![0xAA, 0xBB, 0xCC]);
            }
            _ => panic!("Expected SetupParametersResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_preferences_empty_payload() {
        // 0x26 with just the sub-type byte
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x26],
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::PreferencesResponse { payload: data } => {
                assert!(data.is_empty());
            }
            _ => panic!("Expected PreferencesResponse, got {:?}", msg),
        }
    }

    #[test]
    fn test_dispatch_setup_parameters_empty_payload() {
        // 0x25 with just the sub-type byte
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x25],
        };

        let msg = dispatch_frame(&frame);
        match msg {
            IncomingMessage::SetupParametersResponse { payload: data } => {
                assert!(data.is_empty());
            }
            _ => panic!("Expected SetupParametersResponse, got {:?}", msg),
        }
    }

    /// A thread-local buffer for capturing log::warn! output in tests.
    /// This avoids the one-time-only limitation of `log::set_logger`.
    use std::cell::RefCell;

    thread_local! {
        static WARN_BUFFER: RefCell<Vec<String>> = RefCell::new(Vec::new());
    }

    struct CaptureLogger;

    impl log::Log for CaptureLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Warn
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                WARN_BUFFER.with(|buf| {
                    buf.borrow_mut().push(format!("{}", record.args()));
                });
            }
        }
        fn flush(&self) {}
    }

    /// Install the capture logger (once). Safe to call multiple times.
    fn install_capture_logger() {
        static SET: std::sync::Once = std::sync::Once::new();
        SET.call_once(|| {
            // SAFETY: CaptureLogger is zero-sized and thread-safe (uses thread_local storage).
            let logger: &'static CaptureLogger = &CaptureLogger;
            log::set_logger(logger).unwrap();
            log::set_max_level(log::LevelFilter::Warn);
        });
    }

    /// Run a closure with warn capture active, returning captured warning messages.
    fn with_warn_capture<F: FnOnce()>(f: F) -> Vec<String> {
        install_capture_logger();
        // Clear the buffer before running
        WARN_BUFFER.with(|buf| buf.borrow_mut().clear());
        f();
        // Collect whatever was logged
        WARN_BUFFER.with(|buf| buf.borrow().clone())
    }

    #[test]
    fn test_dispatch_warns_on_status_parse_failure() {
        // FF AF with a payload too short for StatusUpdate parsing
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload: vec![0x01], // way too short for a valid status
        };

        let warnings = with_warn_capture(|| {
            let msg = dispatch_frame(&frame);
            assert!(
                matches!(msg, IncomingMessage::Unknown { .. }),
                "Expected Unknown, got {:?}",
                msg
            );
        });

        assert!(
            !warnings.is_empty(),
            "Expected at least one log::warn! for failed StatusUpdate parse"
        );
        let warning = &warnings[0];
        assert!(
            warning.contains("0xFF"),
            "Warning should contain message type byte 0xFF: got '{}'",
            warning
        );
        assert!(
            warning.contains("0xAF"),
            "Warning should contain message type byte 0xAF: got '{}'",
            warning
        );
        assert!(
            warning.contains("1 byte"),
            "Warning should contain payload length: got '{}'",
            warning
        );
    }

    #[test]
    fn test_dispatch_warns_on_config_parse_failure() {
        // 0x94 sub-type with invalid short config data
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x94, 0x01], // too short for SpaConfig
        };

        let warnings = with_warn_capture(|| {
            let msg = dispatch_frame(&frame);
            assert!(
                matches!(msg, IncomingMessage::Unknown { .. }),
                "Expected Unknown, got {:?}",
                msg
            );
        });

        assert!(
            !warnings.is_empty(),
            "Expected at least one log::warn! for failed SpaConfig parse"
        );
        assert!(
            warnings[0].contains("0x0A"),
            "Warning should contain message type byte 0x0A: got '{}'",
            warnings[0]
        );
        assert!(
            warnings[0].contains("0xBF"),
            "Warning should contain message type byte 0xBF: got '{}'",
            warnings[0]
        );
    }

    #[test]
    fn test_dispatch_escape_bytes_in_message_type() {
        // Message types containing 0x7E or 0x7D (HDLC escape bytes) should
        // dispatch as Unknown without panicking. Use second byte != 0xBF
        // to avoid the Ready catch-all arm.
        for mt in [0x7E, 0x7D] {
            let frame = Frame {
                message_type: [mt, 0xCD],
                payload: vec![0x01, 0x02],
            };
            let msg = dispatch_frame(&frame);
            match msg {
                IncomingMessage::Unknown {
                    message_type,
                    payload,
                } => {
                    assert_eq!(message_type, [mt, 0xCD]);
                    assert_eq!(payload, vec![0x01, 0x02]);
                }
                _ => panic!(
                    "Expected Unknown for message_type [0x{:02X}, 0xCD], got {:?}",
                    mt, msg
                ),
            }
        }
    }

    #[test]
    fn test_dispatch_idempotency() {
        // Dispatching the same frame twice must produce identical results.
        let mut payload = vec![0u8; 24];
        payload[2] = 100;
        payload[20] = 104;
        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload,
        };

        let result1 = dispatch_frame(&frame);
        let result2 = dispatch_frame(&frame);
        assert_eq!(
            result1, result2,
            "Dispatching the same frame twice must produce equal results"
        );
    }

    #[test]
    fn test_dispatch_short_0x22_payload_alone() {
        // Payload [0x22] alone: sub-type byte present but no second byte
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x22],
        };
        let msg = dispatch_frame(&frame);
        assert!(
            matches!(msg, IncomingMessage::Unknown { .. }),
            "Expected Unknown for [0x22] alone, got {:?}",
            msg
        );
    }

    /// Regression test: real sniffer capture from BP6013G1 in Celsius mode.
    /// Raw wire payload is 25 bytes (0x13 prefix + 24 status bytes).
    /// Before the fix, the 0x13 prefix shifted all offsets by 1, causing:
    ///   - current_temp = 3 (init_mode byte 0x03) instead of 35°C
    ///   - temp_range = Low (wrong flags byte) instead of High
    ///   - temp_scale = Fahrenheit (wrong flags byte) instead of Celsius
    #[test]
    fn test_real_sniffer_celsius_status_frame() {
        let sniffer_hex = "130003460c0c00280306031c00000200000000020248000000";
        let payload = hex_decode(sniffer_hex);
        assert_eq!(payload.len(), 25);
        assert_eq!(payload[0], 0x13);

        let frame = Frame {
            message_type: [0xFF, 0xAF],
            payload,
        };

        let msg = dispatch_frame(&frame);
        let status = match msg {
            IncomingMessage::StatusUpdate(s) => s,
            other => panic!("Expected StatusUpdate, got {:?}", other),
        };

        // Temperature: payload[3]=0x46=70, Celsius → 70/2 = 35°C
        assert_eq!(status.current_temp, Some(Temperature::celsius(35.0)));
        // Set temp: payload[21]=0x48=72, Celsius → 72/2 = 36°C
        assert_eq!(status.set_temp, Temperature::celsius(36.0));
        // Scale: payload[9]=0x03, bit 0=1 → Celsius
        assert_eq!(status.temperature_scale, TemperatureScale::Celsius);
        // Temp range: payload[10]=0x1C=28, bit 2=1 → High
        assert_eq!(status.temp_range, TempRange::High);
        // Heating: payload[10]=0x1C, bits 4-5=0x10 → heating active
        assert!(status.is_heating);
        // All pumps off: payload[11]=0x1C... wait, let me re-check
        // payload[11]=0x00 → pumps 1-4 all off
        // Actually payload[11] in the raw is index 11 of the full 25-byte payload
        // After stripping 0x13, status data[11] = 0x00 → all pumps 1-4 off
        for (i, pump) in status.pumps.iter().enumerate() {
            assert_eq!(*pump, PumpState::Off, "pump {} should be off", i + 1);
        }
        // Blower off
        assert!(!status.blower);
        // Mister off
        assert!(!status.mister);
        // Lights off: status data[14]=0x00
        for (i, &on) in status.lights.iter().enumerate() {
            assert!(!on, "light {} should be off", i + 1);
        }
        // Heating mode: payload[5]=0x00 → Ready
        assert_eq!(status.heating_mode, HeatingMode::Ready);
        // Hour: payload[3]=0x0C=12, Minute: payload[4]=0x0C=12
        assert_eq!(status.hour, 12);
        assert_eq!(status.minute, 12);
        // Circ pump: payload[13]=0x02, bit 1=1 → on
        assert!(status.circ_pump);
        // Init mode: payload[1]=0x03 → Reminder
        // (SpaApp doesn't expose init_mode publicly, but is_priming should be false)
        assert!(!status.is_priming);
        // 24h time: payload[9]=0x03, bit 1=1
        assert_eq!(status.time_format, crate::status::TimeFormat::Hour24);
    }

    /// Same sniffer data, but verify it fails with the OLD behavior (no 0x13 stripping).
    /// This proves the bug: if we pass the raw 25 bytes directly to StatusUpdate::parse,
    /// we get the wrong values.
    #[test]
    fn test_sniffer_data_proves_the_offset_bug() {
        let sniffer_hex = "130003460c0c00280306031c00000200000000020248000000";
        let payload = hex_decode(sniffer_hex);

        // Without the 0x13 strip, parse reads payload[2]=0x03 as current_temp
        let buggy = StatusUpdate::parse(&payload).unwrap();
        // Bug: init_mode byte 0x03 parsed as temperature in Fahrenheit = 3°F
        assert_eq!(buggy.current_temp, Some(Temperature::fahrenheit(3.0)));
        // Bug: wrong flags byte → Fahrenheit instead of Celsius
        assert_eq!(buggy.temperature_scale, TemperatureScale::Fahrenheit);
        // Bug: wrong flags byte → Low instead of High
        assert_eq!(buggy.temp_range, TempRange::Low);
    }
}
