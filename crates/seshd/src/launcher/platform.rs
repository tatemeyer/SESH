//! Process control, behind a trait so the launcher is testable off-Pi.

use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
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
        let mut command = Command::new(program);
        command.args(args);
        // Put the app in its own process group, so `pid` is a handle on
        // everything it starts rather than only the process SESH spawned.
        // Debian's `/usr/bin/kodi` is a shell wrapper: if it forks the real
        // binary instead of `exec`ing it, the spawned pid dies at once while
        // Kodi stays on the TV, and the group is the only remaining handle.
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn()?;
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
        let Some(child) = children.get_mut(&pid) else {
            return false;
        };
        // `try_wait` reaps as well as reports. That matters below: an unreaped
        // zombie is still a member of its process group, so skipping this
        // would leave the group looking alive forever.
        if matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        // The process SESH spawned is gone, which is not the same as the app
        // being gone — a wrapper may have forked the real binary and exited.
        // Anything still in the group is still on the TV.
        if group_alive(pid) {
            return true;
        }
        // A process that exited on its own (the "quit Kodi from its own
        // menu" case) never has kill() called on it, so this is the
        // only place a dead child's entry — and on Windows its open
        // process HANDLE — ever gets reclaimed.
        children.remove(&pid);
        false
    }
}

/// How long a child gets to shut itself down after SIGTERM.
#[cfg(unix)]
const GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// How often the grace period checks whether the child has gone.
#[cfg(unix)]
const GRACE_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Ask an app's whole process group to exit cleanly, falling back to SIGKILL
/// after `GRACE`.
///
/// `Child::kill` is SIGKILL with no SIGTERM first, which skips the app's
/// shutdown path entirely: RetroArch never writes SRAM and Kodi never saves
/// playback position, so pressing B mid-game loses the save. Shelling out to
/// `kill(1)` is deliberate — `libc` would be a whole new dependency for one
/// syscall, and `kill` is present on every Linux the Pi image ships. Do not
/// "simplify" this back to `child.kill()`.
///
/// The group, not the child, is the target: a wrapper that forked the real
/// binary is already gone by the time SESH wants to stop it, and signalling
/// the dead wrapper would leave the app running with nothing pointing at it.
#[cfg(unix)]
fn terminate(child: &mut Child, pid: Pid) {
    signal_group(pid, "TERM");

    let deadline = std::time::Instant::now() + GRACE;
    while std::time::Instant::now() < deadline {
        // Reap the spawned process as soon as it goes, so its own zombie does
        // not hold the group open for the whole grace period.
        let child_gone = !matches!(child.try_wait(), Ok(None));
        if child_gone && !group_alive(pid) {
            return;
        }
        std::thread::sleep(GRACE_POLL);
    }

    signal_group(pid, "KILL");
    let _ = child.kill();
}

/// Send a signal to every process in `pid`'s group. A negative pid means
/// "the process group with this id", which is why `spawn` makes each app a
/// group leader — its pid and its group id are the same number.
///
/// The `--` is load-bearing, not decoration. This runs the external
/// `kill(1)` — util-linux's, on Raspberry Pi OS — not a shell builtin, and
/// without `--` it parses the leading `-` of `-1234` as an option, signals
/// nothing at all, and still exits 0. That combination is silent: the app
/// stays on the TV and every status check says the signal was sent.
///
/// The exit status is discarded for the same reason — it is 0 whether the
/// group died, never existed, or was never signalled. `group_alive` reads
/// `/proc` to find out what actually happened.
#[cfg(unix)]
fn signal_group(pid: Pid, signal: &str) {
    let _ = Command::new("kill")
        .args([format!("-{signal}"), "--".to_string(), format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Whether any live process is left in the app's process group.
///
/// Zombies do not count: a process waiting to be reaped is already off the TV.
///
/// `/proc` is read directly because `kill -0` cannot answer this — see
/// `signal_group`. This is only meaningful while SESH still holds the group
/// leader's entry: once a pid is released the kernel may reuse it, so a stale
/// pid could in principle name an unrelated group. `is_running` and
/// `terminate` both consult it only for a pid they are still tracking. On a
/// unix without `/proc` the scan finds nothing and reports "empty", degrading
/// to the previous behaviour of tracking only the spawned child — the deploy
/// target is Linux.
#[cfg(unix)]
fn group_alive(pgid: Pid) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            return false;
        };
        // `stat` is "pid (comm) state ppid pgrp ...". `comm` is parenthesised
        // and may itself contain spaces and brackets, so the fields after it
        // are found by scanning back from the last ')' rather than splitting
        // from the left.
        let Some(after_comm) = stat.rfind(')').map(|end| &stat[end + 1..]) else {
            return false;
        };
        let mut fields = after_comm.split_whitespace();
        let state = fields.next();
        let _ppid = fields.next();
        let pgrp = fields.next().and_then(|field| field.parse::<Pid>().ok());
        pgrp == Some(pgid) && state != Some("Z")
    })
}

