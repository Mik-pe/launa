//! RS-485 raw byte burst capture for remote diagnostics.
//!
//! When triggered via MQTT, captures raw UART byte chunks (both RX and TX)
//! with timestamps, then publishes as JSON. Frame decoding is done by the
//! frontend/decoder, not here.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Instant;
use log::{info, warn};

/// Channel for sending sniff capture JSON from uart_task to MQTT task.
pub(crate) static SNIFF_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

/// Set to a non-zero value to request a one-shot burst capture.
/// The uart_task reads this, captures raw byte chunks with timestamps,
/// publishes the result to SNIFF_CHANNEL, then auto-clears it.
/// A value of 0 means no capture is active.
pub(crate) static SNIFF_CAPTURE: core::sync::atomic::AtomicU16 =
    core::sync::atomic::AtomicU16::new(0);

/// Maximum capture duration in microseconds (2s to catch FEBF query cycle).
const SNIFF_MAX_DURATION_US: u64 = 2_000_000;

/// Maximum total hex bytes across all chunks before forcing capture end.
/// Prevents unbounded RAM use. ~200 frames * ~30 bytes = ~6KB hex.
const SNIFF_MAX_TOTAL_HEX_LEN: usize = 12_000;

/// Direction marker for a raw byte chunk.
#[derive(Clone, Copy)]
pub(crate) enum Direction {
    Rx,
    Tx,
}

/// A raw byte chunk captured from the RS-485 bus with timestamp.
struct RawChunk {
    ts_us: u64,
    dir: Direction,
    hex: String,
}

/// Build the JSON burst capture payload from collected chunks.
///
/// Format: `{"capture_us":N,"chunks":[["R",ts,"HEX"],...]}`
fn build_sniff_json(chunks: &[RawChunk], capture_us: u64) -> Vec<u8> {
    let mut json = String::with_capacity(80 + chunks.len() * 60);
    json.push_str("{\"capture_us\":");
    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", capture_us));
    json.push_str(",\"chunks\":[");

    for (i, c) in chunks.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let dir = match c.dir {
            Direction::Rx => 'R',
            Direction::Tx => 'T',
        };
        json.push_str("[\"");
        json.push(dir);
        json.push_str("\",");
        let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", c.ts_us));
        json.push_str(",\"");
        json.push_str(&c.hex);
        json.push_str("\"]");
    }

    json.push_str("]}");
    json.into_bytes()
}

/// Sniff burst capture state.
pub(crate) struct SniffState {
    active: bool,
    start: Option<Instant>,
    chunks: Vec<RawChunk>,
    total_hex_len: usize,
}

impl SniffState {
    pub const fn new() -> Self {
        SniffState {
            active: false,
            start: None,
            chunks: Vec::new(),
            total_hex_len: 0,
        }
    }

    pub fn check_start(&mut self) {
        if !self.active {
            let requested = SNIFF_CAPTURE.load(Ordering::Relaxed);
            if requested > 0 {
                self.active = true;
                self.start = Some(Instant::now());
                self.chunks.clear();
                self.total_hex_len = 0;
            }
        }
    }

    /// Record a raw byte chunk. Returns true if capture should end this call.
    pub fn record_chunk(&mut self, dir: Direction, data: &[u8]) -> bool {
        if !self.active {
            return false;
        }
        let Some(start) = self.start else {
            return false;
        };
        let ts_us = start.elapsed().as_micros();
        let hex = launa_protocol::hex::to_hex(data);
        self.total_hex_len += hex.len();

        self.chunks.push(RawChunk { ts_us, dir, hex });

        ts_us >= SNIFF_MAX_DURATION_US || self.total_hex_len >= SNIFF_MAX_TOTAL_HEX_LEN
    }

    /// Finalize capture: build JSON, publish, reset state.
    pub fn finish(&mut self) {
        let capture_us = self.start.unwrap().elapsed().as_micros();
        let count = self.chunks.len();
        let json = build_sniff_json(&self.chunks, capture_us);
        self.chunks.clear();
        self.total_hex_len = 0;
        self.active = false;
        self.start = None;
        SNIFF_CAPTURE.store(0, Ordering::Relaxed);
        if SNIFF_CHANNEL.try_send(json).is_err() {
            warn!("SNIFF_CHANNEL full, dropping burst capture");
        }
        info!(
            "Sniff burst capture complete: {} chunks in {}us",
            count, capture_us
        );
    }
}
