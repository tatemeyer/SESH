//! Process control, behind a trait so the launcher is testable off-Pi.

use std::collections::{HashMap, HashSet};
use std::process::{Child, Command};
use std::sync::Mutex;

use anyhow::{anyhow, Result};

/// A process handle. For `ProcessPlatform` this is the OS pid.
pub type Pid = u32;

/// Everything SESH needs from the operating system to run an app.
pub trait Platform: Send + Sync + 'static {
    /// Start a program and return a handle to it.
    fn spawn(&self, program: &str, args: &[String]) -> Result<Pid>;

    /// Stop a process. Stopping an already-dead process is not an error.
    fn kill(&self, pid: Pid) -> Result<()>;

    /// Whether the process is still alive.
    fn is_running(&self, pid: Pid) -> bool;
}

/// The real implementation: spawns child processes of `seshd`.
///
/// Because `seshd` runs inside the compositor's user session, children
/// inherit `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` and appear on the TV.
#[derive(Default)]
pub struct ProcessPlatform {
    children: Mutex<HashMap<Pid, Child>>,
}

impl ProcessPlatform {
    /// Create a platform backed by real OS processes.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Platform for ProcessPlatform {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Pid> {
        let child = Command::new(program).args(args).spawn()?;
        let pid = child.id();
        self.children
            .lock()
            .expect("children mutex poisoned")
            .insert(pid, child);
        Ok(pid)
    }

    fn kill(&self, pid: Pid) -> Result<()> {
        let mut children = self.children.lock().expect("children mutex poisoned");
        if let Some(child) = children.get_mut(&pid) {
            // An already-exited child is the normal case when the user quit
            // the app themselves, so a failed kill is not an error.
            terminate(child, pid);
            let _ = child.wait();
            children.remove(&pid);
        }
        Ok(())
    }

    fn is_running(&self, pid: Pid) -> bool {
        let mut children = self.children.lock().expect("children mutex poisoned");
        let running = matches!(children.get_mut(&pid).map(|c| c.try_wait()), Some(Ok(None)));
        if !running {
            // A process that exited on its own (the "quit Kodi from its own
            // menu" case) never has kill() called on it, so this is the
            // only place a dead child's entry — and on Windows its open
            // process HANDLE — ever gets reclaimed.
            children.remove(&pid);
        }
        running
    }
}