/// Windows has no process group with these semantics, and it is the dev
/// machine rather than the deploy target, so the spawned child is the whole
/// story there.
#[cfg(not(unix))]
fn group_alive(_pid: Pid) -> bool {
    false
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

    /// Debian's `/usr/bin/kodi` is a shell wrapper. If it forks the real
    /// binary and exits rather than `exec`ing it, the pid SESH tracks dies
    /// immediately while the app is still on the TV. SESH then records a false
    /// `app.exited`, clears `current`, and can no longer stop what is on
    /// screen — the worst failure in the runbook, because the room is stuck
    /// until someone SSHes in.
    ///
    /// Tracking the process *group* rather than the bare child is what makes
    /// both halves survive that: "is it still running" and "stop it".
    #[cfg(unix)]
    #[test]
    fn a_wrapper_that_forks_and_exits_is_still_running_and_still_killable() {
        let platform = ProcessPlatform::new();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("app.pid");
        let terminated = dir.path().join("terminated");

        // The outer `sh` stands in for the wrapper: it backgrounds the real
        // app and exits straight away. The inner one stands in for the app,
        // and records both its pid and the fact that it was asked to stop, so
        // the test can tell "SESH reached it" from "SESH lost track of it".
        // The stand-in app gives up on its own after a few seconds and drops
        // the inherited stdio. Without both, a failing run would leak a
        // process that outlives the test and holds the harness's output pipe
        // open — the suite would hang instead of reporting a failure.
        let script = format!(
            "sh -c 'trap \"touch {}; exit 0\" TERM; echo $$ > {}; \
             for _ in $(seq 200); do sleep 0.05; done' >/dev/null 2>&1 & exit 0",
            terminated.display(),
            pidfile.display()
        );
        let pid = platform.spawn("sh", &["-c".to_string(), script]).unwrap();

        let app_pid = read_pid_when_written(&pidfile);
        assert_ne!(
            app_pid, pid,
            "the app should be a different pid to the wrapper"
        );
        // Let the wrapper reach its own `exit 0`.
        std::thread::sleep(std::time::Duration::from_millis(300));

        assert!(
            platform.is_running(pid),
            "the wrapper exited but the app it forked is still alive — SESH \
             must not report this app as gone"
        );

        platform.kill(pid).unwrap();

        // The app itself is the witness here. Checking /proc for the pid would
        // not do: by now the app is an orphan, so it is reaped asynchronously
        // by whoever adopted it, and a not-yet-reaped zombie still has a
        // /proc entry. The marker only exists if the signal genuinely landed
        // on the process the wrapper forked.
        assert!(
            terminated.exists(),
            "killing the app must reach the process the wrapper forked, not \
             just the wrapper SESH happened to spawn"
        );
        assert!(!platform.is_running(pid));
    }

    /// Poll until the forked app has written its pid, so the test never races
    /// the shell. Panics rather than hanging if it never appears.
    #[cfg(unix)]
    fn read_pid_when_written(pidfile: &std::path::Path) -> Pid {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(pidfile) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!(
            "the forked app never wrote its pid to {}",
            pidfile.display()
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
