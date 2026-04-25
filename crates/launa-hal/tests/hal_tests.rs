//! HAL integration tests for mock transport and mock network.

use launa_hal::network::mock::MockNetwork;
use launa_hal::transport::mock::MockTransport;
use launa_hal::transport::TransportError;
use launa_hal::Network;
use launa_hal::Transport;

/// Helper: poll an async future to completion synchronously.
/// MockTransport's async methods always complete immediately,
/// so this is safe for test use.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

// MockTransport tests

#[test]
fn test_mock_transport_new_is_empty() {
    let t = MockTransport::new();
    assert!(
        !t.has_incoming(),
        "new transport should have no incoming data"
    );
    assert!(
        t.written().is_empty(),
        "new transport should have no written data"
    );
}

#[test]
fn test_mock_transport_write_and_read_back() {
    let mut t = MockTransport::new();

    // Write data
    block_on(t.write(&[0x01, 0x02, 0x03])).unwrap();
    assert_eq!(t.written(), &[0x01, 0x02, 0x03]);

    // Inject data to read
    t.inject(&[0xAA, 0xBB, 0xCC]);
    assert!(t.has_incoming());

    let mut buf = [0u8; 3];
    let n = block_on(t.read(&mut buf)).unwrap();
    assert_eq!(n, 3);
    assert_eq!(buf, [0xAA, 0xBB, 0xCC]);
    assert!(!t.has_incoming());
}

#[test]
fn test_mock_transport_returns_0_when_empty() {
    let mut t = MockTransport::new();
    let mut buf = [0u8; 10];
    let n = block_on(t.read(&mut buf)).unwrap();
    assert_eq!(n, 0, "reading from empty transport should return 0 bytes");
}

#[test]
fn test_mock_transport_clear_written() {
    let mut t = MockTransport::new();
    block_on(t.write(&[0x42])).unwrap();
    assert_eq!(t.written(), &[0x42]);

    t.clear_written();
    assert!(t.written().is_empty());
}

#[test]
fn test_mock_transport_incremental_inject() {
    let mut t = MockTransport::new();
    t.inject(&[0x01]);
    t.inject(&[0x02]);
    t.inject(&[0x03]);

    let mut buf = [0u8; 10];
    let n = block_on(t.read(&mut buf)).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[0x01, 0x02, 0x03]);
}

#[test]
fn test_mock_transport_partial_read() {
    let mut t = MockTransport::new();
    t.inject(&[0x01, 0x02, 0x03, 0x04, 0x05]);

    let mut buf = [0u8; 2];
    let n = block_on(t.read(&mut buf)).unwrap();
    assert_eq!(n, 2);
    assert_eq!(buf, [0x01, 0x02]);

    // Remaining bytes still available
    assert!(t.has_incoming());

    let mut buf2 = [0u8; 10];
    let n2 = block_on(t.read(&mut buf2)).unwrap();
    assert_eq!(n2, 3);
    assert_eq!(&buf2[..3], &[0x03, 0x04, 0x05]);
    assert!(!t.has_incoming());
}

#[test]
fn test_mock_transport_flush_is_noop() {
    let mut t = MockTransport::new();
    block_on(t.flush()).unwrap();
}

#[test]
fn test_mock_transport_multiple_writes() {
    let mut t = MockTransport::new();
    block_on(t.write(&[0x01])).unwrap();
    block_on(t.write(&[0x02])).unwrap();
    block_on(t.write(&[0x03])).unwrap();
    assert_eq!(t.written(), &[0x01, 0x02, 0x03]);

    // Clear and write again
    t.clear_written();
    block_on(t.write(&[0xAA])).unwrap();
    assert_eq!(t.written(), &[0xAA]);
}

#[test]
fn test_mock_transport_full_lifecycle() {
    let mut t = MockTransport::new();

    // 1. Inject incoming data
    t.inject(&[0x7E, 0x04, 0xFF, 0xAF, 0x01, 0x42, 0x7E]);

    // 2. Read it back
    let mut buf = [0u8; 7];
    let n = block_on(t.read(&mut buf)).unwrap();
    assert_eq!(n, 7);

    // 3. Write a response
    block_on(t.write(&[0x0A, 0xBF, 0x04])).unwrap();
    assert_eq!(t.written(), &[0x0A, 0xBF, 0x04]);

    // 4. Clear written
    t.clear_written();
    assert!(t.written().is_empty());
}

