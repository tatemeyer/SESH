//! `SpotifyPlayer` against a stub Spotify, on a real port.
//!
//! The token-refresh and rate-limit paths cannot be reached by mapping tests,
//! and they are where the failures actually live: a token that silently stops
//! refreshing looks like "the music stopped" an hour into an evening, with
//! nothing in the log pointing at the cause.
//!
//! No network leaves the machine — the stub binds `127.0.0.1:0`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use seshd::player::spotify::{Endpoints, SpotifyConfig, SpotifyPlayer};
use seshd::player::Player;

/// What the stub was asked, and how it should answer.
#[derive(Default)]
struct Stub {
    tokens_issued: AtomicUsize,
    api_calls: AtomicUsize,
    /// Fail the first N API calls with this status.
    fail_first: AtomicUsize,
    fail_status: AtomicUsize,
    /// Hand back a rotated refresh token on the next grant.
    rotate_refresh: AtomicUsize,
    searches: std::sync::Mutex<Vec<Value>>,
    transfers: std::sync::Mutex<Vec<Value>>,
    /// Non-zero hides the room's device, as a box with librespot down would.
    hide_room_device: AtomicUsize,
    /// Raw query string of every play and queue call.
    targeted: std::sync::Mutex<Vec<String>>,
    /// Whether each bodyless call carried a Content-Length.
    lengths: std::sync::Mutex<Vec<Option<String>>>,
    /// Non-zero makes command endpoints answer 200 with a body that is not
    /// JSON, which is what the real service did on 2026-08-20 and what broke
    /// the veto in the room. The stub answered 204 to everything before this,
    /// and 204 is the one answer that could never have caught it.
    commands_answer_junk: AtomicUsize,
}

type Shared = Arc<Stub>;

async fn token(State(stub): State<Shared>) -> Json<Value> {
    let n = stub.tokens_issued.fetch_add(1, Ordering::SeqCst) + 1;
    let mut body = json!({ "access_token": format!("access-{n}"), "expires_in": 3600 });
    if stub.rotate_refresh.load(Ordering::SeqCst) > 0 {
        body["refresh_token"] = json!("rotated-refresh");
    }
    Json(body)
}

/// Apply any scripted failure. Returns `Some(response)` when it fired.
fn scripted_failure(stub: &Stub) -> Option<Response> {
    let seen = stub.api_calls.fetch_add(1, Ordering::SeqCst) + 1;
    if seen > stub.fail_first.load(Ordering::SeqCst) {
        return None;
    }
    let status = StatusCode::from_u16(stub.fail_status.load(Ordering::SeqCst) as u16).unwrap();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Some((status, [("retry-after", "1")], "slow down").into_response());
    }
    Some((status, "nope").into_response())
}

async fn me_player(State(stub): State<Shared>) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    Json(json!({
        "is_playing": true,
        "progress_ms": 42_000,
        "device": { "name": "SESH" },
        "item": {
            "uri": "spotify:track:playing",
            "name": "Mr. Brightside",
            "artists": [{ "name": "The Killers" }],
            "duration_ms": 222_000
        }
    }))
    .into_response()
}

async fn me_player_empty(State(stub): State<Shared>) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn search(
    State(stub): State<Shared>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    stub.searches.lock().unwrap().push(json!(params));
    Json(json!({ "tracks": { "items": [
        { "uri": "spotify:track:a", "name": "A", "artists": [{ "name": "Wheatus" }],
          "duration_ms": 235_000 },
        { "name": "no uri, must be dropped" }
    ]}}))
    .into_response()
}

async fn devices(State(stub): State<Shared>) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    if stub.hide_room_device.load(Ordering::SeqCst) > 0 {
        return Json(json!({ "devices": [
            { "id": "other-id", "name": "Someone's Phone" }
        ]}))
        .into_response();
    }
    Json(json!({ "devices": [
        { "id": "other-id", "name": "Someone's Phone" },
        { "id": "sesh-device-id", "name": "SESH" }
    ]}))
    .into_response()
}

/// Records the query it was called with, then answers like Spotify does.
async fn record_target(
    State(stub): State<Shared>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    stub.targeted
        .lock()
        .unwrap()
        .push(query.unwrap_or_default());
    StatusCode::NO_CONTENT.into_response()
}

