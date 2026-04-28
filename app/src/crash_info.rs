//! Crash information persistence via NVS.
//!
//! On panic, the panic handler stores a crash reason + truncated message to NVS.
//! On next boot, after MQTT connects, the crash alarm is published and only then
//! is the NVS flag cleared. If the publish fails, the flag persists for the next boot.
//!
//! NVS write pre-check: the panic handler only writes if no crash flag is already
//! stored, preventing flash wear in crash loops.

use alloc::string::String;
use core::sync::atomic::{AtomicPtr, Ordering};

use log::{info, warn};

use crate::*;

const CRASH_NAMESPACE: &str = "crash";
const KEY_MAGIC: &str = "magic";
const KEY_REASON: &str = "reason";
const KEY_MESSAGE: &str = "message";

/// Magic value written to NVS to indicate a crash was recorded.
const CRASH_MAGIC: u8 = 0xC7;

/// Maximum length for the panic message stored in NVS.
/// Limited to keep within typical NVS entry size constraints.
const MAX_MESSAGE_LEN: usize = 200;

/// Case-insensitive substring search without heap allocation.
/// Compares each byte pair directly using ASCII case folding.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle_bytes = needle.as_bytes();
    let haystack_bytes = haystack.as_bytes();
    if needle_bytes.len() > haystack_bytes.len() {
        return false;
    }
    haystack_bytes.windows(needle_bytes.len()).any(|window| {
        window
            .iter()
            .zip(needle_bytes.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
    })
}

/// Crash reason codes stored as a u8 in NVS.
///
/// The panic handler categorizes panics by inspecting the panic message.
/// Additional reasons can be added for OOM handler, stack canary, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrashReason {
    /// Generic panic (default when no specific category matches).
    Panic = 1,
    /// Out-of-memory (allocation failure / OOM).
    Oom = 2,
    /// Stack overflow detected via canary or exceeded bounds.
    StackOverflow = 3,
    /// Assertion failed (assert!, assert_eq!, assert_ne!, debug_assert).
    Assertion = 4,
    /// Array index out of bounds.
    IndexOutOfBounds = 5,
    /// Arithmetic overflow (add/mul with overflow in debug builds).
    ArithmeticOverflow = 6,
    /// Panic reason could not be determined.
    Unknown = 255,
}

impl CrashReason {
    /// Convert from the u8 stored in NVS.
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Panic,
            2 => Self::Oom,
            3 => Self::StackOverflow,
            4 => Self::Assertion,
            5 => Self::IndexOutOfBounds,
            6 => Self::ArithmeticOverflow,
            _ => Self::Unknown,
        }
    }

    /// Human-readable label for MQTT alarm payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Oom => "oom",
            Self::StackOverflow => "stack_overflow",
            Self::Assertion => "assertion",
            Self::IndexOutOfBounds => "index_out_of_bounds",
            Self::ArithmeticOverflow => "arithmetic_overflow",
            Self::Unknown => "unknown",
        }
    }

    /// Classify a panic info message into a crash reason.
    ///
    /// Inspects the formatted panic message for known patterns produced by
    /// Rust's standard panic macros and allocation failure paths.
    /// Uses case-insensitive matching without heap allocation.
    pub fn classify(message: &str) -> Self {
        // OOM patterns: "out of memory", "allocation failed", "oom"
        if contains_ci(message, "out of memory")
            || contains_ci(message, "allocation failed")
            || contains_ci(message, "oom")
        {
            return Self::Oom;
        }

        // Stack overflow: could come from embassy task overflow detection
        if contains_ci(message, "stack overflow") {
            return Self::StackOverflow;
        }

        // Assertion patterns
        if contains_ci(message, "assertion") {
            return Self::Assertion;
        }

        // Index out of bounds
        if contains_ci(message, "index out of bounds") || contains_ci(message, "range start index ")
        {
            return Self::IndexOutOfBounds;
        }

        // Arithmetic overflow
        if contains_ci(message, "attempt to add with overflow")
            || contains_ci(message, "attempt to multiply with overflow")
            || contains_ci(message, "attempt to subtract with overflow")
            || contains_ci(message, "overflow")
        {
            return Self::ArithmeticOverflow;
        }

        Self::Panic
    }
}

/// Static pointer to NVS handle, set once after NVS init and cleared before
/// OTA consumes the flash. The panic handler reads this to write crash info.
/// SAFETY: Only accessed from the panic handler (single-threaded, no reentrancy
/// concern since panics don't nest).
static NVS_PTR: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(core::ptr::null_mut());

/// Store the NVS handle reference for use by the panic handler.
/// Must be called after NVS init. Call `clear_nvs_ptr()` before consuming
/// the NVS handle for OTA.
///
/// # Safety
/// `nvs` must remain valid and not be moved or consumed until `clear_nvs_ptr`
/// is called.
pub unsafe fn set_nvs_ptr(nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>) {
    NVS_PTR.store(nvs as *mut _ as *mut core::ffi::c_void, Ordering::Relaxed);
}

