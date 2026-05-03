//! MQTT 3.1.1 protocol codec — pure encode/decode functions.
//!
//! Provides packet-level encoding and decoding for MQTT 3.1.1. All functions
//! are pure protocol logic with no TCP/socket/ESP32 dependencies, making
//! them fully testable on desktop.

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::packet::decode_remaining_length;

/// Encode an MQTT v5 remaining-length field and append to `buf`.
///
/// MQTT uses a variable-length encoding: each byte stores 7 bits of the
/// value plus a continuation bit (bit 7).
pub fn encode_remaining_length(buf: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut byte = (len & 0x7F) as u8;
        len >>= 7;
        if len > 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if len == 0 {
            break;
        }
    }
}

/// Append an MQTT Length-Prefixed (LP) UTF-8 string to `buf`.
///
/// Format: 2-byte big-endian length + UTF-8 bytes.
pub fn append_lp_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buf.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// CONNECT packet configuration.
pub struct ConnectConfig<'a> {
    pub client_id: &'a str,
    pub lwt_topic: &'a str,
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    pub keep_alive: u16,
}

/// Encode an MQTT v5 CONNECT packet.
///
/// Returns the complete packet bytes (fixed header + variable header + payload).
/// The packet uses Clean Start, Will Flag with QoS 1 + Retain, and optional
/// username/password.
pub fn encode_connect(config: &ConnectConfig<'_>) -> Vec<u8> {
    let mut connect_flags: u8 = 0x02 // Clean Session
        | (1 << 2) // Will Flag
        | (1 << 3) // Will QoS 1 (bit 3)
        | (1 << 5); // Will Retain

    if config.username.is_some() {
        connect_flags |= 1 << 7;
    }
    if config.password.is_some() {
        connect_flags |= 1 << 6;
    }

    // Variable header
    let mut var_header = Vec::new();
    var_header.extend_from_slice(&[0x00, 0x04]);
    var_header.extend_from_slice(b"MQTT");
    var_header.push(0x04); // Protocol level 4 (MQTT 3.1.1)
    var_header.push(connect_flags);
    var_header.extend_from_slice(&config.keep_alive.to_be_bytes());

    // Payload (no v5 properties — MQTT 3.1.1 format)
    let mut payload = Vec::new();
    append_lp_string(&mut payload, config.client_id);
    append_lp_string(&mut payload, config.lwt_topic);
    let will_payload = b"offline";
    payload.extend_from_slice(&(will_payload.len() as u16).to_be_bytes());
    payload.extend_from_slice(will_payload);
    if let Some(user) = config.username {
        append_lp_string(&mut payload, user);
    }
    if let Some(pass) = config.password {
        append_lp_string(&mut payload, pass);
    }

    // Fixed header + variable header + payload
    let remaining_len = var_header.len() + payload.len();
    let mut packet = Vec::new();
    packet.push(0x10); // CONNECT packet type
    encode_remaining_length(&mut packet, remaining_len);
    packet.extend_from_slice(&var_header);
    packet.extend_from_slice(&payload);

    packet
}

/// Parse a CONNACK packet from raw bytes.
///
/// Returns `Ok(())` if the packet is a valid CONNACK with a success code.
/// Returns `Err(ConnackError)` with details on failure.
#[derive(Debug, PartialEq, Eq)]
pub enum ConnackError {
    /// Buffer too short to contain a valid CONNACK.
    TooShort,
    /// First byte is not 0x20 (CONNACK type).
    WrongType(u8),
    /// Connect reason code indicates failure.
    ReasonCode(u8),
}

pub fn parse_connack(buf: &[u8]) -> Result<(), ConnackError> {
    if buf.len() < 4 {
        return Err(ConnackError::TooShort);
    }
    if buf[0] != 0x20 {
        return Err(ConnackError::WrongType(buf[0]));
    }
    // buf[1] = remaining length (typically 0x02)
    // buf[2] = connect acknowledge flags
    // buf[3] = connect reason code (0x00 = success)
    let reason_code = buf[3];
    if reason_code != 0x00 {
        return Err(ConnackError::ReasonCode(reason_code));
    }
    Ok(())
}