async fn transfer(State(stub): State<Shared>, Json(body): Json<Value>) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    stub.transfers.lock().unwrap().push(body);
    StatusCode::NO_CONTENT.into_response()
}

/// Records whether the request carried a Content-Length, then answers.
///
/// Spotify's edge is stricter than a naive stub: it answers 411 to a POST or
/// PUT without one, and reqwest omits the header for a bodyless request. A stub
/// that does not look is a stub that lets that ship.
async fn record_length(State(stub): State<Shared>, headers: axum::http::HeaderMap) -> Response {
    if let Some(failure) = scripted_failure(&stub) {
        return failure;
    }
    if stub.commands_answer_junk.load(Ordering::SeqCst) > 0 {
        stub.lengths.lock().unwrap().push(
            headers
                .get(axum::http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
        );
        // A 2xx that is not 204, carrying something that is not JSON.
        return (StatusCode::OK, "OK").into_response();
    }
    stub.lengths.lock().unwrap().push(
        headers
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Start a stub and a player pointed at it.
async fn serve(nothing_playing: bool) -> (Shared, SpotifyPlayer, tempfile::TempDir) {
    let stub: Shared = Arc::new(Stub::default());

    let player_route = if nothing_playing {
        get(me_player_empty).put(transfer)
    } else {
        get(me_player).put(transfer)
    };

    let app = Router::new()
        .route("/api/token", post(token))
        .route("/v1/me/player", player_route)
        .route("/v1/me/player/devices", get(devices))
        .route("/v1/me/player/queue", post(record_target))
        .route("/v1/me/player/play", axum::routing::put(record_target))
        .route("/v1/me/player/next", post(record_length))
        .route("/v1/me/player/pause", axum::routing::put(record_length))
        .route("/v1/search", get(search))
        .with_state(stub.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("spotify-token.json");

    let player = SpotifyPlayer::with_refresh_token(
        SpotifyConfig {
            client_id: "client".into(),
            client_secret: "secret".into(),
            device_name: "SESH".into(),
        },
        token_path,
        "original-refresh".into(),
    )
    .with_endpoints(Endpoints {
        api: format!("http://{addr}/v1"),
        accounts: format!("http://{addr}"),
    });

    (stub, player, dir)
}

/// Start just the accounts endpoint and return its base URL.
async fn serve_accounts(with_refresh: bool) -> String {
    let stub: Shared = Arc::new(Stub::default());
    if with_refresh {
        stub.rotate_refresh.store(1, Ordering::SeqCst);
    }
    let app = Router::new()
        .route("/api/token", post(token))
        .with_state(stub);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

// The one-time flow. If this is wrong nothing else in the arc can start, and
// the failure lands on a person standing at a terminal with a browser open.
#[tokio::test]
async fn exchanging_an_authorisation_code_yields_a_refresh_token() {
    let accounts = serve_accounts(true).await;

    let (tokens, access) = seshd::player::auth::exchange_code(
        &reqwest::Client::new(),
        &accounts,
        "client",
        "secret",
        "the-code",
        &seshd::player::auth::redirect_uri(),
    )
    .await
    .unwrap();

    assert_eq!(tokens.refresh_token, "rotated-refresh");
    assert_eq!(access.expires_in_s, 3600);
}

// Without a refresh token the room works until the access token expires and
// then stops, an hour later, for no reason anyone could see. Better to refuse
// while the person is still standing there.
#[tokio::test]
async fn an_exchange_without_a_refresh_token_is_refused_immediately() {
    let accounts = serve_accounts(false).await;

    let error = seshd::player::auth::exchange_code(
        &reqwest::Client::new(),
        &accounts,
        "client",
        "secret",
        "the-code",
        &seshd::player::auth::redirect_uri(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("refresh token"), "got {error}");
}

#[tokio::test]
async fn playback_is_fetched_and_mapped() {
    let (stub, player, _dir) = serve(false).await;

    let playing = player.playback().await.unwrap().unwrap();
    assert_eq!(playing.track.title, "Mr. Brightside");
    assert_eq!(playing.track.artist, "The Killers");
    assert_eq!(playing.remaining_ms(), 180_000);
    assert_eq!(playing.device.as_deref(), Some("SESH"));

    // One grant for the first call, and it is cached for the second.
    player.playback().await.unwrap();
    assert_eq!(stub.tokens_issued.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn nothing_playing_reads_as_none_not_an_error() {
    let (_stub, player, _dir) = serve(true).await;
    assert_eq!(player.playback().await.unwrap(), None);
}

// The path that decides whether music keeps playing an hour into an evening.
#[tokio::test]
async fn an_expired_token_is_refreshed_and_the_call_retried() {
    let (stub, player, _dir) = serve(false).await;
    stub.fail_first.store(1, Ordering::SeqCst);
    stub.fail_status
        .store(StatusCode::UNAUTHORIZED.as_u16() as usize, Ordering::SeqCst);

    let playing = player.playback().await.unwrap().unwrap();
    assert_eq!(playing.track.title, "Mr. Brightside");
    assert_eq!(
        stub.tokens_issued.load(Ordering::SeqCst),
        2,
        "the 401 must have forced a second grant"
    );
    assert_eq!(stub.api_calls.load(Ordering::SeqCst), 2);
}

// A second 401 means the grant itself is gone, not that the token was stale.
// Retrying forever would spin against a revoked authorisation.
#[tokio::test]
async fn a_persistent_401_gives_up_rather_than_looping() {
    let (stub, player, _dir) = serve(false).await;
    stub.fail_first.store(99, Ordering::SeqCst);
    stub.fail_status
        .store(StatusCode::UNAUTHORIZED.as_u16() as usize, Ordering::SeqCst);

    assert!(player.playback().await.is_err());
    assert!(
        stub.api_calls.load(Ordering::SeqCst) <= 3,
        "gave up after {} calls",
        stub.api_calls.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn a_rate_limited_call_waits_and_retries() {
    let (stub, player, _dir) = serve(false).await;
    stub.fail_first.store(1, Ordering::SeqCst);
    stub.fail_status.store(
        StatusCode::TOO_MANY_REQUESTS.as_u16() as usize,
        Ordering::SeqCst,
    );

    let started = std::time::Instant::now();
    let playing = player.playback().await.unwrap().unwrap();

    assert_eq!(playing.track.title, "Mr. Brightside");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(900),
        "Retry-After was not honoured"
    );
    // A rate limit is not an auth problem; the token must not be discarded.
    assert_eq!(stub.tokens_issued.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn search_asks_for_ten_tracks_and_drops_unusable_results() {
    let (stub, player, _dir) = serve(false).await;

    let tracks = player.search("teenage dirtbag").await.unwrap();
    assert_eq!(tracks.len(), 1, "the result with no URI must be dropped");
    assert_eq!(tracks[0].artist, "Wheatus");

    let asked = stub.searches.lock().unwrap()[0].clone();
    assert_eq!(asked["q"], "teenage dirtbag");
    assert_eq!(asked["type"], "track");
    assert_eq!(
        asked["limit"], "10",
        "Spotify rejects a limit above 10 outright since 2026-03-09"
    );
}

#[tokio::test]
async fn transfer_moves_playback_onto_the_configured_device() {
    let (stub, player, _dir) = serve(false).await;

    player.transfer().await.unwrap();

    let body = stub.transfers.lock().unwrap()[0].clone();
    assert_eq!(body["device_ids"][0], "sesh-device-id");
}

// The most likely real failure in Phase 6: librespot is not running, so the
// device simply is not there. The message has to say that.
#[tokio::test]
async fn transfer_without_the_device_explains_what_is_missing() {
    let (_stub, player, _dir) = serve(false).await;
    let player = player.with_device_name("Not Running");

    let error = player.transfer().await.unwrap_err().to_string();
    assert!(error.contains("Not Running"), "got {error}");
    assert!(error.contains("librespot"), "got {error}");
}

#[tokio::test]
async fn enqueue_and_skip_succeed_on_a_204() {
    let (_stub, player, _dir) = serve(false).await;
    player.enqueue("spotify:track:a").await.unwrap();
    player.skip().await.unwrap();
}

// Spotify occasionally rotates the refresh token. Losing the replacement
// works for an hour and then locks the room out with nothing pointing at why.
#[tokio::test]
async fn a_rotated_refresh_token_is_written_back_to_disk() {
    let (stub, player, dir) = serve(false).await;
    stub.rotate_refresh.store(1, Ordering::SeqCst);

    player.playback().await.unwrap();

    let saved = std::fs::read_to_string(dir.path().join("spotify-token.json")).unwrap();
    assert!(
        saved.contains("rotated-refresh"),
        "the rotated token was not persisted: {saved}"
    );
}

// Without naming a device, Spotify acts on whatever the house account last
// used. On a shared account that is somebody's phone, and a room whose queue
// plays into a pocket two streets away is not a room.
#[tokio::test]
async fn play_and_enqueue_name_the_room_device() {
    let (stub, player, _dir) = serve(true).await;

    player.play("spotify:track:a").await.unwrap();
    player.enqueue("spotify:track:b").await.unwrap();

    let targeted = stub.targeted.lock().unwrap().clone();
    assert_eq!(targeted.len(), 2);
    assert!(
        targeted[0].contains("device_id=sesh-device-id"),
        "play must name the room: {:?}",
        targeted[0]
    );
    assert!(
        targeted[1].contains("device_id=sesh-device-id"),
        "enqueue must name the room: {:?}",
        targeted[1]
    );
    assert!(
        targeted[1].contains("uri=spotify"),
        "and still send the uri"
    );
}

// librespot down is a silent speaker, not a broken queue. Falling back to the
// active device keeps the room usable and the warning names the cause.
#[tokio::test]
async fn a_missing_room_device_falls_back_rather_than_failing() {
    let (stub, player, _dir) = serve(true).await;
    stub.hide_room_device.store(1, Ordering::SeqCst);

    player.play("spotify:track:a").await.unwrap();

    let targeted = stub.targeted.lock().unwrap().clone();
    assert_eq!(targeted.len(), 1);
    assert!(
        !targeted[0].contains("device_id"),
        "with no room device it must name none: {:?}",
        targeted[0]
    );
}

// Spotify answers 411 Length Required to a POST or PUT with no Content-Length,
// and reqwest omits the header entirely for a bodyless request. Observed live:
// every pre-push in a real evening failed, so D7's seamless handoff had never
// worked against the real API — only the fallback to `play` kept music going.
#[tokio::test]
async fn calls_without_a_json_body_still_carry_a_content_length() {
    let (stub, player, _dir) = serve(true).await;

    player.skip().await.unwrap();
    player.pause().await.unwrap();

    let lengths = stub.lengths.lock().unwrap().clone();
    assert_eq!(lengths.len(), 2, "both calls should have reached the stub");
    for (call, length) in ["skip", "pause"].iter().zip(&lengths) {
        assert_eq!(
            length.as_deref(),
            Some("0"),
            "{call} must send Content-Length: 0, or Spotify answers 411"
        );
    }
}

/// The bug that broke the queue in the room on 2026-08-20.
///
/// Three people were in, two vetoed the playing track — a carried veto — and
/// the track played to the end anyway. `honour_vetoes` did call `skip`; every
/// call failed with "parsing Spotify's answer to POST /me/player/next", the
/// conductor returned early without recording anything, and the next tick tried
/// again until Spotify answered 429.
///
/// The cause is not what Spotify sent. It is that **a command's success
/// depended on parsing a body no caller reads**: `skip`, `pause`, `play`,
/// `enqueue`, `resume` and `transfer` all discard the result. Same shape as the
/// Content-Length bug in #25 — the client failing on the envelope of an
/// operation that has no content.
#[tokio::test]
async fn a_command_succeeds_even_when_the_answer_is_not_json() {
    let (stub, player, _dir) = serve(false).await;
    stub.commands_answer_junk.store(1, Ordering::SeqCst);

    player
        .skip()
        .await
        .expect("skip must not fail on a body nobody reads");
    player
        .pause()
        .await
        .expect("pause must not fail on a body nobody reads");
}

/// A command that genuinely fails must still fail. The fix above must not
/// become "ignore everything the server says".
#[tokio::test]
async fn a_command_still_fails_on_an_error_status() {
    let (stub, player, _dir) = serve(false).await;
    stub.fail_first.store(9, Ordering::SeqCst);
    stub.fail_status.store(500, Ordering::SeqCst);

    assert!(
        player.skip().await.is_err(),
        "a 500 is a real failure and must not be swallowed"
    );
}
