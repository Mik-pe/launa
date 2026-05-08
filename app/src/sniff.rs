//! RS-485 burst frame capture for remote diagnostics.
//!
//! When triggered via MQTT, captures decoded frames with timestamps for
//! a configurable duration, then publishes the result as JSON.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Instant;
use launa_protocol::frame::Frame;
use log::{info, warn};

use crate::*;

/// Channel for sending raw sniff frame JSON from uart_task to MQTT task.
pub(crate) static SNIFF_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8>, 4> = Channel::new();

/// Set to a non-zero value to request a one-shot burst capture of RS-485 frames.
/// The uart_task reads this, captures up to N frames with timestamps, publishes
/// the result to SNIFF_CHANNEL, then auto-clears it.
/// A value of 0 means no capture is active.
pub(crate) static SNIFF_CAPTURE: core::sync::atomic::AtomicU16 =
    core::sync::atomic::AtomicU16::new(0);

/// Default maximum number of frames to capture in one burst.
const SNIFF_MAX_FRAMES: u16 = 200;

/// Maximum capture duration in microseconds (2s to catch FEBF query cycle).
const SNIFF_MAX_DURATION_US: u64 = 2_000_000;

/// A decoded frame with its timestamp relative to capture start.
struct SniffCaptureFrame {
    ts_us: u64,
    message_type: [u8; 2],
    payload: Vec<u8>,
}

/// Build the JSON burst capture payload from collected frames.
fn build_sniff_json(frames: &[SniffCaptureFrame], capture_us: u64) -> Vec<u8> {
    let mut json = String::with_capacity(80 + frames.len() * 40);
    json.push_str("{\"capture_us\":");
    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", capture_us));
    json.push_str(",\"frame_count\":");
    let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", frames.len()));
    json.push_str(",\"frames\":[");

    for (i, f) in frames.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push('[');
        let _ = core::fmt::Write::write_fmt(&mut json, core::format_args!("{}", f.ts_us));
        json.push_str(",\"");
        let _ = core::fmt::Write::write_fmt(
            &mut json,
            core::format_args!("{:02X}{:02X}", f.message_type[0], f.message_type[1]),
        );
        json.push_str("\",\"");
        let payload_hex = launa_protocol::hex::to_hex(&f.payload);
        json.push_str(&payload_hex);
        json.push_str("\"]");
    }

    json.push_str("]}");
    json.into_bytes()
}

/// Sniff burst capture state, grouped for clarity.
pub(crate) struct SniffState {
    active: bool,
    start: Option<Instant>,
    frames: Vec<SniffCaptureFrame>,
}

impl SniffState {
    pub const fn new() -> Self {
        SniffState {
            active: false,
            start: None,
            frames: Vec::new(),
        }
    }

    /// Maximum frames for the current capture, read from SNIFF_CAPTURE at start.
    max_frames: u16,
}

impl SniffState {
    pub const fn new() -> Self {
        SniffState {
            active: false,
            start: None,
            frames: Vec::new(),
            max_frames: SNIFF_MAX_FRAMES,
        }
    }

    pub fn check_start(&mut self) {
        if !self.active {
            let requested = SNIFF_CAPTURE.load(Ordering::Relaxed);
            if requested > 0 {
                self.active = true;
                self.start = Some(Instant::now());
                self.frames.clear();
                self.max_frames = if requested > SNIFF_MAX_FRAMES {
                    SNIFF_MAX_FRAMES
                } else {
                    requested
                };
            }
        }
    }

    /// Record a decoded frame. Returns true if capture completed this call.
    pub fn record_frame(&mut self, frame: &Frame) -> bool {
        if !self.active {
            return false;
        }
        let Some(start) = self.start else {
            return false;
        };
        let ts_us = start.elapsed().as_micros();
        self.frames.push(SniffCaptureFrame {
            ts_us,
            message_type: frame.message_type,
            payload: frame.payload.clone(),
        });
        self.frames.len() >= self.max_frames as usize || ts_us >= SNIFF_MAX_DURATION_US
    }

    /// Finalize capture: build JSON, publish, reset state.
    pub fn finish(&mut self) {
        let capture_us = self.start.unwrap().elapsed().as_micros();
        let count = self.frames.len();
        let json = build_sniff_json(&self.frames, capture_us);
        self.frames.clear();
        self.active = false;
        self.start = None;
        SNIFF_CAPTURE.store(0, Ordering::Relaxed);
        if SNIFF_CHANNEL.try_send(json).is_err() {
            warn!("SNIFF_CHANNEL full, dropping burst capture");
        }
        info!(
            "Sniff burst capture complete: {} frames in {}us",
            count, capture_us
        );
    }
}