/// Error returned by [`encode_publish`] for invalid inputs.
#[derive(Debug, PartialEq, Eq)]
pub enum PublishError {
    /// QoS must be 0 or 1.
    InvalidQoS(u8),
    /// QoS > 0 requires a packet identifier, but `None` was provided.
    MissingPacketId,
}

/// Encode an MQTT 3.1.1 PUBLISH packet.
///
/// `qos` must be 0 or 1. For QoS > 0, `packet_id` must be `Some`.
/// Returns the complete packet bytes ready to send.
pub fn encode_publish(
    topic: &str,
    payload: &[u8],
    qos: u8,
    retain: bool,
    packet_id: Option<u16>,
) -> Result<Vec<u8>, PublishError> {
    if qos > 1 {
        return Err(PublishError::InvalidQoS(qos));
    }
    if qos > 0 && packet_id.is_none() {
        return Err(PublishError::MissingPacketId);
    }

    let retain_flag = if retain { 0x01 } else { 0x00 };
    let qos_flag = (qos & 0x03) << 1;
    let mut packet = Vec::new();
    packet.push(0x30 | qos_flag | retain_flag);

    let topic_bytes = topic.as_bytes();
    let mut remaining = 2 + topic_bytes.len() + payload.len();
    if qos > 0 {
        remaining += 2;
    }
    encode_remaining_length(&mut packet, remaining);
    packet.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
    packet.extend_from_slice(topic_bytes);

    if qos > 0 {
        // SAFETY: checked above that packet_id is Some when qos > 0
        let id = packet_id.unwrap();
        packet.extend_from_slice(&id.to_be_bytes());
    }

    packet.extend_from_slice(payload);

    Ok(packet)
}

/// Encode an MQTT 3.1.1 SUBSCRIBE packet.
///
/// Returns the complete packet bytes including the fixed header,
/// packet identifier, topic filter, and requested QoS.
pub fn encode_subscribe(topic: &str, packet_id: u16) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(0x82); // SUBSCRIBE (type 8, flags 2)

    let topic_bytes = topic.as_bytes();
    let remaining = 2 + 2 + topic_bytes.len() + 1; // pkt_id + topic_lp + sub_opts
    encode_remaining_length(&mut packet, remaining);

    packet.extend_from_slice(&packet_id.to_be_bytes());
    packet.extend_from_slice(&(topic_bytes.len() as u16).to_be_bytes());
    packet.extend_from_slice(topic_bytes);
    packet.push(0x01); // Requested QoS 1

    packet
}

/// Encode a PUBACK packet for the given packet identifier.
pub fn encode_puback(packet_id: u16) -> [u8; 4] {
    [0x40, 0x02, (packet_id >> 8) as u8, (packet_id & 0xFF) as u8]
}

/// Encode a PINGREQ packet (keepalive probe).
pub fn encode_pingreq() -> [u8; 2] {
    [0xC0, 0x00]
}

/// Encode a PINGRESP packet (response to PINGREQ).
pub fn encode_pingresp() -> [u8; 2] {
    [0xD0, 0x00]
}

/// Encode a DISCONNECT packet (clean session termination).
pub fn encode_disconnect() -> [u8; 2] {
    [0xE0, 0x00]
}

/// Parse an incoming MQTT PUBLISH packet, extracting topic and payload.
///
/// Handles QoS 0 and QoS 1 packets. Returns `None` if the buffer doesn't
/// contain a PUBLISH packet (wrong type) or is too short to parse.
/// For QoS 1, returns the packet ID so the caller can send PUBACK.
pub struct IncomingPublish<'a> {
    pub topic: &'a str,
    pub payload: &'a [u8],
    pub packet_id: Option<u16>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublishParseError {
    /// Buffer too short.
    TooShort,
    /// Not a PUBLISH packet.
    NotPublish(u8),
    /// Invalid remaining-length encoding.
    InvalidRemainingLength,
    /// Topic is not valid UTF-8.
    InvalidTopic,
}

