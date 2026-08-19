//! Turning a phone into a person: the QR, the exchange, and the heartbeat.
//!
//! No accounts and no passwords. Identity is a name and a face in this house,
//! and the LAN is the trust boundary — unchanged from Arc 1, now with a token
//! so events can say *who*.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use qrcode::render::svg;
use qrcode::QrCode;
use serde::{Deserialize, Serialize};

use crate::event::{kind, NewEvent};
use crate::store::Person;

use super::auth::Authenticated;
use super::AppState;

/// Longest display name accepted. Long enough for a real name, short enough
/// that the phone list and the TV's now-playing card stay readable.
const MAX_NAME_LEN: usize = 40;

/// What a phone posts to trade its scanned code for a token.
#[derive(Debug, Deserialize)]
pub struct JoinRequest {
    /// The one-time code from the QR.
    pub code: String,
    /// What they want to be called.
    pub name: String,
}

/// What a phone gets back. The only time a token is ever sent anywhere.
#[derive(Debug, Serialize)]
pub struct JoinResponse {
    /// Their new person id.
    pub id: String,
    /// Their display name.
    pub name: String,
    /// The bearer token to keep and send on every later request.
    pub token: String,
}

/// `GET /api/join/qr.svg` — the code the TV displays.
pub async fn join_qr(State(state): State<AppState>) -> Response {
    let code = state.join.current(state.clock.mono_ms());
    let url = format!("{}/join?c={}", state.join_base, code);

    let svg = match QrCode::new(url.as_bytes()) {
        Ok(qr) => qr
            .render::<svg::Color<'_>>()
            .min_dimensions(240, 240)
            .quiet_zone(true)
            .build(),
        Err(error) => {
            tracing::error!(%error, "could not encode the join QR");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            // The code rotates, and a cached QR is a code that outlives its
            // window — the one thing rotation exists to prevent.
            (header::CACHE_CONTROL, "no-store, must-revalidate"),
        ],
        svg,
    )
        .into_response()
}