/// How long a child gets to shut itself down after SIGTERM.
#[cfg(unix)]
const GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the grace period checks whether the child has gone.
#[cfg(unix)]
const GRACE_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Ask a child to exit cleanly, falling back to SIGKILL after `GRACE`.
///
/// `Child::kill` is SIGKILL with no SIGTERM first, which skips the app's
/// shutdown path entirely: RetroArch never writes SRAM and Kodi never saves
/// playback position, so pressing B mid-game loses the save. Shelling out to
/// `kill(1)` is deliberate — `libc` would be a whole new dependency for one
/// syscall, and `kill` is present on every Linux the Pi image ships. Do not
/// "simplify" this back to `child.kill()`.
#[cfg(unix)]
fn terminate(child: &mut Child, pid: Pid) {
    let sent = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();

    if sent {
        let deadline = std::time::Instant::now() + GRACE;
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(GRACE_POLL),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
}

/// Windows has no SIGTERM, and it is the dev machine rather than the deploy
/// target, so it keeps the abrupt kill.
#[cfg(not(unix))]
fn terminate(child: &mut Child, _pid: Pid) {
    let _ = child.kill();
}

/// An in-memory platform for tests. Records every spawn and lets a test
/// simulate an app the user quit from inside itself.
#[derive(Default)]
pub struct MockPlatform {
    next_pid: Mutex<Pid>,
    running: Mutex<HashSet<Pid>>,
    spawned: Mutex<Vec<(String, Vec<String>)>>,
    fail_next_kill: Mutex<bool>,
    fail_next_spawn: Mutex<bool>,
}

impl MockPlatform {
    /// Create an empty mock platform.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `(program, args)` pair spawned so far, in order.
    pub fn spawned(&self) -> Vec<(String, Vec<String>)> {
        self.spawned.lock().expect("spawned mutex poisoned").clone()
    }

    /// Every pid still alive, ascending. Lets a test check that SESH left
    /// nothing running behind its own back.
    pub fn running_pids(&self) -> Vec<Pid> {
        let mut pids: Vec<Pid> = self
            .running
            .lock()
            .expect("running mutex poisoned")
            .iter()
            .copied()
            .collect();
        pids.sort_unstable();
        pids
    }

    /// Mark a process as having exited on its own, without SESH killing it.
    pub fn simulate_exit(&self, pid: Pid) {
        self.running
            .lock()
            .expect("running mutex poisoned")
            .remove(&pid);
    }

    /// Make the next spawn fail, the way a missing binary or an exhausted
    /// process table does.
    ///
    /// Added by the #48 audit: this double could fail a `kill` but never a
    /// `spawn`, so `Launcher::launch`'s error path — and the 500 the API
    /// returns from it — had never once been executed.
    pub fn fail_next_spawn(&self) {
        *self
            .fail_next_spawn
            .lock()
            .expect("fail_next_spawn mutex poisoned") = true;
    }

    /// Make the next call to `kill` return an error instead of succeeding.
    /// The flag is one-shot: it resets after the next `kill` call, whether
    /// or not that call was actually reached.
    pub fn fail_next_kill(&self) {
        *self
            .fail_next_kill
            .lock()
            .expect("fail_next_kill mutex poisoned") = true;
    }
}

impl Platform for MockPlatform {
    fn spawn(&self, program: &str, args: &[String]) -> Result<Pid> {
        if program.is_empty() {
            return Err(anyhow!("empty program name"));
        }
        // The realistic failure: the binary is not there, or the process table
        // is full. An empty program name was the only way to make this fail
        // before, and the app registry cannot produce one — so the path was
        // unreachable from any test that went through the launcher.
        let mut fail_next = self
            .fail_next_spawn
            .lock()
            .expect("fail_next_spawn mutex poisoned");
        if *fail_next {
            *fail_next = false;
            return Err(anyhow!("simulated spawn failure"));
        }
        drop(fail_next);
        let mut next = self.next_pid.lock().expect("next_pid mutex poisoned");
        *next += 1;
        let pid = *next;
        self.running
            .lock()
            .expect("running mutex poisoned")
            .insert(pid);
        self.spawned
            .lock()
            .expect("spawned mutex poisoned")
            .push((program.to_string(), args.to_vec()));
        Ok(pid)
    }

    fn kill(&self, pid: Pid) -> Result<()> {
        let mut fail_next = self
            .fail_next_kill
            .lock()
            .expect("fail_next_kill mutex poisoned");
        if *fail_next {
            *fail_next = false;
            return Err(anyhow!("simulated kill failure"));
        }
        drop(fail_next);

        self.running
            .lock()
            .expect("running mutex poisoned")
            .remove(&pid);
        Ok(())
    }

    fn is_running(&self, pid: Pid) -> bool {
        self.running
            .lock()
            .expect("running mutex poisoned")
            .contains(&pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_records_what_was_spawned() {
        let platform = MockPlatform::new();
        platform
            .spawn("kodi", &["--fullscreen".to_string()])
            .unwrap();

        assert_eq!(
            platform.spawned(),
            vec![("kodi".to_string(), vec!["--fullscreen".to_string()])]
        );
    }

    #[test]
    fn mock_reports_spawned_processes_as_running() {
        let platform = MockPlatform::new();
        let pid = platform.spawn("retroarch", &[]).unwrap();
        assert!(platform.is_running(pid));
    }

    #[test]
    fn mock_kill_stops_a_process() {
        let platform = MockPlatform::new();
        let pid = platform.spawn("retroarch", &[]).unwrap();
        platform.kill(pid).unwrap();
        assert!(!platform.is_running(pid));
    }

    #[test]
    fn mock_can_simulate_a_process_exiting_on_its_own() {
        let platform = MockPlatform::new();
        let pid = platform.spawn("kodi", &[]).unwrap();
        platform.simulate_exit(pid);
        assert!(!platform.is_running(pid));
    }

    #[test]
    fn mock_pids_are_unique() {
        let platform = MockPlatform::new();
        let a = platform.spawn("a", &[]).unwrap();
        let b = platform.spawn("b", &[]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn process_platform_spawns_and_kills_a_real_process() {
        let platform = ProcessPlatform::new();

        // A long-running process that needs no console and no stdin.
        // `timeout` is not usable here: it fails without a real console.
        #[cfg(windows)]
        let (program, args) = (
            "ping",
            vec!["-n".to_string(), "60".to_string(), "127.0.0.1".to_string()],
        );
        #[cfg(unix)]
        let (program, args) = ("sleep", vec!["60".to_string()]);

        let pid = platform.spawn(program, &args).unwrap();
        assert!(platform.is_running(pid));

        platform.kill(pid).unwrap();
        assert!(!platform.is_running(pid));

        // Nothing here distinguishes SIGTERM-then-exit from SIGKILL: on
        // Windows `terminate` really is just `child.kill()`, and on Unix the
        // signal a reaped child received is not recoverable through `Child`.
        // The graceful path is covered by the `#[cfg(unix)]` test below.
        //
        // `is_running` only consults this platform's own bookkeeping, so it
        // can't tell "actually killed" from "bookkeeping merely dropped."
        // On the deploy target (Linux) we can check OS truth directly via
        // /proc, with no new dependency and no subprocess: once `wait()`
        // reaps a process, its /proc/<pid> entry disappears. This doesn't
        // run on the Windows dev machine, since Windows has no /proc.
        #[cfg(unix)]
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn killing_a_child_that_already_exited_is_still_ok_and_fast() {
        let platform = ProcessPlatform::new();

        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/c".to_string(), "exit".to_string()]);
        #[cfg(unix)]
        let (program, args) = ("true", vec![]);

        let pid = platform.spawn(program, &args).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));

        // The "user quit the app from inside it" case. The graceful path must
        // notice the child is already gone rather than burning the full grace
        // period on a process that cannot answer.
        let started = std::time::Instant::now();
        platform.kill(pid).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "killing a dead child should not wait out the grace period"
        );
        assert!(!platform.is_running(pid));
    }

    /// Covers the SIGTERM-first path on the deploy target. RetroArch writes
    /// SRAM from its SIGTERM handler; SIGKILL would lose it.
    #[cfg(unix)]
    #[test]
    fn kill_lets_a_unix_child_run_its_shutdown_path() {
        let platform = ProcessPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("terminated");

        let script = format!(
            "trap 'touch {}; exit 0' TERM; while true; do sleep 0.05; done",
            marker.display()
        );
        let pid = platform.spawn("sh", &["-c".to_string(), script]).unwrap();
        // Let the shell install its trap before signalling it.
        std::thread::sleep(std::time::Duration::from_millis(300));

        platform.kill(pid).unwrap();

        assert!(
            marker.exists(),
            "child should have run its SIGTERM handler before dying"
        );
    }

    #[test]
    fn process_platform_errors_on_a_missing_program() {
        let platform = ProcessPlatform::new();
        assert!(platform
            .spawn("definitely-not-a-real-program-xyz", &[])
            .is_err());
    }
}