pub fn parse_incoming_publish(buf: &[u8]) -> Result<IncomingPublish<'_>, PublishParseError> {
    if buf.is_empty() {
        return Err(PublishParseError::TooShort);
    }
    let pkt_type = buf[0] >> 4;
    if pkt_type != 3 {
        return Err(PublishParseError::NotPublish(buf[0]));
    }

    let (remaining_len, header_size) = decode_remaining_length(buf)
        .ok_or(PublishParseError::InvalidRemainingLength)?;

    if buf.len() < header_size + remaining_len {
        return Err(PublishParseError::TooShort);
    }

    let body = &buf[header_size..header_size + remaining_len];
    if body.len() < 2 {
        return Err(PublishParseError::TooShort);
    }

    let topic_len = u16::from_be_bytes([body[0], body[1]]) as usize;
    if body.len() < 2 + topic_len {
        return Err(PublishParseError::TooShort);
    }

    let topic = core::str::from_utf8(&body[2..2 + topic_len])
        .map_err(|_| PublishParseError::InvalidTopic)?;

    let qos = (buf[0] >> 1) & 0x03;
    let mut idx = 2 + topic_len;
    let packet_id = if qos > 0 {
        if body.len() < idx + 2 {
            return Err(PublishParseError::TooShort);
        }
        let id = u16::from_be_bytes([body[idx], body[idx + 1]]);
        idx += 2;
        Some(id)
    } else {
        None
    };

    let payload = &body[idx..];
    Ok(IncomingPublish {
        topic,
        payload,
        packet_id,
    })
}

/// Parse a SUBACK packet from raw bytes.
///
/// Validates that the packet type, packet identifier, and return code
/// are all correct. Returns `Ok(())` on success.
#[derive(Debug, PartialEq, Eq)]
pub enum SubackError {
    /// Buffer too short.
    TooShort,
    /// First byte is not 0x90 (SUBACK type).
    WrongType(u8),
    /// Invalid remaining-length encoding.
    InvalidRemainingLength,
    /// Payload too short to contain packet ID.
    PayloadTooShort,
    /// Packet identifier mismatch.
    PacketIdMismatch { expected: u16, got: u16 },
    /// Subscription rejected (return code 0x80).
    SubscriptionFailed(u8),
}

