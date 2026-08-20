//! The LAN-facing HTTP and WebSocket API.
//!
//! Mostly unauthenticated, deliberately. Arc 2 adds per-person tokens only
//! where an action needs an actor attached to it; reads stay open, and
//! `POST /api/events` stays the open ingest port the invariants describe.

pub mod apps;
pub mod auth;
pub mod events;
pub mod join;
pub mod music;
pub mod ws;

use std::sync::Arc;

use std::path::Path;

use axum::routing::{get, post};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

use crate::clock::Clock;
use crate::conductor::Status;
use crate::join::JoinCodes;
use crate::launcher::Launcher;
use crate::player::Player;
use crate::presence::Presence;
use crate::room::Room;

/// Everything the API handlers need. Cheap to clone — the fields are `Arc`s
/// and one short string.
#[derive(Clone)]
pub struct AppState {
    /// The event log and its projections.
    pub room: Arc<Room>,
    /// The app launcher.
    pub launcher: Arc<Launcher>,
    /// The live join code shown on the TV.
    pub join: Arc<JoinCodes>,
    /// Who is in the room, per their phones.
    pub presence: Arc<Presence>,
    /// The music source, when one is configured.
    ///
    /// `None` on a box with no Spotify credentials. The room still launches
    /// apps and still keeps a queue; only search and playback go away, which
    /// is the degradation the vision asks for rather than a failure to boot.
    pub player: Option<Arc<dyn Player>>,
    /// Whether the music source is answering, as the conductor last saw it.
    pub music: Arc<Status>,
    /// Time, and whether it can be believed.
    ///
    /// Handlers measure elapsed time with [`Clock::mono_ms`], never the wall
    /// clock: a code's sixty-second life is a duration, and on a Pi with no RTC
    /// the wall clock steps sideways once per boot.
    pub clock: Arc<dyn Clock>,
    /// Origin the QR points phones at, e.g. `http://192.168.40.195:7373`.
    ///
    /// It cannot be derived from the request: the TV fetches the QR over
    /// `127.0.0.1`, and a QR encoding loopback is a QR no phone can use.
    pub join_base: String,
}

/// Build the API router. Static file serving is added by `main`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/events",
            get(events::list_events).post(events::post_event),
        )
        .route("/api/roster", get(events::get_roster))
        .route("/api/apps", get(apps::list_apps))
        .route("/api/apps/:id/launch", post(apps::launch_app))
        .route("/api/apps/quit", post(apps::quit_app))
        .route("/api/join/qr.svg", get(join::join_qr))
        .route("/api/join", post(join::join))
        .route("/api/me", get(join::me))
        .route("/api/heartbeat", post(join::heartbeat))
        .route("/api/music", get(music::get_music))
        .route("/api/music/queue", post(music::queue_track))
        .route("/api/music/veto", post(music::veto_track))
        .route("/api/music/search", get(music::search_tracks))
        .with_state(state)
}

/// Serve the built surface bundle, with unknown paths falling back to
/// `index.html` so the surface owns its own routing (D9).
///
/// `fallback` rather than `not_found_service`, and the difference is the whole
/// point: `not_found_service` serves the body but forces the status to **404**.
/// A browser renders that anyway, so `/join` and `/phone` would have limped
/// along looking fine while every status-code check — including the boot
/// verification harness — called the surface broken.
pub fn surface_service(static_dir: &Path) -> ServeDir<ServeFile> {
    let index = static_dir.join("index.html");
    ServeDir::new(static_dir).fallback(ServeFile::new(index))
}

/// The API router plus the live event feed. `router` alone is kept for
/// tests that use `oneshot`, which cannot perform a WebSocket upgrade.
pub fn router_with_ws(state: AppState) -> Router {
    Router::new()
        .route("/ws", get(ws::ws_handler))
        .with_state(state.clone())
        .merge(router(state))
}

