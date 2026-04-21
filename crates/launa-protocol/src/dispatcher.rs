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
    Ready,
    NewClientQuery,
    ClientIdAssignment {
        id: u8,
    },
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

// ---------------------------------------------------------------------------
// Helper: construct Unknown variant
// ---------------------------------------------------------------------------

#[inline]
fn unknown_msg(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    IncomingMessage::Unknown {
        message_type: msg_type,
        payload: payload.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Per-message-type handlers
// ---------------------------------------------------------------------------

/// Handle status update frames (message type `FF AF`).
fn handle_status(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    match StatusUpdate::parse(payload) {
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
    if payload.is_empty() {
        return unknown_msg(msg_type, payload);
    }
    match payload[0] {
        0x00 => IncomingMessage::NewClientQuery,
        0x02 => {
            if payload.len() >= 2 {
                IncomingMessage::ClientIdAssignment { id: payload[1] }
            } else {
                unknown_msg(msg_type, payload)
            }
        }
        _ => unknown_msg(msg_type, payload),
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
                let filter_data = &payload[3..];
                match FilterCycles::parse(filter_data) {
                    Ok(fc) => IncomingMessage::FilterCyclesResponse(fc),
                    Err(_) => {
                        log::warn!(
                            "dispatch: failed to parse FilterCycles from [{:#04X}, {:#04X}] sub-type 0x22/0x01 with {} byte payload",
                            msg_type[0],
                            msg_type[1],
                            payload.len()
                        );
                        unknown_msg(msg_type, payload)
                    }
                }
            } else {
                unknown_msg(msg_type, payload)
            }
        }

        // Information response
        0x02 => {
            if payload.len() > 3 {
                let info_data = &payload[3..];
                match InformationResponse::parse(info_data) {
                    Ok(info) => IncomingMessage::InformationResponse(info),
                    Err(_) => {
                        log::warn!(
                            "dispatch: failed to parse InformationResponse from [{:#04X}, {:#04X}] sub-type 0x22/0x02 with {} byte payload",
                            msg_type[0],
                            msg_type[1],
                            payload.len()
                        );
                        unknown_msg(msg_type, payload)
                    }
                }
            } else {
                unknown_msg(msg_type, payload)
            }
        }

        // Fault log response
        0x20 => {
            if payload.len() > 3 {
                let fault_data = &payload[3..];
                match FaultLogEntry::parse(fault_data) {
                    Ok(fault) => IncomingMessage::FaultLogResponse(fault),
                    Err(_) => {
                        log::warn!(
                            "dispatch: failed to parse FaultLogEntry from [{:#04X}, {:#04X}] sub-type 0x22/0x20 with {} byte payload",
                            msg_type[0],
                            msg_type[1],
                            payload.len()
                        );
                        unknown_msg(msg_type, payload)
                    }
                }
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
    let filter_data = &payload[1..];
    match FilterCycles::parse(filter_data) {
        Ok(fc) => IncomingMessage::FilterCyclesResponse(fc),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse FilterCycles from [{:#04X}, {:#04X}] sub-type 0x23 with {} byte payload",
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
}

/// Handle `0x0A 0xBF` sub-type `0x24` — direct information response.
fn handle_information_direct(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    let info_data = &payload[1..];
    match InformationResponse::parse(info_data) {
        Ok(info) => IncomingMessage::InformationResponse(info),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse InformationResponse from [{:#04X}, {:#04X}] sub-type 0x24 with {} byte payload",
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
}

/// Handle `0x0A 0xBF` sub-type `0x28` — direct fault log response.
fn handle_fault_log_direct(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    let fault_data = &payload[1..];
    match FaultLogEntry::parse(fault_data) {
        Ok(fault) => IncomingMessage::FaultLogResponse(fault),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse FaultLogEntry from [{:#04X}, {:#04X}] sub-type 0x28 with {} byte payload",
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
}

/// Handle `0x0A 0xBF` sub-type `0x2E` — control configuration.
fn handle_control_configuration(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    match SpaConfig::parse(&payload[1..]) {
        Ok(config) => IncomingMessage::ControlConfiguration(config),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse SpaConfig from [{:#04X}, {:#04X}] sub-type 0x2E with {} byte payload",
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
}

/// Handle `0x0A 0xBF` sub-type `0x94` — configuration response.
fn handle_configuration_response(msg_type: [u8; 2], payload: &[u8]) -> IncomingMessage {
    match SpaConfig::parse(&payload[1..]) {
        Ok(config) => IncomingMessage::ConfigurationResponse(config),
        Err(_) => {
            log::warn!(
                "dispatch: failed to parse SpaConfig from [{:#04X}, {:#04X}] sub-type 0x94 with {} byte payload",
                msg_type[0],
                msg_type[1],
                payload.len()
            );
            unknown_msg(msg_type, payload)
        }
    }
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

// ---------------------------------------------------------------------------
// Main dispatcher — thin router
// ---------------------------------------------------------------------------

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

        // Ready indicator: any XX BF where XX is not a known message type.
        // Protocol: "10 BF 06" for unregistered clients, "<ID> BF 06" for registered.
        // The second byte 0xBF identifies these as client-addressed ready-to-send messages.
        // Known prefixes (0x0A, 0xFE, 0xFF) are already matched above; any remaining
        // XX BF combination is a ready-to-send indicator.
        [_, 0xBF] => IncomingMessage::Ready,

        // Any other message type
        _ => unknown_msg(msg_type, payload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Temperature;
    use std::string::String;

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

    #[test]
    fn test_dispatch_ready() {
        let frame = Frame {
            message_type: [0x10, 0xBF],
            payload: vec![0x06],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(msg, IncomingMessage::Ready);
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
        assert_eq!(msg, IncomingMessage::Ready);
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
                IncomingMessage::Ready,
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
        assert_eq!(msg, IncomingMessage::Ready);
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
        assert_eq!(msg, IncomingMessage::Ready);
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

        // 0xFE BF with 0x00 → NewClientQuery, not Ready
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x00],
        };
        assert_eq!(dispatch_frame(&frame), IncomingMessage::NewClientQuery);

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
        assert_eq!(msg, IncomingMessage::NewClientQuery);
    }

    #[test]
    fn test_dispatch_client_id_assignment() {
        let frame = Frame {
            message_type: [0xFE, 0xBF],
            payload: vec![0x02, 0x05],
        };

        let msg = dispatch_frame(&frame);
        assert_eq!(msg, IncomingMessage::ClientIdAssignment { id: 0x05 });
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

    #[test]
    fn test_dispatch_short_0x22_payload_too_short_for_data() {
        // Payload [0x22, 0x01]: sub-type and settings-type present, but
        // only 2 bytes total — no room for the 3-byte header + filter data.
        let frame = Frame {
            message_type: [0x0A, 0xBF],
            payload: vec![0x22, 0x01],
        };
        let msg = dispatch_frame(&frame);
        assert!(
            matches!(msg, IncomingMessage::Unknown { .. }),
            "Expected Unknown for [0x22, 0x01] too-short payload, got {:?}",
            msg
        );
    }
}