// MockTransport error injection tests

#[test]
fn test_mock_transport_write_error_injection_and_recovery() {
    let mut t = MockTransport::new();

    // Configure write error
    t.set_write_error(Some(TransportError::Io));
    let result = block_on(t.write(&[0x01]));
    assert!(result.is_err(), "write should fail when error is injected");
    assert!(t.written().is_empty(), "no data should be written on error");

    // Clear error and verify recovery
    t.set_write_error(None);
    block_on(t.write(&[0x42])).unwrap();
    assert_eq!(
        t.written(),
        &[0x42],
        "write should succeed after clearing error"
    );
}

#[test]
fn test_mock_transport_read_error_injection_and_recovery() {
    let mut t = MockTransport::new();

    // Inject data so there's something to read, but configure read error
    t.inject(&[0xAA, 0xBB]);
    t.set_read_error(Some(TransportError::Io));

    let mut buf = [0u8; 10];
    let result = block_on(t.read(&mut buf));
    assert!(result.is_err(), "read should fail when error is injected");

    // Clear error and verify recovery — data should still be readable
    t.set_read_error(None);
    let n = block_on(t.read(&mut buf)).unwrap();
    assert_eq!(
        n, 2,
        "read should return injected bytes after clearing error"
    );
    assert_eq!(&buf[..2], &[0xAA, 0xBB]);
}

#[test]
fn test_mock_transport_read_error_returns_zero_bytes() {
    let mut t = MockTransport::new();
    t.set_read_error(Some(TransportError::Timeout));

    let mut buf = [0u8; 10];
    let result = block_on(t.read(&mut buf));
    assert!(
        result.is_err(),
        "read should return Err when read error is set"
    );
}

// MockNetwork tests

#[test]
fn test_mock_network_new_not_connected() {
    let net = MockNetwork::new();
    assert!(!net.is_connected());
}

#[test]
fn test_mock_network_connect_wifi() {
    let mut net = MockNetwork::new();
    net.connect_wifi("TestSSID", "password123").unwrap();
    assert!(net.is_connected());
}

#[test]
fn test_mock_network_tcp_connect_without_wifi_fails() {
    let mut net = MockNetwork::new();
    let result = net.tcp_connect("192.168.1.1", 8080);
    assert!(result.is_err());
}

#[test]
fn test_mock_network_tcp_connect_flow() {
    let mut net = MockNetwork::new();

    // 1. Connect WiFi
    net.connect_wifi("TestSSID", "password").unwrap();

    // 2. Queue a response
    net.queue_response(vec![0x01, 0x02, 0x03]);

    // 3. Connect TCP
    let mut socket = net.tcp_connect("example.com", 1883).unwrap();

    // 4. Read the queued response
    let mut buf = [0u8; 10];
    let n = socket.read(&mut buf).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[0x01, 0x02, 0x03]);

    // 5. Write data
    socket.write(&[0xAA, 0xBB]).unwrap();
    assert_eq!(net.get_sent_data(), vec![0xAA, 0xBB]);

    // 6. Close
    socket.close().unwrap();
}

#[test]
fn test_mock_network_tcp_empty_response() {
    let mut net = MockNetwork::new();
    net.connect_wifi("SSID", "pass").unwrap();
    // No response queued
    let mut socket = net.tcp_connect("host", 80).unwrap();
    let mut buf = [0u8; 10];
    let n = socket.read(&mut buf).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn test_mock_network_clear_sent_data() {
    let mut net = MockNetwork::new();
    net.connect_wifi("SSID", "pass").unwrap();
    let mut socket = net.tcp_connect("host", 80).unwrap();
    socket.write(&[0x01]).unwrap();
    assert_eq!(net.get_sent_data(), vec![0x01]);

    net.clear_sent_data();
    assert!(net.get_sent_data().is_empty());
}

#[test]
fn test_mock_network_tracks_connect_params() {
    let mut net = MockNetwork::new();
    net.connect_wifi("SSID", "pass").unwrap();
    let _socket = net.tcp_connect("broker.example.com", 1883).unwrap();
    assert_eq!(net.last_connect_addr(), Some("broker.example.com"));
    assert_eq!(net.last_connect_port(), Some(1883));
}
