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
            let _ = child.kill();
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

/// An in-memory platform for tests. Records every spawn and lets a test
/// simulate an app the user quit from inside itself.
#[derive(Default)]
pub struct MockPlatform {
    next_pid: Mutex<Pid>,
    running: Mutex<HashSet<Pid>>,
    spawned: Mutex<Vec<(String, Vec<String>)>>,
    fail_next_kill: Mutex<bool>,
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

    /// Mark a process as having exited on its own, without SESH killing it.
    pub fn simulate_exit(&self, pid: Pid) {
        self.running
            .lock()
            .expect("running mutex poisoned")
            .remove(&pid);
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
    fn process_platform_errors_on_a_missing_program() {
        let platform = ProcessPlatform::new();
        assert!(platform
            .spawn("definitely-not-a-real-program-xyz", &[])
            .is_err());
    }
}