/// Shared fixtures, so each endpoint's tests can live beside its handler
/// instead of piling up in this file.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::clock::TestClock;
    use crate::config::AppSpec;
    use crate::launcher::platform::MockPlatform;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::Request;

    /// A router over an in-memory room with one app registered.
    pub fn app() -> (Router, AppState) {
        let (router, state, _player) = app_with_music();
        (router, state)
    }

    /// The same fixture, keeping hold of the clock so a test can move it —
    /// including sideways, the way NTP does once per boot.
    pub fn app_with_clock() -> (Router, AppState, Arc<TestClock>) {
        let clock = Arc::new(TestClock::new(1_787_161_000_000));
        clock.set_synced(true);
        let (router, state, _player) = build(clock.clone());
        (router, state, clock)
    }

    /// The same fixture, keeping hold of the mock source so a test can
    /// script what a search returns.
    pub fn app_with_music() -> (Router, AppState, Arc<crate::player::mock::MockPlayer>) {
        let clock = Arc::new(TestClock::new(1_787_161_000_000));
        clock.set_synced(true);
        build(clock)
    }

    fn build(clock: Arc<TestClock>) -> (Router, AppState, Arc<crate::player::mock::MockPlayer>) {
        let room = Room::new(
            Store::open_in_memory()
                .unwrap()
                .with_clock(clock.clone() as Arc<dyn Clock>),
        )
        .unwrap();
        let launcher = Launcher::new(
            vec![AppSpec {
                id: "kodi".into(),
                name: "Kodi".into(),
                command: "kodi".into(),
                args: vec![],
                icon: "movie".into(),
            }],
            Arc::new(MockPlatform::new()),
            room.clone(),
        );
        let player = Arc::new(crate::player::mock::MockPlayer::new());
        let state = AppState {
            room: room.clone(),
            launcher: launcher.clone(),
            join: Arc::new(JoinCodes::new()),
            presence: Arc::new(Presence::new()),
            player: Some(player.clone()),
            music: Arc::new(Status::new()),
            clock: clock as Arc<dyn Clock>,
            join_base: "http://pi.test:7373".into(),
        };
        (router(state.clone()), state, player)
    }

    /// Parse a response body as JSON.
    pub async fn json(response: axum::response::Response) -> serde_json::Value {
        use http_body_util::BodyExt;
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// A bare GET. Named `get_req` because `axum::routing::get` is in scope.
    pub fn get_req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    /// A POST with no body.
    pub fn post_empty(uri: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    /// A POST carrying a JSON body, and optionally a bearer token.
    pub fn post_json(uri: &str, body: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    /// A GET carrying a bearer token.
    pub fn get_with_token(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn get_events_is_empty_on_a_fresh_log() {
        let (app, _state) = app();
        let response = app.oneshot(get_req("/api/events")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await, serde_json::json!([]));
    }

    #[tokio::test]
    async fn post_events_appends_and_returns_the_stored_event() {
        let (app, state) = app();
        let room = &state.room;
        let request = Request::builder()
            .method("POST")
            .uri("/api/events")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"kind":"match.result","actors":["tate"]}"#))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = json(response).await;
        assert_eq!(body["kind"], "match.result");
        assert!(body["id"].as_i64().unwrap() > 0);
        assert_eq!(room.events_since(0, -1).unwrap().len(), 1);
    }

    /// Arc 3 Phase 1's Definition of Done, and the invariant it could most
    /// easily have broken. `POST /api/events` is an open ingest port: adding a
    /// `via` vocabulary must not make a presence row from a producer that has
    /// never heard of it any less valid. Absent stays absent — not defaulted to
    /// `heartbeat`, which would be a guess written into an append-only log.
    #[tokio::test]
    async fn a_presence_row_posted_without_a_via_is_still_valid_and_stays_unknown() {
        use crate::presence::via::Via;

        let (app, state) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/api/events")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"kind":"presence.arrived","actors":["marcus"]}"#,
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let stored = state.room.events_since(0, -1).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].kind, "presence.arrived");
        assert_eq!(
            Via::read(&stored[0]),
            None,
            "a row that does not say must read as unknown, never as a signal"
        );
    }

    /// The other half of an open port: a producer that knows about `via` but
    /// uses a value this build has never seen keeps its provenance.
    #[tokio::test]
    async fn a_presence_row_with_an_unknown_via_keeps_it() {
        use crate::presence::via::Via;

        let (app, state) = app();
        let request = Request::builder()
            .method("POST")
            .uri("/api/events")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"kind":"presence.arrived","actors":["marcus"],"payload":{"via":"doorbell"}}"#,
            ))
            .unwrap();

        assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::CREATED);

        let stored = state.room.events_since(0, -1).unwrap();
        assert_eq!(
            Via::read(&stored[0]),
            Some(Via::Other("doorbell".to_string()))
        );
    }

    #[tokio::test]
    async fn get_events_honours_after_and_limit() {
        let (app, state) = app();
        let room = &state.room;
        for i in 0..4 {
            room.record(crate::event::NewEvent::new(format!("k{i}")))
                .unwrap();
        }

        let response = app
            .oneshot(get_req("/api/events?after=1&limit=2"))
            .await
            .unwrap();
        let body = json(response).await;
        let kinds: Vec<_> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["k1", "k2"]);
    }

    #[tokio::test]
    async fn get_roster_reflects_presence_events() {
        let (app, state) = app();
        let room = &state.room;
        room.record(crate::event::NewEvent::new(crate::event::kind::PRESENCE_ARRIVED).actor("sam"))
            .unwrap();

        let response = app.oneshot(get_req("/api/roster")).await.unwrap();
        assert_eq!(json(response).await, serde_json::json!(["sam"]));
    }

    #[tokio::test]
    async fn get_apps_lists_the_registry_and_nothing_current() {
        let (app, _state) = app();
        let body = json(app.oneshot(get_req("/api/apps")).await.unwrap()).await;

        assert_eq!(body["apps"][0]["id"], "kodi");
        assert_eq!(body["apps"][0]["name"], "Kodi");
        assert_eq!(body["current"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn launching_an_app_returns_no_content_and_updates_current() {
        let (app, state) = app();
        let launcher = &state.launcher;
        let response = app
            .oneshot(post_empty("/api/apps/kodi/launch"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(launcher.current(), Some("kodi".to_string()));
    }

    #[tokio::test]
    async fn launching_an_unknown_app_is_not_found() {
        let (app, _state) = app();
        let response = app
            .oneshot(post_empty("/api/apps/nintendo64/launch"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn quitting_returns_no_content_and_clears_current() {
        let (app, state) = app();
        let launcher = &state.launcher;
        launcher.launch("kodi").unwrap();

        let response = app.oneshot(post_empty("/api/apps/quit")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(launcher.current(), None);
    }
}
