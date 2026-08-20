//! Starting, stopping, and reaping the apps SESH launches.
//!
//! Exactly one app runs at a time: launching while something is running
//! quits it first. The compositor stacks the new window over the SESH
//! kiosk, and killing it reveals SESH again, so there is no focus
//! management to do here.

pub mod platform;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};

use crate::config::AppSpec;
use crate::event::{kind, NewEvent};
use crate::room::Room;
use platform::{Pid, Platform};

/// How often the reaper checks whether the current app is still alive.
const REAP_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
struct Running {
    app_id: String,
    pid: Pid,
}

/// Runs at most one app at a time and keeps the log in sync with reality.
pub struct Launcher {
    apps: Vec<AppSpec>,
    platform: Arc<dyn Platform>,
    room: Arc<Room>,
    current: Mutex<Option<Running>>,
}

impl Launcher {
    /// Build a launcher over an app registry.
    pub fn new(apps: Vec<AppSpec>, platform: Arc<dyn Platform>, room: Arc<Room>) -> Arc<Self> {
        Arc::new(Self {
            apps,
            platform,
            room,
            current: Mutex::new(None),
        })
    }

    /// Every launchable app, in registry order.
    pub fn apps(&self) -> &[AppSpec] {
        &self.apps
    }

    /// The id of the app currently running, if any.
    pub fn current(&self) -> Option<String> {
        self.current
            .lock()
            .expect("current mutex poisoned")
            .as_ref()
            .map(|r| r.app_id.clone())
    }

    /// The pid of the app currently running, if any. Used by tests; the
    /// reaper reads `current` directly.
    pub fn current_pid(&self) -> Option<Pid> {
        self.current
            .lock()
            .expect("current mutex poisoned")
            .as_ref()
            .map(|r| r.pid)
    }

    /// Quit whatever is running, then start `id`.
    pub fn launch(&self, id: &str) -> Result<()> {
        let spec = self
            .apps
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| anyhow!("no such app: {id}"))?
            .clone();

        // The guard is held across the whole launch — quit, spawn, record,
        // and the final assignment. Releasing it between the quit and the
        // assignment let two overlapping launches both see nothing running,
        // both spawn, and the second overwrite the first's `Running`. That
        // process was then invisible to `quit` and `reap` alike: it stayed on
        // screen over SESH until someone SSHed into the Pi.
        let mut current = self.current.lock().expect("current mutex poisoned");
        // `std::sync::Mutex` is not reentrant, so this must not be `self.quit()`.
        self.quit_locked(&mut current)?;

