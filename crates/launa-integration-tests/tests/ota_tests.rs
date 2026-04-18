//! OTA firmware update integration tests.
//!
//! Tests for OTA firmware download lifecycle using SimHttpServer:
//! - Full download cycle with chunked writes
//! - Variable chunk sizes (small, TCP-sized, large)
//! - Rollback on write failure, finalize failure, and begin failure

use launa_ota::OtaUpdate;

/// Simulates an HTTP firmware download server that serves firmware data
/// in configurable chunk sizes, mimicking how the real OTA downloads
/// firmware over a TCP socket.
struct SimHttpServer {
    firmware: Vec<u8>,
    chunk_size: usize,
}

impl SimHttpServer {
    fn new(firmware: Vec<u8>, chunk_size: usize) -> Self {
        SimHttpServer {
            firmware,
            chunk_size,
        }
    }

    fn download_chunks(&self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < self.firmware.len() {
            let end = (offset + self.chunk_size).min(self.firmware.len());
            chunks.push(self.firmware[offset..end].to_vec());
            offset = end;
        }
        chunks
    }
}

#[test]
fn test_spaapp_ota_full_download_cycle() {
    let mut ota = launa_ota::mock::MockOta::new();

    let firmware: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    let server = SimHttpServer::new(firmware.clone(), 1024);

    ota.begin().unwrap();
    assert!(
        ota.firmware_data.is_empty(),
        "data should be empty after begin"
    );

    let chunks = server.download_chunks();
    assert_eq!(chunks.len(), 4, "4 KiB / 1 KiB chunks = 4 chunks");
    for (i, chunk) in chunks.iter().enumerate() {
        ota.write(chunk).unwrap();
        assert_eq!(
            ota.firmware_data.len(),
            (i + 1) * 1024,
            "data should grow after each write"
        );
    }

    ota.finalize().unwrap();
    assert!(ota.finalized, "should be finalized");

    ota.mark_valid().unwrap();
    assert!(ota.valid, "should be marked valid");

    assert_eq!(ota.firmware_data.len(), 4096);
    assert_eq!(
        ota.firmware_data, firmware,
        "firmware data should match original"
    );
}

#[test]
fn test_spaapp_ota_variable_chunk_sizes() {
    let mut ota = launa_ota::mock::MockOta::new();

    let firmware: Vec<u8> = (0..16384).map(|i| ((i * 7 + 13) % 256) as u8).collect();

    let server = SimHttpServer::new(firmware.clone(), 64);
    ota.begin().unwrap();
    for chunk in server.download_chunks() {
        ota.write(&chunk).unwrap();
    }
    ota.finalize().unwrap();
    ota.mark_valid().unwrap();
    assert_eq!(ota.firmware_data, firmware);

    let mut ota2 = launa_ota::mock::MockOta::new();
    let server2 = SimHttpServer::new(firmware.clone(), 1460);
    ota2.begin().unwrap();
    for chunk in server2.download_chunks() {
        ota2.write(&chunk).unwrap();
    }
    ota2.finalize().unwrap();
    ota2.mark_valid().unwrap();
    assert_eq!(ota2.firmware_data, firmware);
}

#[test]
fn test_spaapp_ota_rollback_on_write_failure() {
    let mut ota = launa_ota::mock::MockOta::new();
    ota.fail_on_write_after = Some(2048);

    let firmware: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    let server = SimHttpServer::new(firmware, 512);

    ota.begin().unwrap();

    let chunks = server.download_chunks();
    for chunk in &chunks {
        let result = ota.write(chunk);
        if result.is_err() {
            break;
        }
    }

    assert_eq!(
        ota.firmware_data.len(),
        2048,
        "should have written exactly 2048 bytes"
    );

    assert!(!ota.valid, "mark_valid should NOT be called after failure");

    assert!(
        !ota.finalized,
        "finalize should NOT have succeeded after failure"
    );

    ota.rollback_and_reboot().unwrap();
    assert!(ota.rolled_back, "should have rolled back");
    assert!(!ota.valid, "should still not be valid after rollback");
}

#[test]
fn test_spaapp_ota_rollback_on_finalize_failure() {
    let mut ota = launa_ota::mock::MockOta::new();
    ota.fail_on_finalize = true;

    let firmware: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
    let server = SimHttpServer::new(firmware.clone(), 512);

    ota.begin().unwrap();
    for chunk in server.download_chunks() {
        ota.write(&chunk).unwrap();
    }
    let result = ota.finalize();
    assert!(
        result.is_err(),
        "finalize should fail when fail_on_finalize is set"
    );

    assert_eq!(ota.firmware_data.len(), 2048);
    assert!(!ota.finalized, "should not be finalized");

    assert!(!ota.valid, "mark_valid should NOT be called");

    ota.rollback_and_reboot().unwrap();
    assert!(ota.rolled_back, "should have rolled back");
}

#[test]
fn test_spaapp_ota_rollback_on_begin_failure() {
    let mut ota = launa_ota::mock::MockOta::new();
    ota.fail_on_begin = true;

    let result = ota.begin();
    assert!(result.is_err(), "begin should fail");

    assert!(ota.firmware_data.is_empty());
    assert!(!ota.valid);
    assert!(!ota.finalized);

    ota.rollback_and_reboot().unwrap();
    assert!(ota.rolled_back);
}
