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
    Unknown {
        message_type: [u8; 2],
        payload: Vec<u8>,
    },
}

/// Dispatch a parsed frame into a typed `IncomingMessage`.
pub fn dispatch_frame(frame: &Frame) -> IncomingMessage {
    match frame.message_type {
        // Status update: FF AF
        [0xFF, 0xAF] => match StatusUpdate::parse(&frame.payload) {
            Ok(status) => IncomingMessage::StatusUpdate(status),
            Err(_) => IncomingMessage::Unknown {
                message_type: frame.message_type,
                payload: frame.payload.clone(),
            },
        },

        // Ready indicator: 10 BF
        [0x10, 0xBF] => IncomingMessage::Ready,

        // Registration messages: FE BF
        [0xFE, 0xBF] => {
            if frame.payload.is_empty() {
                return IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                };
            }
            match frame.payload[0] {
                0x00 => IncomingMessage::NewClientQuery,
                0x02 => {
                    let id = frame.payload.get(1).copied().unwrap_or(0);
                    IncomingMessage::ClientIdAssignment { id }
                }
                _ => IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                },
            }
        }

        // 0A BF messages: disambiguate by first payload byte
        [0x0A, 0xBF] => {
            if frame.payload.is_empty() {
                return IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                };
            }

            match frame.payload[0] {
                // 0x04 → Configuration Request (outgoing, shouldn't appear as incoming)
                0x04 => IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                },

                // 0x07 → Nothing to send (ack)
                0x07 => IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                },

                // 0x11 → Toggle item response
                0x11 => IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                },

                // 0x20 → Set temperature response
                0x20 => IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                },

                // 0x22 → Settings sub-type: look at second byte
                0x22 => {
                    if frame.payload.len() < 2 {
                        return IncomingMessage::Unknown {
                            message_type: frame.message_type,
                            payload: frame.payload.clone(),
                        };
                    }
                    match frame.payload[1] {
                        // Panel settings response — not a dedicated parser
                        0x00 => IncomingMessage::Unknown {
                            message_type: frame.message_type,
                            payload: frame.payload.clone(),
                        },
                        // Filter cycles response
                        0x01 => {
                            // The actual filter data starts after the 2 sub-type bytes
                            // and 1 more byte (total 3-byte header for settings response)
                            if frame.payload.len() > 3 {
                                let filter_data = &frame.payload[3..];
                                match FilterCycles::parse(filter_data) {
                                    Ok(fc) => IncomingMessage::FilterCyclesResponse(fc),
                                    Err(_) => IncomingMessage::Unknown {
                                        message_type: frame.message_type,
                                        payload: frame.payload.clone(),
                                    },
                                }
                            } else {
                                IncomingMessage::Unknown {
                                    message_type: frame.message_type,
                                    payload: frame.payload.clone(),
                                }
                            }
                        }
                        // Information response
                        0x02 => {
                            if frame.payload.len() > 3 {
                                let info_data = &frame.payload[3..];
                                match InformationResponse::parse(info_data) {
                                    Ok(info) => IncomingMessage::InformationResponse(info),
                                    Err(_) => IncomingMessage::Unknown {
                                        message_type: frame.message_type,
                                        payload: frame.payload.clone(),
                                    },
                                }
                            } else {
                                IncomingMessage::Unknown {
                                    message_type: frame.message_type,
                                    payload: frame.payload.clone(),
                                }
                            }
                        }
                        // Fault log response
                        0x20 => {
                            if frame.payload.len() > 3 {
                                let fault_data = &frame.payload[3..];
                                match FaultLogEntry::parse(fault_data) {
                                    Ok(fault) => IncomingMessage::FaultLogResponse(fault),
                                    Err(_) => IncomingMessage::Unknown {
                                        message_type: frame.message_type,
                                        payload: frame.payload.clone(),
                                    },
                                }
                            } else {
                                IncomingMessage::Unknown {
                                    message_type: frame.message_type,
                                    payload: frame.payload.clone(),
                                }
                            }
                        }
                        // Preferences or other
                        _ => IncomingMessage::Unknown {
                            message_type: frame.message_type,
                            payload: frame.payload.clone(),
                        },
                    }
                }

                // 0x23 → Filter cycles response (direct)
                0x23 => {
                    // Data starts after the sub-type byte
                    let filter_data = &frame.payload[1..];
                    match FilterCycles::parse(filter_data) {
                        Ok(fc) => IncomingMessage::FilterCyclesResponse(fc),
                        Err(_) => IncomingMessage::Unknown {
                            message_type: frame.message_type,
                            payload: frame.payload.clone(),
                        },
                    }
                }

                // 0x24 → Information response (direct)
                0x24 => {
                    let info_data = &frame.payload[1..];
                    match InformationResponse::parse(info_data) {
                        Ok(info) => IncomingMessage::InformationResponse(info),
                        Err(_) => IncomingMessage::Unknown {
                            message_type: frame.message_type,
                            payload: frame.payload.clone(),
                        },
                    }
                }

                // 0x28 → Fault log response (direct)
                0x28 => {
                    let fault_data = &frame.payload[1..];
                    match FaultLogEntry::parse(fault_data) {
                        Ok(fault) => IncomingMessage::FaultLogResponse(fault),
                        Err(_) => IncomingMessage::Unknown {
                            message_type: frame.message_type,
                            payload: frame.payload.clone(),
                        },
                    }
                }

                // 0x2E → Control configuration
                0x2E => match SpaConfig::parse(&frame.payload[1..]) {
                    Ok(config) => IncomingMessage::ControlConfiguration(config),
                    Err(_) => IncomingMessage::Unknown {
                        message_type: frame.message_type,
                        payload: frame.payload.clone(),
                    },
                },

                // 0x94 → Configuration response
                0x94 => match SpaConfig::parse(&frame.payload[1..]) {
                    Ok(config) => IncomingMessage::ConfigurationResponse(config),
                    Err(_) => IncomingMessage::Unknown {
                        message_type: frame.message_type,
                        payload: frame.payload.clone(),
                    },
                },

                // Unknown 0A BF sub-type
                _ => IncomingMessage::Unknown {
                    message_type: frame.message_type,
                    payload: frame.payload.clone(),
                },
            }
        }

        // Any other message type
        _ => IncomingMessage::Unknown {
            message_type: frame.message_type,
            payload: frame.payload.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                assert_eq!(s.current_temp, Some(100.0));
                assert_eq!(s.set_temp, 104.0);
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
}