pub fn parse_suback(buf: &[u8], expected_packet_id: u16) -> Result<(), SubackError> {
    if buf.is_empty() {
        return Err(SubackError::TooShort);
    }
    if buf[0] != 0x90 {
        return Err(SubackError::WrongType(buf[0]));
    }

    // Decode remaining length
    let (remaining_len, header_size) =
        decode_remaining_length(buf).ok_or(SubackError::InvalidRemainingLength)?;

    if buf.len() < header_size + remaining_len {
        return Err(SubackError::TooShort);
    }

    let payload = &buf[header_size..header_size + remaining_len];

    if payload.len() < 3 {
        return Err(SubackError::PayloadTooShort);
    }

    // Parse packet identifier (first 2 bytes)
    let ack_pkt_id = u16::from_be_bytes([payload[0], payload[1]]);
    if ack_pkt_id != expected_packet_id {
        return Err(SubackError::PacketIdMismatch {
            expected: expected_packet_id,
            got: ack_pkt_id,
        });
    }

    // MQTT 3.1.1 SUBACK: no properties field. The return code immediately
    // follows the 2-byte packet identifier.
    if payload.len() < 3 {
        return Err(SubackError::PayloadTooShort);
    }
    let return_code = payload[2];
    if return_code == 0x80 {
        return Err(SubackError::SubscriptionFailed(return_code));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_remaining_length_helper(buf: &mut Vec<u8>, len: usize) {
        encode_remaining_length(buf, len);
    }

    // ── encode_remaining_length tests ──

    #[test]
    fn test_encode_remaining_length_zero() {
        let mut buf = Vec::new();
        encode_remaining_length(&mut buf, 0);
        assert_eq!(buf, vec![0x00]);
    }

    #[test]
    fn test_encode_remaining_length_single_byte() {
        let mut buf = Vec::new();
        encode_remaining_length(&mut buf, 127);
        assert_eq!(buf, vec![0x7F]);
    }

    #[test]
    fn test_encode_remaining_length_two_bytes() {
        let mut buf = Vec::new();
        encode_remaining_length(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);
    }

    #[test]
    fn test_encode_remaining_length_three_bytes() {
        let mut buf = Vec::new();
        encode_remaining_length(&mut buf, 16384);
        assert_eq!(buf, vec![0x80, 0x80, 0x01]);
    }

    #[test]
    fn test_encode_remaining_length_max() {
        let mut buf = Vec::new();
        // MQTT max remaining length is 268,435,455
        encode_remaining_length(&mut buf, 268_435_455);
        assert_eq!(buf, vec![0xFF, 0xFF, 0xFF, 0x7F]);
    }

    // ── append_lp_string tests ──

    #[test]
    fn test_append_lp_string_basic() {
        let mut buf = Vec::new();
        append_lp_string(&mut buf, "hello");
        // Length prefix (2 bytes big-endian) + string bytes
        let mut expected = Vec::new();
        expected.extend_from_slice(&[0x00, 0x05]); // length = 5
        expected.extend_from_slice(b"hello");
        assert_eq!(buf, expected);
    }

    #[test]
    fn test_append_lp_string_empty() {
        let mut buf = Vec::new();
        append_lp_string(&mut buf, "");
        assert_eq!(buf, vec![0x00, 0x00]);
    }

    // ── encode_connect tests ──

    #[test]
    fn test_encode_connect_no_auth() {
        let config = ConnectConfig {
            client_id: "test_device",
            lwt_topic: "test/avail",
            username: None,
            password: None,
            keep_alive: 30,
        };
        let packet = encode_connect(&config);

        // Fixed header
        assert_eq!(packet[0], 0x10); // CONNECT type
                                     // Variable header starts after remaining length
                                     // Check protocol name "MQTT"
        let rl_end = 2; // single-byte remaining length
        assert_eq!(
            &packet[rl_end..rl_end + 6],
            &[0x00, 0x04, b'M', b'Q', b'T', b'T']
        );
        // Protocol level
        assert_eq!(packet[rl_end + 6], 0x04);
        // Connect flags: clean session + will flag + will qos1 + will retain
        let expected_flags: u8 = 0x02 | (1 << 2) | (1 << 3) | (1 << 5);
        assert_eq!(packet[rl_end + 7], expected_flags);
        // Keep alive
        assert_eq!(&packet[rl_end + 8..rl_end + 10], &[0x00, 0x1E]); // 30 big-endian
    }

    #[test]
    fn test_encode_connect_with_auth() {
        let config = ConnectConfig {
            client_id: "launa_abc",
            lwt_topic: "launa/abc/avail",
            username: Some("user"),
            password: Some("pass"),
            keep_alive: 60,
        };
        let packet = encode_connect(&config);

        assert_eq!(packet[0], 0x10);
        let rl_end = 2;
        let connect_flags = packet[rl_end + 7];
        assert_ne!(connect_flags & (1 << 7), 0); // username flag
        assert_ne!(connect_flags & (1 << 6), 0); // password flag

        // Verify keep alive is 60
        assert_eq!(&packet[rl_end + 8..rl_end + 10], &[0x00, 0x3C]);
    }

    #[test]
    fn test_encode_connect_contains_client_id_and_lwt() {
        let config = ConnectConfig {
            client_id: "myclient",
            lwt_topic: "my/lwt",
            username: None,
            password: None,
            keep_alive: 30,
        };
        let packet = encode_connect(&config);
        let packet_str = core::str::from_utf8(&packet).unwrap();
        assert!(packet_str.contains("myclient"));
        assert!(packet_str.contains("my/lwt"));
        assert!(packet_str.contains("offline"));
    }

    // ── parse_connack tests ──

    #[test]
    fn test_parse_connack_success() {
        let buf = [0x20, 0x02, 0x00, 0x00];
        assert!(parse_connack(&buf).is_ok());
    }

    #[test]
    fn test_parse_connack_too_short() {
        assert_eq!(parse_connack(&[0x20, 0x02]), Err(ConnackError::TooShort));
        assert_eq!(parse_connack(&[0x20]), Err(ConnackError::TooShort));
        assert_eq!(parse_connack(&[]), Err(ConnackError::TooShort));
    }

    #[test]
    fn test_parse_connack_wrong_type() {
        let buf = [0x30, 0x02, 0x00, 0x00]; // PUBLISH, not CONNACK
        assert_eq!(parse_connack(&buf), Err(ConnackError::WrongType(0x30)));
    }

    #[test]
    fn test_parse_connack_failure_reason_code() {
        let buf = [0x20, 0x02, 0x00, 0x05]; // reason code 5 = Unauthorized
        assert_eq!(parse_connack(&buf), Err(ConnackError::ReasonCode(5)));
    }

    // ── encode_publish tests ──

    #[test]
    fn test_encode_publish_qos0_no_retain() {
        let packet = encode_publish("test/topic", b"hello", 0, false, None).unwrap();
        assert_eq!(packet[0], 0x30); // PUBLISH, QoS 0, no retain

        // Decode topic length and topic
        let (_, hdr_size) = decode_remaining_length(&packet).unwrap();
        let mut idx = hdr_size;
        let topic_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
        idx += 2;
        assert_eq!(topic_len, 10);
        assert_eq!(
            core::str::from_utf8(&packet[idx..idx + topic_len]).unwrap(),
            "test/topic"
        );
    }

    #[test]
    fn test_encode_publish_qos1_with_retain() {
        let packet = encode_publish("a/b", b"data", 1, true, Some(42)).unwrap();
        assert_eq!(packet[0], 0x33); // PUBLISH, QoS 1, retain

        let (_, hdr_size) = decode_remaining_length(&packet).unwrap();
        let mut idx = hdr_size;
        let topic_len = u16::from_be_bytes([packet[idx], packet[idx + 1]]) as usize;
        idx += 2 + topic_len;
        // Packet ID
        let pkt_id = u16::from_be_bytes([packet[idx], packet[idx + 1]]);
        assert_eq!(pkt_id, 42);
    }

    #[test]
    fn test_encode_publish_empty_payload() {
        let packet = encode_publish("t", b"", 0, false, None).unwrap();
        assert_eq!(packet[0], 0x30);
        // Verify the packet is well-formed by checking remaining length
        let (remaining, hdr_size) = decode_remaining_length(&packet).unwrap();
        assert_eq!(remaining + hdr_size, packet.len());
    }

    #[test]
    fn test_encode_publish_qos1_missing_packet_id() {
        let result = encode_publish("test/topic", b"hello", 1, false, None);
        assert_eq!(result, Err(PublishError::MissingPacketId));
    }

    #[test]
    fn test_encode_publish_invalid_qos() {
        let result = encode_publish("test/topic", b"hello", 2, false, Some(1));
        assert_eq!(result, Err(PublishError::InvalidQoS(2)));
    }

    // ── encode_subscribe tests ──

    #[test]
    fn test_encode_subscribe_basic() {
        let packet = encode_subscribe("cmd/test/#", 1);
        assert_eq!(packet[0], 0x82); // SUBSCRIBE type

        let (_, hdr_size) = decode_remaining_length(&packet).unwrap();
        let payload = &packet[hdr_size..];

        // Packet ID
        let pkt_id = u16::from_be_bytes([payload[0], payload[1]]);
        assert_eq!(pkt_id, 1);

        // Topic filter length (no properties byte in MQTT 3.1.1)
        let topic_len = u16::from_be_bytes([payload[2], payload[3]]) as usize;
        assert_eq!(topic_len, 10);
        assert_eq!(
            core::str::from_utf8(&payload[4..4 + topic_len]).unwrap(),
            "cmd/test/#"
        );

        // Requested QoS 1
        assert_eq!(payload[4 + topic_len], 0x01);
    }

    #[test]
    fn test_encode_subscribe_large_packet_id() {
        let packet = encode_subscribe("topic", 65535);
        let (_, hdr_size) = decode_remaining_length(&packet).unwrap();
        let payload = &packet[hdr_size..];
        let pkt_id = u16::from_be_bytes([payload[0], payload[1]]);
        assert_eq!(pkt_id, 65535);
    }

    // ── encode_puback tests ──

    #[test]
    fn test_encode_puback() {
        let puback = encode_puback(42);
        assert_eq!(puback, [0x40, 0x02, 0x00, 0x2A]);
    }

    #[test]
    fn test_encode_puback_max_id() {
        let puback = encode_puback(65535);
        assert_eq!(puback, [0x40, 0x02, 0xFF, 0xFF]);
    }

    #[test]
    fn test_encode_puback_zero_id() {
        let puback = encode_puback(0);
        assert_eq!(puback, [0x40, 0x02, 0x00, 0x00]);
    }

    // ── encode_pingreq/pingresp/disconnect tests ──

    #[test]
    fn test_encode_pingreq() {
        assert_eq!(encode_pingreq(), [0xC0, 0x00]);
    }

    #[test]
    fn test_encode_pingresp() {
        assert_eq!(encode_pingresp(), [0xD0, 0x00]);
    }

    #[test]
    fn test_encode_disconnect() {
        assert_eq!(encode_disconnect(), [0xE0, 0x00]);
    }

    // ── parse_suback tests ──

    /// Build a MQTT 3.1.1 SUBACK packet: [0x90, remaining_len, pkt_id_hi, pkt_id_lo, return_code]
    fn build_suback(packet_id: u16, return_code: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(0x90); // SUBACK type
        encode_remaining_length_helper(&mut buf, 3); // pkt_id(2) + return_code(1)
        buf.extend_from_slice(&packet_id.to_be_bytes());
        buf.push(return_code);
        buf
    }

    #[test]
    fn test_parse_suback_success() {
        let buf = build_suback(1, 0x00); // Success, QoS 0 granted
        assert!(parse_suback(&buf, 1).is_ok());
    }

    #[test]
    fn test_parse_suback_success_qos1() {
        let buf = build_suback(42, 0x01); // QoS 1 granted
        assert!(parse_suback(&buf, 42).is_ok());
    }

    #[test]
    fn test_parse_suback_wrong_type() {
        let buf = [0x30, 0x02, 0x00, 0x01]; // PUBLISH, not SUBACK
        assert_eq!(parse_suback(&buf, 1), Err(SubackError::WrongType(0x30)));
    }

    #[test]
    fn test_parse_suback_empty() {
        assert_eq!(parse_suback(&[], 1), Err(SubackError::TooShort));
    }

    #[test]
    fn test_parse_suback_packet_id_mismatch() {
        let buf = build_suback(2, 0x00);
        assert_eq!(
            parse_suback(&buf, 1),
            Err(SubackError::PacketIdMismatch {
                expected: 1,
                got: 2
            })
        );
    }

    #[test]
    fn test_parse_suback_subscription_failed() {
        let buf = build_suback(1, 0x80); // Failure
        assert_eq!(
            parse_suback(&buf, 1),
            Err(SubackError::SubscriptionFailed(0x80))
        );
    }

    #[test]
    fn test_parse_suback_multiple_subscriptions() {
        // MQTT 3.1.1 SUBACK with multiple return codes
        let mut buf = Vec::new();
        buf.push(0x90);
        encode_remaining_length_helper(&mut buf, 4); // pkt_id(2) + 2 return codes
        buf.extend_from_slice(&5u16.to_be_bytes());
        buf.push(0x01); // QoS 1 granted for first subscription
        buf.push(0x00); // QoS 0 granted for second subscription
        assert!(parse_suback(&buf, 5).is_ok());
    }

    // ── Roundtrip encode/decode tests ──

    #[test]
    fn test_remaining_length_roundtrip() {
        for &len in &[0, 1, 127, 128, 16383, 16384, 2097151, 268_435_455] {
            let mut buf = Vec::new();
            encode_remaining_length(&mut buf, len);
            // Prepend a dummy packet type byte for decode_remaining_length
            let mut decode_buf = vec![0x30];
            decode_buf.extend_from_slice(&buf);
            let (decoded, header_size) = decode_remaining_length(&decode_buf).unwrap();
            assert_eq!(decoded, len, "roundtrip failed for len={}", len);
            assert_eq!(header_size, 1 + buf.len());
        }
    }

    #[test]
    fn test_encode_connect_has_mqtt_protocol() {
        let config = ConnectConfig {
            client_id: "x",
            lwt_topic: "y",
            username: None,
            password: None,
            keep_alive: 30,
        };
        let packet = encode_connect(&config);
        // Find "MQTT" in the packet
        let mqtt_bytes = b"MQTT";
        let mut found = false;
        for i in 0..packet.len().saturating_sub(5) {
            if packet[i] == 0x00 && packet[i + 1] == 0x04 && &packet[i + 2..i + 6] == mqtt_bytes {
                found = true;
                break;
            }
        }
        assert!(found, "CONNECT packet should contain MQTT protocol name");
    }
}
