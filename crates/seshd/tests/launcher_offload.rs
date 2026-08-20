//! Launching and quitting must not block the async runtime.
//!
//! `Launcher::launch` and `Launcher::quit` are synchronous: they spawn or kill
//! a process and take a `std::sync::Mutex` across it. Calling them straight
//! from an `async` handler parks a Tokio worker for the whole duration, and
//! `quit` in particular waits out a SIGTERM grace period measured in seconds.
//!
//! With four workers on a Pi and one viewer that was invisible, which is why
//! it survived Arc 1 as follow-up item 1 rather than as a bug. These tests pin
//! the property directly instead: on a single-threaded runtime, nothing else
//! can make progress while a worker is parked, so a timer task that keeps
//! ticking during a slow launch proves the blocking work left the worker.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use seshd::api::apps::{launch_app, quit_app};
use seshd::api::AppState;
use seshd::config::AppSpec;
use seshd::join::JoinCodes;
use seshd::launcher::platform::{MockPlatform, Pid, Platform};
use seshd::launcher::Launcher;
use seshd::presence::Presence;
use seshd::room::Room;
use seshd::store::Store;

/// How long the fake platform stalls inside `spawn`/`kill`.
const STALL: Duration = Duration::from_millis(600);

/// A platform whose process control blocks for [`STALL`], standing in for a
/// real `spawn` on a loaded Pi and for `quit`'s SIGTERM grace period.
struct StallingPlatform(Arc<MockPlatform>);

impl Platform for StallingPlatform {
    fn spawn(&self, program: &str, args: &[String]) -> anyhow::Result<Pid> {
        std::thread::sleep(STALL);
        self.0.spawn(program, args)
    }
    fn kill(&self, pid: Pid) -> anyhow::Result<()> {
        std::thread::sleep(STALL);
        self.0.kill(pid)
    }
    fn is_running(&self, pid: Pid) -> bool {
        self.0.is_running(pid)
    }
}

fn state() -> AppState {
    let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
    let launcher = Launcher::new(
        vec![AppSpec {
            id: "kodi".into(),
            name: "Kodi".into(),
            command: "kodi".into(),
            args: vec![],
            icon: "movie".into(),
        }],
        Arc::new(StallingPlatform(Arc::new(MockPlatform::new()))),
        room.clone(),
    );
    AppState {
        room,
        launcher,
        join: Arc::new(JoinCodes::new()),
        presence: Arc::new(Presence::new()),
        player: None,
        music: Arc::new(seshd::conductor::Status::new()),
        clock: Arc::new(seshd::clock::SystemClock::new()),
        join_base: "http://pi.test:7373".into(),
    }
}

/// Ticks every 10ms on the current runtime until dropped, counting as it goes.
/// On a single-threaded runtime it can only advance while the worker is free.
fn ticker() -> Arc<AtomicUsize> {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = ticks.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(10)).await;
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    ticks
}

#[tokio::test]
async fn a_slow_launch_leaves_the_runtime_free() {
    let state = state();
    let ticks = ticker();
    // Let the ticker reach its first await before the launch starts.
    tokio::task::yield_now().await;

    launch_app(State(state), Path("kodi".to_string()))
        .await
        .expect("launch should succeed");

    let seen = ticks.load(Ordering::SeqCst);
    assert!(
        seen > 10,
        "the runtime made almost no progress during a {STALL:?} launch \
         ({seen} ticks): the blocking work is still on the async worker"
    );
}

#[tokio::test]
async fn a_slow_quit_leaves_the_runtime_free() {
    let state = state();
    launch_app(State(state.clone()), Path("kodi".to_string()))
        .await
        .expect("launch should succeed");

    let ticks = ticker();
    tokio::task::yield_now().await;

    quit_app(State(state)).await.expect("quit should succeed");

    let seen = ticks.load(Ordering::SeqCst);
    assert!(
        seen > 10,
        "the runtime made almost no progress during a {STALL:?} quit \
         ({seen} ticks): the blocking work is still on the async worker"
    );
}

/// The offload must not change what the endpoints actually do.
#[tokio::test]
async fn launching_an_unknown_app_is_still_a_404() {
    let status = launch_app(State(state()), Path("nope".to_string()))
        .await
        .unwrap_err();
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}