/// Clear the stored NVS pointer. Must be called before the NVS handle is consumed.
pub fn clear_nvs_ptr() {
    NVS_PTR.store(core::ptr::null_mut(), Ordering::Relaxed);
}

/// Write crash info to NVS. Called from the panic handler.
///
/// Pre-checks whether a crash flag is already stored (read-only, no flash wear).
/// Only writes if no flag exists, preventing repeated writes in crash loops.
///
/// Returns `true` if crash info was written, `false` if skipped (already stored
/// or NVS unavailable).
pub(crate) fn write_crash_info(reason: CrashReason, message: &str) -> bool {
    let ptr = NVS_PTR.load(Ordering::Relaxed);
    if ptr.is_null() {
        return false;
    }

    // SAFETY: pointer was set by `set_nvs_ptr` and is still valid
    // (panic handler runs before `clear_nvs_ptr` is called).
    let nvs = unsafe { &mut *(ptr as *mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>) };
    let ns = esp_nvs::Key::from_str(CRASH_NAMESPACE);

    // Pre-check: only write if no crash flag already stored (avoid flash wear)
    if nvs
        .get::<u8>(&ns, &esp_nvs::Key::from_str(KEY_MAGIC))
        .is_ok()
    {
        // Crash flag already present — skip write to preserve flash
        return false;
    }

    let mut success = true;

    if let Err(e) = nvs.set(&ns, &esp_nvs::Key::from_str(KEY_MAGIC), CRASH_MAGIC) {
        success = false;
        // Can't log here (might be in panic), just continue
        let _ = e;
    }

    if let Err(e) = nvs.set(&ns, &esp_nvs::Key::from_str(KEY_REASON), reason as u8) {
        success = false;
        let _ = e;
    }

    // Truncate message to MAX_MESSAGE_LEN
    let truncated = if message.len() > MAX_MESSAGE_LEN {
        &message[..MAX_MESSAGE_LEN]
    } else {
        message
    };

    if let Err(e) = nvs.set(&ns, &esp_nvs::Key::from_str(KEY_MESSAGE), truncated) {
        success = false;
        let _ = e;
    }

    success
}

/// Crash info read from NVS on boot.
#[derive(Debug, Clone)]
pub struct CrashInfo {
    pub reason: CrashReason,
    pub message: String,
}

/// Read crash info from NVS during boot. Returns `None` if no crash flag is stored.
pub fn read_crash_info(
    nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>,
) -> Option<CrashInfo> {
    let ns = esp_nvs::Key::from_str(CRASH_NAMESPACE);

    // Check magic byte
    let magic = nvs
        .get::<u8>(&ns, &esp_nvs::Key::from_str(KEY_MAGIC))
        .ok()?;
    if magic != CRASH_MAGIC {
        return None;
    }

    let reason_u8 = nvs
        .get::<u8>(&ns, &esp_nvs::Key::from_str(KEY_REASON))
        .ok()?;
    let reason = CrashReason::from_u8(reason_u8);
    let message = nvs
        .get::<String>(&ns, &esp_nvs::Key::from_str(KEY_MESSAGE))
        .unwrap_or_else(|_| String::from("<message unavailable>"));

    info!(
        "Crash info found in NVS: reason={}, message={}",
        reason.as_str(),
        &message[..message.len().min(80)]
    );

    Some(CrashInfo { reason, message })
}

/// Clear crash info from NVS. Called only after the crash alarm has been
/// successfully published to MQTT.
pub fn clear_crash_info(nvs: &mut esp_nvs::Nvs<esp_storage::FlashStorage<'static>>) {
    let ns = esp_nvs::Key::from_str(CRASH_NAMESPACE);
    let _ = nvs.delete(&ns, &esp_nvs::Key::from_str(KEY_MESSAGE));
    let _ = nvs.delete(&ns, &esp_nvs::Key::from_str(KEY_REASON));
    if let Err(e) = nvs.delete(&ns, &esp_nvs::Key::from_str(KEY_MAGIC)) {
        warn!("Failed to clear crash info from NVS: {:?}", e);
    } else {
        info!("Crash info cleared from NVS");
    }
}

/// Build a JSON payload for the crash alarm MQTT message.
pub fn crash_alarm_json(crash: &CrashInfo, firmware_version: &str) -> String {
    let uptime = uptime_secs();
    alloc::format!(
        r#"{{"level":"error","message":"crash_alarm","crash_reason":"{}","crash_message":"{}","timestamp":{},"firmware_version":"{}"}}"#,
        crash.reason.as_str(),
        launa_mqtt::escape::escape_json_string(&crash.message),
        uptime,
        launa_mqtt::escape::escape_json_string(firmware_version),
    )
}
