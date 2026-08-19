//! Time, and whether it can be believed.
//!
//! A Raspberry Pi 5 has no battery-backed clock. On every cold boot
//! `systemd-timesyncd` corrects the time in two steps — restore the timestamp
//! recorded at last shutdown, then contact NTP — and `seshd` starts between
//! them. Measured on TatePi: nine seconds of running with a clock thirteen
//! minutes slow, with startup reconciliation landing inside the window.
//!
//! So SESH separates two things that a single `SystemTime::now()` conflates:
//!
//! - **Durations** — "has a minute passed", "is this token stale". These must
//!   come from [`Clock::mono_ms`], which no clock correction can move. Computing
//!   one from two wall-clock reads that straddle the NTP jump is what expired
//!   the TV's join code mid-scan.
//! - **Recorded instants** — an event's `ts_ms`, a person's `joined_ms`. These
//!   are what a human reads off the log later, so they keep the wall clock and
//!   are marked, via [`Clock::synced`], when it was not yet trustworthy.
//!
//! Spec: `docs/superpowers/specs/2026-08-19-clock-trust.md`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The file `systemd-timesyncd` creates the first time it successfully syncs.
pub const SYNCED_MARKER: &str = "/run/systemd/timesync/synchronized";

/// Payload key marking a row written before the wall clock could be trusted.
///
/// Absent means it could be. Written only when false, so a healthy box's rows
/// are byte-identical to what they were before this existed and the key's mere
/// presence is the signal. Shaped after `exit_observed` in
/// [`reconcile`](crate::reconcile): a measurement SESH could not make, recorded
/// as such rather than guessed at.
pub const CLOCK_SYNCED: &str = "clock_synced";

/// The two clocks SESH needs, and the question of whether one of them is real.
pub trait Clock: Send + Sync {
    /// Wall-clock milliseconds since the Unix epoch. For recorded instants.
    fn now_ms(&self) -> i64;

    /// Milliseconds from an arbitrary fixed origin, never moved by a clock
    /// correction. For durations.
    fn mono_ms(&self) -> i64;

    /// Whether the wall clock has been corrected against a time source yet.
    fn synced(&self) -> bool;
}

/// The real clock: `SystemTime` for instants, [`Instant`] for durations, and
/// the presence of [`SYNCED_MARKER`] for trust.
pub struct SystemClock {
    origin: Instant,
    marker: PathBuf,
    latched: AtomicBool,
}

impl SystemClock {
    /// A clock reading the real `systemd-timesyncd` marker.
    pub fn new() -> Self {
        Self::with_marker(Path::new(SYNCED_MARKER))
    }

    /// A clock reading `marker` instead. For tests, which cannot write `/run`.
    pub fn with_marker(marker: &Path) -> Self {
        Self {
            origin: Instant::now(),
            marker: marker.to_path_buf(),
            latched: AtomicBool::new(false),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_millis() as i64
    }

    fn mono_ms(&self) -> i64 {
        self.origin.elapsed().as_millis() as i64
    }

    /// Latches. Once the marker has been seen the filesystem is never consulted
    /// again, because this answers "was the clock trustworthy when that row was
    /// written" — a `/run` that vanishes later must not change the answer for
    /// rows already written. It also keeps the common path an atomic read
    /// rather than a `stat` per event.
    fn synced(&self) -> bool {
        if self.latched.load(Ordering::Relaxed) {
            return true;
        }
        if self.marker.exists() {
            self.latched.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }
}

/// A clock tests drive directly, so the unsynced window is a unit test rather
/// than a reboot. The two clocks move independently on purpose: that is the
/// whole condition being reproduced.
pub struct TestClock {
    wall_ms: AtomicI64,
    mono_ms: AtomicI64,
    synced: AtomicBool,
}

impl TestClock {
    /// A clock reading `wall_ms`, monotonic at zero, not yet synced.
    pub fn new(wall_ms: i64) -> Self {
        Self {
            wall_ms: AtomicI64::new(wall_ms),
            mono_ms: AtomicI64::new(0),
            synced: AtomicBool::new(false),
        }
    }

    /// Move the wall clock to `wall_ms`, leaving the monotonic clock alone.
    /// This is the NTP jump.
    pub fn set_wall_ms(&self, wall_ms: i64) {
        self.wall_ms.store(wall_ms, Ordering::Relaxed);
    }

    /// Advance both clocks by `ms`. This is time simply passing.
    pub fn advance(&self, ms: i64) {
        self.wall_ms.fetch_add(ms, Ordering::Relaxed);
        self.mono_ms.fetch_add(ms, Ordering::Relaxed);
    }

    /// Declare the wall clock trustworthy, or not.
    pub fn set_synced(&self, synced: bool) {
        self.synced.store(synced, Ordering::Relaxed);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.wall_ms.load(Ordering::Relaxed)
    }

    fn mono_ms(&self) -> i64 {
        self.mono_ms.load(Ordering::Relaxed)
    }

    fn synced(&self) -> bool {
        self.synced.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_marker_means_the_clock_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let clock = SystemClock::with_marker(&dir.path().join("never-created"));
        assert!(!clock.synced());
    }

    #[test]
    fn the_marker_appearing_makes_the_clock_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("synchronized");
        let clock = SystemClock::with_marker(&marker);

        assert!(!clock.synced());
        std::fs::write(&marker, "").unwrap();
        assert!(clock.synced());
    }

    // A row written while synced stays a row written while synced, whatever
    // /run does afterwards.
    #[test]
    fn trust_latches_and_never_goes_back() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("synchronized");
        std::fs::write(&marker, "").unwrap();
        let clock = SystemClock::with_marker(&marker);

        assert!(clock.synced());
        std::fs::remove_file(&marker).unwrap();
        assert!(
            clock.synced(),
            "trust must not be revoked by a vanishing /run"
        );
    }

    #[test]
    fn the_system_clock_reads_a_plausible_wall_time() {
        let clock = SystemClock::new();
        // Later than 2026-01-01, which every boot is once timesyncd has
        // advanced the clock to build time, sync or no sync.
        assert!(clock.now_ms() > 1_767_225_600_000);
    }

    #[test]
    fn the_system_clock_is_monotonic() {
        let clock = SystemClock::new();
        let first = clock.mono_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = clock.mono_ms();
        assert!(second >= first);
        assert!(
            first >= 0,
            "the monotonic origin is construction, not the epoch"
        );
    }

    // The bug, in miniature: the wall clock leaps thirteen minutes and every
    // duration measured against it is suddenly wrong. The monotonic one does
    // not move.
    #[test]
    fn a_wall_clock_jump_does_not_move_the_monotonic_clock() {
        let clock = TestClock::new(1_787_161_000_000);
        clock.advance(9_200);

        let mono_before = clock.mono_ms();
        let wall_before = clock.now_ms();

        clock.set_wall_ms(wall_before + 13 * 60 * 1000 + 28_000);

        assert_eq!(clock.mono_ms(), mono_before, "monotonic must not jump");
        assert_eq!(
            clock.now_ms() - wall_before,
            808_000,
            "the wall clock did jump"
        );
    }

    #[test]
    fn advancing_moves_both_clocks_together() {
        let clock = TestClock::new(1_000_000);
        clock.advance(250);
        assert_eq!(clock.now_ms(), 1_000_250);
        assert_eq!(clock.mono_ms(), 250);
    }

    #[test]
    fn a_test_clock_starts_untrusted_and_can_be_told_otherwise() {
        let clock = TestClock::new(1_000_000);
        assert!(!clock.synced());
        clock.set_synced(true);
        assert!(clock.synced());
    }
}