/// `POST /api/join` — spend a code, become a person, receive a token.
pub async fn join(
    State(state): State<AppState>,
    Json(request): Json<JoinRequest>,
) -> Result<(StatusCode, Json<JoinResponse>), StatusCode> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }

    // A refusal, not a missing thing: the code was wrong, expired, or already
    // spent, and the phone's remedy is the same in all three cases — rescan.
    // Which of the three it was is deliberately not disclosed.
    if !state.join.redeem(&request.code, state.clock.mono_ms()) {
        return Err(StatusCode::FORBIDDEN);
    }

    let token = crate::join::new_token();
    let person = state.room.register_person(name, &token).map_err(|error| {
        tracing::error!(%error, "registering a person failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    state
        .room
        .record(
            NewEvent::new(kind::PERSON_JOINED)
                .actor(&person.id)
                .payload(serde_json::json!({ "name": person.name })),
        )
        .map_err(|error| {
            tracing::error!(%error, "recording person.joined failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!(id = %person.id, "a phone joined the room");

    Ok((
        StatusCode::CREATED,
        Json(JoinResponse {
            id: person.id,
            name: person.name,
            token,
        }),
    ))
}

/// `GET /api/me` — who this token belongs to.
pub async fn me(Authenticated(person): Authenticated) -> Json<Person> {
    Json(person)
}

/// `POST /api/heartbeat` — "this phone is still in the room".
pub async fn heartbeat(
    State(state): State<AppState>,
    Authenticated(person): Authenticated,
) -> Result<StatusCode, StatusCode> {
    if let Some(arrival) = state.presence.beat(&person.id, state.clock.mono_ms()) {
        state.room.record(arrival).map_err(|error| {
            tracing::error!(%error, "recording presence.arrived failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testing::*;
    use crate::clock::Clock;
    use tower::ServiceExt;

    /// Join a fresh room and return the token handed back.
    async fn join_as(app: &axum::Router, state: &AppState, name: &str) -> String {
        let code = state.join.current(state.clock.mono_ms());
        let body = serde_json::json!({ "code": code, "name": name }).to_string();
        let response = app
            .clone()
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        json(response).await["token"].as_str().unwrap().to_string()
    }

    // The measured failure. `seshd` comes up ~9s before NTP answers, and when
    // it answers the wall clock leaps 13m28s in one step — past ROTATE_MS and
    // its grace at once. A guest halfway through scanning the TV gets an
    // expired code for something neither they nor the room did.
    #[tokio::test]
    async fn an_ntp_step_does_not_expire_the_code_on_the_tv() {
        let (app, state, clock) = app_with_clock();
        let code = state.join.current(state.clock.mono_ms());

        // A second of real time passes while they scan; then NTP lands.
        clock.advance(1_000);
        clock.set_wall_ms(clock.now_ms() + 13 * 60 * 1000 + 28_000);

        let body = serde_json::json!({ "code": code, "name": "Marcus" }).to_string();
        let response = app
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "a code one second old must still be good"
        );
    }

    // Rotation is a duration and must keep working when nothing else does.
    #[tokio::test]
    async fn the_code_still_rotates_on_monotonic_time() {
        let (_app, state, clock) = app_with_clock();
        let first = state.join.current(state.clock.mono_ms());
        clock.advance(crate::join::ROTATE_MS);
        let second = state.join.current(state.clock.mono_ms());
        assert_ne!(first, second, "a minute of real time must still rotate it");
    }

    #[tokio::test]
    async fn the_qr_endpoint_serves_an_svg_that_is_never_cached() {
        let (app, _state) = app();
        let response = app.oneshot(get_req("/api/join/qr.svg")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/svg+xml");
        // A cached QR is a code that outlives its window.
        assert!(response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .contains("no-store"));

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("<svg"), "not an svg");
    }

    #[tokio::test]
    async fn joining_with_a_valid_code_creates_a_person_and_returns_a_token() {
        let (app, state) = app();
        let code = state.join.current(state.clock.mono_ms());
        let body = serde_json::json!({ "code": code, "name": "Marcus" }).to_string();

        let response = app
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let payload = json(response).await;
        assert_eq!(payload["id"], "marcus");
        assert_eq!(payload["name"], "Marcus");
        assert!(!payload["token"].as_str().unwrap().is_empty());
        assert_eq!(state.room.people().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn joining_records_person_joined_with_the_new_id_as_actor() {
        let (app, state) = app();
        join_as(&app, &state, "Marcus").await;

        let events = state.room.events_since(0, -1).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, kind::PERSON_JOINED);
        assert_eq!(events[0].actors, vec!["marcus".to_string()]);
        assert_eq!(events[0].payload["name"], "Marcus");
    }

    // The token is the one secret here. It must reach the phone that earned it
    // and appear nowhere else — least of all the log, which is append-only and
    // served unauthenticated, so a leak there could never be walked back.
    #[tokio::test]
    async fn the_token_never_reaches_the_event_log() {
        let (app, state) = app();
        let token = join_as(&app, &state, "Marcus").await;

        let log = serde_json::to_string(&state.room.events_since(0, -1).unwrap()).unwrap();
        assert!(!log.contains(&token), "token leaked into the log");
    }

    #[tokio::test]
    async fn a_bogus_code_is_refused() {
        let (app, _state) = app();
        let body = serde_json::json!({ "code": "nope", "name": "Marcus" }).to_string();

        let response = app
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_code_cannot_be_spent_twice() {
        let (app, state) = app();
        let code = state.join.current(state.clock.mono_ms());
        let body = serde_json::json!({ "code": code, "name": "Marcus" }).to_string();

        let first = app
            .clone()
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);

        let second = app
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(
            second.status(),
            StatusCode::FORBIDDEN,
            "a photographed QR must not keep working"
        );
        assert_eq!(state.room.people().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_blank_name_is_rejected_without_burning_the_code() {
        let (app, _state) = app();
        let code = _state.join.current(_state.clock.mono_ms());
        let body = serde_json::json!({ "code": code, "name": "   " }).to_string();

        let response = app
            .clone()
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // The name is validated before the code is spent, on purpose: a typo
        // must not cost the guest their scan and send them back to the TV.
        let retry = serde_json::json!({ "code": code, "name": "Marcus" }).to_string();
        let ok = app
            .oneshot(post_json("/api/join", &retry, None))
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn an_over_long_name_is_rejected() {
        let (app, state) = app();
        let code = state.join.current(state.clock.mono_ms());
        let name = "a".repeat(MAX_NAME_LEN + 1);
        let body = serde_json::json!({ "code": code, "name": name }).to_string();

        let response = app
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn me_returns_the_person_behind_the_token() {
        let (app, state) = app();
        let token = join_as(&app, &state, "Marcus").await;

        let response = app
            .oneshot(get_with_token("/api/me", &token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let payload = json(response).await;
        assert_eq!(payload["id"], "marcus");
        assert!(
            payload.get("token").is_none(),
            "the token was echoed back to a client"
        );
    }

    #[tokio::test]
    async fn me_without_a_token_is_unauthorized() {
        let (app, _state) = app();
        let response = app.oneshot(get_req("/api/me")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_with_an_unknown_token_is_unauthorized() {
        let (app, _state) = app();
        let response = app
            .oneshot(get_with_token("/api/me", "not-a-token"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_heartbeat_puts_someone_on_the_roster_exactly_once() {
        let (app, state) = app();
        let token = join_as(&app, &state, "Marcus").await;

        let first = app
            .clone()
            .oneshot(post_json("/api/heartbeat", "{}", Some(&token)))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::NO_CONTENT);
        assert_eq!(state.room.roster(), vec!["marcus".to_string()]);

        let before = state.room.events_since(0, -1).unwrap().len();
        let second = app
            .oneshot(post_json("/api/heartbeat", "{}", Some(&token)))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            state.room.events_since(0, -1).unwrap().len(),
            before,
            "a repeat heartbeat must not append anything"
        );
    }

    #[tokio::test]
    async fn a_heartbeat_without_a_token_is_unauthorized() {
        let (app, _state) = app();
        let response = app
            .oneshot(post_json("/api/heartbeat", "{}", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
