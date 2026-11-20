//! OPC-UA `DateTime` helpers (Int64, 100 ns ticks since 1601-01-01 UTC).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Ticks between 1601-01-01 and 1970-01-01.
pub const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

/// Converts OPC-UA ticks to a [`SystemTime`] (values <= 0 map to 1601-01-01).
pub fn ticks_to_system_time(ticks: i64) -> SystemTime {
    let ticks = ticks.max(0);
    let rel = ticks - UNIX_EPOCH_TICKS;
    let d = Duration::from_nanos(rel.unsigned_abs() * 100);
    if rel >= 0 {
        UNIX_EPOCH + d
    } else {
        UNIX_EPOCH - d
    }
}

/// Converts a [`SystemTime`] to OPC-UA ticks (saturating).
pub fn system_time_to_ticks(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => UNIX_EPOCH_TICKS.saturating_add((d.as_nanos() / 100).min(i64::MAX as u128) as i64),
        Err(e) => UNIX_EPOCH_TICKS
            .saturating_sub((e.duration().as_nanos() / 100).min(i64::MAX as u128) as i64),
    }
}

/// Converts OPC-UA ticks to Unix time in nanoseconds.
#[inline]
pub fn ticks_to_unix_nanos(ticks: i64) -> i128 {
    (ticks as i128 - UNIX_EPOCH_TICKS as i128) * 100
}

/// Converts Unix time in nanoseconds to OPC-UA ticks.
#[inline]
pub fn unix_nanos_to_ticks(nanos: i128) -> i64 {
    (nanos / 100 + UNIX_EPOCH_TICKS as i128).clamp(i64::MIN as i128, i64::MAX as i128) as i64
}