        let pid = self.platform.spawn(&spec.command, &spec.args)?;
        // The log write is the commit point. If it fails, undo the spawn rather
        // than leaving a running process SESH has no record of and cannot kill.
        if let Err(error) = self
            .room
            .record(NewEvent::new(kind::APP_LAUNCHED).subject(&spec.id))
        {
            let _ = self.platform.kill(pid);
            return Err(error);
        }
        *current = Some(Running {
            app_id: spec.id.clone(),
            pid,
        });
        Ok(())
    }

    /// Stop the running app, if any.
    pub fn quit(&self) -> Result<()> {
        let mut current = self.current.lock().expect("current mutex poisoned");
        self.quit_locked(&mut current)
    }

    /// Quit whatever `current` holds. The caller owns the `current` guard, so
    /// `launch` can quit without releasing it mid-flight.
    fn quit_locked(&self, current: &mut Option<Running>) -> Result<()> {
        let Some(running) = current.as_ref() else {
            return Ok(());
        };
        let (pid, app_id) = (running.pid, running.app_id.clone());
        self.platform.kill(pid)?;
        self.room
            .record(NewEvent::new(kind::APP_EXITED).subject(&app_id))?;
        *current = None;
        Ok(())
    }

    /// Notice an app that exited on its own — the user quit it from inside
    /// itself, or it crashed — and record the exit.
    pub fn reap(&self) -> Result<()> {
        let mut current = self.current.lock().expect("current mutex poisoned");
        let Some(running) = current.as_ref() else {
            return Ok(());
        };
        if self.platform.is_running(running.pid) {
            return Ok(());
        }
        let app_id = running.app_id.clone();
        // Record before forgetting: if the log write fails, `current` stays set
        // and the next reap retries, rather than silently losing the exit.
        self.room
            .record(NewEvent::new(kind::APP_EXITED).subject(&app_id))?;
        *current = None;
        Ok(())
    }

    /// Reap forever. Spawned as a background task by `main`.
    ///
    /// `reap` takes the `current` mutex and, when an app has exited, writes to
    /// the log behind it — so it runs on a blocking thread rather than on the
    /// runtime worker this loop is scheduled on. The lock is shared with
    /// `launch` and `quit`, which means a reap that arrives mid-launch waits
    /// out the SIGTERM grace period before it can look.
    pub async fn reap_loop(launcher: Arc<Self>) {
        loop {
            tokio::time::sleep(REAP_INTERVAL).await;
            let each = launcher.clone();
            match tokio::task::spawn_blocking(move || each.reap()).await {
                Ok(Err(error)) => tracing::warn!(%error, "reaper failed"),
                Err(join) => tracing::warn!(%join, "reaper task failed to run"),
                Ok(Ok(())) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use platform::MockPlatform;

    fn apps() -> Vec<AppSpec> {
        vec![
            AppSpec {
                id: "kodi".into(),
                name: "Kodi".into(),
                command: "kodi".into(),
                args: vec!["--standalone".into()],
                icon: "movie".into(),
            },
            AppSpec {
                id: "retroarch".into(),
                name: "RetroArch".into(),
                command: "retroarch".into(),
                args: vec![],
                icon: "gamepad".into(),
            },
        ]
    }

    fn fixture() -> (Arc<Launcher>, Arc<MockPlatform>, Arc<Room>) {
        let apps = apps();
        let platform = Arc::new(MockPlatform::new());
        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let launcher = Launcher::new(apps, platform.clone(), room.clone());
        (launcher, platform, room)
    }

    fn kinds(room: &Room) -> Vec<String> {
        room.events_since(0, -1)
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    fn subjects(room: &Room) -> Vec<Option<String>> {
        room.events_since(0, -1)
            .unwrap()
            .into_iter()
            .map(|e| e.subject)
            .collect()
    }

    #[test]
    fn nothing_is_running_initially() {
        let (launcher, _, _) = fixture();
        assert_eq!(launcher.current(), None);
    }

    #[test]
    fn launching_starts_the_configured_command_with_its_args() {
        let (launcher, platform, _) = fixture();
        launcher.launch("kodi").unwrap();

        assert_eq!(
            platform.spawned(),
            vec![("kodi".to_string(), vec!["--standalone".to_string()])]
        );
        assert_eq!(launcher.current(), Some("kodi".to_string()));
    }

    #[test]
    fn launching_records_an_app_launched_event() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();

        assert_eq!(kinds(&room), vec![kind::APP_LAUNCHED.to_string()]);
        assert_eq!(subjects(&room), vec![Some("kodi".to_string())]);
    }

    #[test]
    fn launching_an_unknown_app_is_an_error_and_records_nothing() {
        let (launcher, _, room) = fixture();
        let err = launcher.launch("nintendo64").unwrap_err();

        assert!(
            err.to_string().contains("nintendo64"),
            "error should name the id: {err}"
        );
        assert!(kinds(&room).is_empty());
        assert_eq!(launcher.current(), None);
    }

    #[test]
    fn launching_an_unknown_app_while_running_leaves_the_running_app_untouched() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();

        let err = launcher.launch("nintendo64").unwrap_err();

        assert!(
            err.to_string().contains("nintendo64"),
            "error should name the id: {err}"
        );
        assert_eq!(launcher.current(), Some("kodi".to_string()));
        assert_eq!(kinds(&room), vec![kind::APP_LAUNCHED.to_string()]);
    }

    #[test]
    fn launching_while_running_quits_the_previous_app_first() {
        let (launcher, platform, room) = fixture();
        launcher.launch("kodi").unwrap();
        launcher.launch("retroarch").unwrap();

        assert_eq!(launcher.current(), Some("retroarch".to_string()));
        assert_eq!(
            kinds(&room),
            vec![
                kind::APP_LAUNCHED.to_string(),
                kind::APP_EXITED.to_string(),
                kind::APP_LAUNCHED.to_string(),
            ]
        );
        assert_eq!(platform.spawned().len(), 2);
    }

    #[test]
    fn quitting_stops_the_app_and_records_an_exit() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();
        launcher.quit().unwrap();

        assert_eq!(launcher.current(), None);
        assert_eq!(
            kinds(&room),
            vec![kind::APP_LAUNCHED.to_string(), kind::APP_EXITED.to_string()]
        );
    }

    #[test]
    fn quitting_with_nothing_running_is_a_no_op() {
        let (launcher, _, room) = fixture();
        launcher.quit().unwrap();

        assert!(kinds(&room).is_empty());
        assert_eq!(launcher.current(), None);
    }

    #[test]
    fn quitting_when_kill_fails_leaves_state_and_the_log_untouched() {
        let (launcher, platform, room) = fixture();
        launcher.launch("kodi").unwrap();

        platform.fail_next_kill();
        let err = launcher.quit().unwrap_err();

        assert!(err.to_string().contains("simulated kill failure"));
        assert_eq!(
            launcher.current(),
            Some("kodi".to_string()),
            "a failed kill must not make the Launcher forget the app it \
             couldn't actually stop"
        );
        assert_eq!(
            kinds(&room),
            vec![kind::APP_LAUNCHED.to_string()],
            "no app.exited should be recorded when the app was never confirmed stopped"
        );
    }

    #[test]
    fn reaping_notices_an_app_the_user_quit_from_inside_itself() {
        let (launcher, platform, room) = fixture();
        launcher.launch("kodi").unwrap();

        // The user picked Kodi's own Quit menu item. SESH did not do this.
        let pid = launcher.current_pid().unwrap();
        platform.simulate_exit(pid);

        launcher.reap().unwrap();

        assert_eq!(launcher.current(), None);
        assert_eq!(
            kinds(&room),
            vec![kind::APP_LAUNCHED.to_string(), kind::APP_EXITED.to_string()]
        );
    }

    #[test]
    fn reaping_a_still_running_app_changes_nothing() {
        let (launcher, _, room) = fixture();
        launcher.launch("kodi").unwrap();
        launcher.reap().unwrap();

        assert_eq!(launcher.current(), Some("kodi".to_string()));
        assert_eq!(kinds(&room), vec![kind::APP_LAUNCHED.to_string()]);
    }

    #[test]
    fn reaping_with_nothing_running_is_a_no_op() {
        let (launcher, _, room) = fixture();
        launcher.reap().unwrap();
        assert!(kinds(&room).is_empty());
    }

    #[test]
    fn apps_are_exposed_in_registry_order() {
        let (launcher, _, _) = fixture();
        let ids: Vec<_> = launcher.apps().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["kodi", "retroarch"]);
    }

    /// A platform whose spawn is slow enough that two overlapping launches
    /// would certainly interleave if `launch` released the `current` lock
    /// partway through.
    struct SlowSpawn(Arc<MockPlatform>);

    impl Platform for SlowSpawn {
        fn spawn(&self, program: &str, args: &[String]) -> Result<Pid> {
            std::thread::sleep(Duration::from_millis(50));
            self.0.spawn(program, args)
        }
        fn kill(&self, pid: Pid) -> Result<()> {
            self.0.kill(pid)
        }
        fn is_running(&self, pid: Pid) -> bool {
            self.0.is_running(pid)
        }
    }

    #[test]
    fn concurrent_launches_never_orphan_a_process() {
        let inner = Arc::new(MockPlatform::new());
        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
        let launcher = Launcher::new(apps(), Arc::new(SlowSpawn(inner.clone())), room);

        let handles: Vec<_> = ["kodi", "retroarch"]
            .into_iter()
            .map(|id| {
                let launcher = launcher.clone();
                std::thread::spawn(move || launcher.launch(id))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        // Both launches ran, so both spawned — but only one process may still
        // be alive afterwards, and SESH must be tracking exactly that one.
        // Before `launch` held the guard end to end, the losing launch's
        // process stayed alive with nothing pointing at it.
        assert_eq!(inner.spawned().len(), 2);
        let alive = inner.running_pids();
        assert_eq!(alive.len(), 1, "exactly one app may be running: {alive:?}");
        assert_eq!(
            launcher.current_pid(),
            Some(alive[0]),
            "the surviving process must be the one the launcher tracks"
        );
    }
}
