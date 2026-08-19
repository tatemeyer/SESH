//! The shared queue, over HTTP.
//!
//! Reads are open — the TV needs them and there is nothing private in a queue.
//! Adding and vetoing require a token, because those are the two actions that
//! must have an actor attached to them.
//!
//! Nothing here plays anything: this module records what the room decided, and
//! [`conductor`](crate::conductor) is what makes the speaker agree with it. The
//! queue was complete and correct before it was audible, which is the point of
//! the split — and why `player: "offline"` is a field here rather than an
//! error, since a room can still decide what it wants to hear.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::event::{kind, NewEvent};
use crate::player::Track;
use crate::projections::queue::Entry;
use crate::veto;

use super::auth::Authenticated;
use super::AppState;

/// Longest track URI accepted. Generous next to a Spotify URI, and bounded so
/// the open-ish ingest surface cannot be used to store arbitrary blobs.
const MAX_URI_LEN: usize = 300;

/// Longest title or artist kept.
const MAX_TEXT_LEN: usize = 200;

/// Longest search query accepted.
const MAX_QUERY_LEN: usize = 200;

/// The whole queue, plus what a veto currently costs.
#[derive(Debug, Serialize)]
pub struct MusicResponse {
    /// What is playing, if anything.
    pub now_playing: Option<Entry>,
    /// What is waiting, in order.
    pub pending: Vec<Entry>,
    /// Who is in the room — the denominator for a veto.
    pub present: Vec<String>,
    /// Votes needed to skip a track right now.
    pub needed: usize,
    /// `ok` when the music source is answering, `offline` when it is not.
    ///
    /// The queue keeps accepting tracks either way — a phone should be able to
    /// say what it wants to hear while the Wi-Fi is being difficult — but the
    /// surface needs to be able to say so rather than implying silence is a
    /// choice somebody made.
    pub player: &'static str,
}

/// What to look for.
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    /// The search text, as typed.
    pub q: String,
}

/// A track someone wants to hear.
#[derive(Debug, Deserialize)]
pub struct QueueRequest {
    /// Track URI, e.g. `spotify:track:...`.
    pub uri: String,
    /// Track title.
    #[serde(default)]
    pub title: String,
    /// Artist name.
    #[serde(default)]
    pub artist: String,
    /// Length in milliseconds, if known.
    #[serde(default)]
    pub duration_ms: i64,
}

/// Confirmation that a track is in the queue.
#[derive(Debug, Serialize)]
pub struct QueuedResponse {
    /// The new entry's id — what a veto refers to.
    pub entry: i64,
}

/// A vote to skip.
#[derive(Debug, Deserialize)]
pub struct VetoRequest {
    /// Which queue entry. Not the track URI: the same song can be in the
    /// queue twice and the two are vetoed separately.
    pub entry: i64,
}

/// Where a track's vote count stands after a vote.
#[derive(Debug, Serialize)]
pub struct VetoResponse {
    /// Votes cast by people who are still here.
    pub votes: usize,
    /// Votes needed to skip.
    pub needed: usize,
    /// Whether that threshold is now met.
    pub carried: bool,
}

/// `GET /api/music`
pub async fn get_music(State(state): State<AppState>) -> Json<MusicResponse> {
    let queue = state.room.queue();
    let present = state.room.roster();

    Json(MusicResponse {
        now_playing: queue.now_playing().cloned(),
        pending: queue.pending().to_vec(),
        needed: veto::needed(&present),
        present,
        player: state.music.label(),
    })
}

/// `GET /api/music/search?q=`
///
/// Proxied through `seshd` rather than called from the phone directly: the
/// access token would otherwise have to reach every browser in the house, and
/// it is the house account's, not theirs.
pub async fn search_tracks(
    State(state): State<AppState>,
    Authenticated(_person): Authenticated,
    Query(request): Query<SearchRequest>,
) -> Result<Json<Vec<Track>>, StatusCode> {
    let query = request.q.trim();
    if query.is_empty() || query.len() > MAX_QUERY_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }

    let Some(player) = state.player.as_ref() else {
        // No credentials on this box. Honest 503 rather than an empty list,
        // which would read as "nothing matched".
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    player.search(query).await.map(Json).map_err(|error| {
        tracing::warn!(%error, "searching the music source failed");
        StatusCode::BAD_GATEWAY
    })
}

/// `POST /api/music/queue`
pub async fn queue_track(
    State(state): State<AppState>,
    Authenticated(person): Authenticated,
    Json(request): Json<QueueRequest>,
) -> Result<(StatusCode, Json<QueuedResponse>), StatusCode> {
    let uri = request.uri.trim();
    if uri.is_empty() || uri.len() > MAX_URI_LEN {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Deliberately not checking for a `spotify:track:` prefix. The log is
    // meant to outlive this arc's choice of music source, and a URI scheme is
    // exactly the kind of thing a later arc changes.
    let event = NewEvent::new(kind::MUSIC_QUEUED)
        .actor(&person.id)
        .subject(uri)
        .payload(serde_json::json!({
            "title": clamp(&request.title),
            "artist": clamp(&request.artist),
            "duration_ms": request.duration_ms.max(0),
        }));

    let recorded = state.room.record(event).map_err(|error| {
        tracing::error!(%error, "recording music.queued failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(who = %person.id, entry = recorded.id, %uri, "queued a track");

    // The entry id is the event's own id — see `projections::queue`.
    Ok((
        StatusCode::CREATED,
        Json(QueuedResponse { entry: recorded.id }),
    ))
}

/// `POST /api/music/veto`
pub async fn veto_track(
    State(state): State<AppState>,
    Authenticated(person): Authenticated,
    Json(request): Json<VetoRequest>,
) -> Result<Json<VetoResponse>, StatusCode> {
    // Read the entry before voting, both to reject a vote for something that
    // is not in the queue and to put the track's URI on the event — so the log
    // still reads as "who wanted to skip what" without a join.
    let uri = state
        .room
        .queue()
        .find(request.entry)
        .map(|entry| entry.uri.clone())
        .ok_or(StatusCode::NOT_FOUND)?;

    state
        .room
        .record(
            NewEvent::new(kind::MUSIC_VETOED)
                .actor(&person.id)
                .subject(uri)
                .payload(serde_json::json!({ "entry": request.entry })),
        )
        .map_err(|error| {
            tracing::error!(%error, "recording music.vetoed failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Re-read rather than counting locally: the projection deduplicates a
    // second vote from the same person, and the tally shown must be the one
    // the room will actually act on.
    let present = state.room.roster();
    let votes = state
        .room
        .queue()
        .find(request.entry)
        .map(|entry| veto::counted(&entry.vetoes, &present))
        .unwrap_or(0);

    Ok(Json(VetoResponse {
        votes,
        needed: veto::needed(&present),
        carried: votes >= veto::needed(&present),
    }))
}

/// Keep free-form metadata to a sane length before it enters the log forever.
fn clamp(text: &str) -> String {
    text.chars().take(MAX_TEXT_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testing::*;
    use tower::ServiceExt;

    async fn joined(app: &axum::Router, state: &AppState, name: &str) -> String {
        let code = state.join.current(crate::store::now_ms());
        let body = serde_json::json!({ "code": code, "name": name }).to_string();
        let response = app
            .clone()
            .oneshot(post_json("/api/join", &body, None))
            .await
            .unwrap();
        let token = json(response).await["token"].as_str().unwrap().to_string();
        // Joining alone does not make you present; the heartbeat does.
        app.clone()
            .oneshot(post_json("/api/heartbeat", "{}", Some(&token)))
            .await
            .unwrap();
        token
    }

    async fn add(app: &axum::Router, token: &str, uri: &str, title: &str) -> i64 {
        let body = serde_json::json!({
            "uri": uri, "title": title, "artist": "Someone", "duration_ms": 210_000
        })
        .to_string();
        let response = app
            .clone()
            .oneshot(post_json("/api/music/queue", &body, Some(token)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        json(response).await["entry"].as_i64().unwrap()
    }

    async fn music(app: &axum::Router) -> serde_json::Value {
        json(app.clone().oneshot(get_req("/api/music")).await.unwrap()).await
    }

    #[tokio::test]
    async fn a_search_needs_a_token() {
        let (app, _state) = app();
        let response = app
            .oneshot(get_req("/api/music/search?q=teenage%20dirtbag"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_search_returns_what_the_source_found() {
        let (app, state, player) = crate::api::testing::app_with_music();
        let sam = joined(&app, &state, "Sam").await;
        player.set_results(vec![Track {
            uri: "spotify:track:a".into(),
            title: "Teenage Dirtbag".into(),
            artist: "Wheatus".into(),
            duration_ms: 240_000,
        }]);

        let response = app
            .oneshot(get_with_token(
                "/api/music/search?q=teenage%20dirtbag",
                &sam,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = json(response).await;
        assert_eq!(body[0]["title"], "Teenage Dirtbag");
        assert_eq!(body[0]["uri"], "spotify:track:a");
    }

    // The query has to reach the source intact; a handler that dropped it
    // would still return results and look perfectly fine.
    #[tokio::test]
    async fn the_query_reaches_the_source_as_typed() {
        let (app, state, player) = crate::api::testing::app_with_music();
        let sam = joined(&app, &state, "Sam").await;

        app.oneshot(get_with_token(
            "/api/music/search?q=hounds%20of%20love",
            &sam,
        ))
        .await
        .unwrap();

        assert_eq!(
            player.calls(),
            vec![crate::player::mock::Call::Search("hounds of love".into())]
        );
    }

    #[tokio::test]
    async fn an_empty_search_is_rejected() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;

        let response = app
            .oneshot(get_with_token("/api/music/search?q=%20%20", &sam))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // A source that is down is not the same as a search that found nothing,
    // and a phone showing "no results" for an outage sends someone to reboot
    // the router.
    #[tokio::test]
    async fn a_broken_source_is_a_bad_gateway_not_an_empty_list() {
        let (app, state, player) = crate::api::testing::app_with_music();
        let sam = joined(&app, &state, "Sam").await;
        player.fail_with("spotify unreachable");

        let response = app
            .oneshot(get_with_token("/api/music/search?q=anything", &sam))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn a_box_with_no_credentials_says_so() {
        let (app, state, _player) = crate::api::testing::app_with_music();
        let mut state = state;
        state.player = None;
        let app_without = crate::api::router(state.clone());
        let sam = joined(&app, &state, "Sam").await;

        let response = app_without
            .oneshot(get_with_token("/api/music/search?q=anything", &sam))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // 4.3: the queue stays usable while the source is down, and the surface
    // is told which of those two worlds it is in.
    #[tokio::test]
    async fn the_response_says_whether_the_source_is_answering() {
        let (app, _state) = app();
        assert_eq!(music(&app).await["player"], "offline");
    }

    #[tokio::test]
    async fn an_empty_queue_reads_as_empty() {
        let (app, _state) = app();
        let body = music(&app).await;

        assert_eq!(body["now_playing"], serde_json::Value::Null);
        assert_eq!(body["pending"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn queueing_needs_a_token() {
        let (app, _state) = app();
        let body = serde_json::json!({ "uri": "spotify:track:a" }).to_string();

        let response = app
            .oneshot(post_json("/api/music/queue", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_queued_track_appears_with_who_added_it() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let entry = add(&app, &sam, "spotify:track:a", "A").await;

        let body = music(&app).await;
        let pending = body["pending"].as_array().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["entry"], entry);
        assert_eq!(pending[0]["title"], "A");
        assert_eq!(pending[0]["added_by"], "sam");
    }

    #[tokio::test]
    async fn two_phones_build_one_queue_in_order() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let marcus = joined(&app, &state, "Marcus").await;

        add(&app, &sam, "spotify:track:a", "A").await;
        add(&app, &marcus, "spotify:track:b", "B").await;
        add(&app, &sam, "spotify:track:c", "C").await;

        let body = music(&app).await;
        let titles: Vec<_> = body["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(titles, vec!["A", "B", "C"]);
    }

    #[tokio::test]
    async fn an_empty_uri_is_rejected() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let body = serde_json::json!({ "uri": "  " }).to_string();

        let response = app
            .oneshot(post_json("/api/music/queue", &body, Some(&sam)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_absurd_uri_is_rejected() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let body = serde_json::json!({ "uri": "x".repeat(MAX_URI_LEN + 1) }).to_string();

        let response = app
            .oneshot(post_json("/api/music/queue", &body, Some(&sam)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_needed_count_tracks_who_is_in_the_room() {
        let (app, state) = app();

        // Nobody here: the floor still applies.
        assert_eq!(music(&app).await["needed"], veto::MIN_VOTES);

        joined(&app, &state, "Sam").await;
        assert_eq!(music(&app).await["needed"], 2);

        joined(&app, &state, "Marcus").await;
        joined(&app, &state, "Ali").await;
        assert_eq!(music(&app).await["needed"], 2);

        joined(&app, &state, "Dee").await;
        assert_eq!(music(&app).await["needed"], 3);
    }

    #[tokio::test]
    async fn a_veto_is_recorded_and_reports_the_tally() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let marcus = joined(&app, &state, "Marcus").await;
        let entry = add(&app, &sam, "spotify:track:a", "A").await;

        let body = serde_json::json!({ "entry": entry }).to_string();
        let first = app
            .clone()
            .oneshot(post_json("/api/music/veto", &body, Some(&sam)))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let tally = json(first).await;
        assert_eq!(tally["votes"], 1);
        assert_eq!(tally["needed"], 2);
        assert_eq!(tally["carried"], false);

        let second = app
            .oneshot(post_json("/api/music/veto", &body, Some(&marcus)))
            .await
            .unwrap();
        let tally = json(second).await;
        assert_eq!(tally["votes"], 2);
        assert_eq!(tally["carried"], true);
    }

    #[tokio::test]
    async fn voting_twice_does_not_double_a_persons_vote() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        joined(&app, &state, "Marcus").await;
        let entry = add(&app, &sam, "spotify:track:a", "A").await;
        let body = serde_json::json!({ "entry": entry }).to_string();

        app.clone()
            .oneshot(post_json("/api/music/veto", &body, Some(&sam)))
            .await
            .unwrap();
        let again = app
            .oneshot(post_json("/api/music/veto", &body, Some(&sam)))
            .await
            .unwrap();

        let tally = json(again).await;
        assert_eq!(tally["votes"], 1, "one person, one vote");
        assert_eq!(tally["carried"], false);
    }

    // The D1 regression, over HTTP: two people queue the same song and each
    // copy must be vetoable on its own.
    #[tokio::test]
    async fn the_same_song_queued_twice_is_vetoed_separately() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let marcus = joined(&app, &state, "Marcus").await;

        let first = add(&app, &sam, "spotify:track:a", "A").await;
        let second = add(&app, &marcus, "spotify:track:a", "A").await;
        assert_ne!(first, second);

        // Vote on the *second* copy. Naming the first would pass even against
        // an implementation that looked entries up by URI, which is precisely
        // the bug this test exists to rule out.
        let body = serde_json::json!({ "entry": second }).to_string();
        app.clone()
            .oneshot(post_json("/api/music/veto", &body, Some(&sam)))
            .await
            .unwrap();

        let pending = music(&app).await;
        let entries = pending["pending"].as_array().unwrap();
        assert_eq!(
            entries[0]["vetoes"].as_array().unwrap().len(),
            0,
            "the first copy must be untouched"
        );
        assert_eq!(entries[1]["vetoes"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn vetoing_a_track_that_is_not_queued_is_not_found() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let body = serde_json::json!({ "entry": 9999 }).to_string();

        let response = app
            .oneshot(post_json("/api/music/veto", &body, Some(&sam)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn vetoing_needs_a_token() {
        let (app, _state) = app();
        let body = serde_json::json!({ "entry": 1 }).to_string();

        let response = app
            .oneshot(post_json("/api/music/veto", &body, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_log_records_who_queued_and_who_voted() {
        let (app, state) = app();
        let sam = joined(&app, &state, "Sam").await;
        let entry = add(&app, &sam, "spotify:track:a", "A").await;
        let body = serde_json::json!({ "entry": entry }).to_string();
        app.oneshot(post_json("/api/music/veto", &body, Some(&sam)))
            .await
            .unwrap();

        let events = state.room.events_since(0, -1).unwrap();
        let queued = events
            .iter()
            .find(|e| e.kind == kind::MUSIC_QUEUED)
            .expect("music.queued");
        assert_eq!(queued.actors, vec!["sam".to_string()]);
        assert_eq!(queued.subject.as_deref(), Some("spotify:track:a"));

        let vote = events
            .iter()
            .find(|e| e.kind == kind::MUSIC_VETOED)
            .expect("music.vetoed");
        assert_eq!(vote.actors, vec!["sam".to_string()]);
        assert_eq!(vote.payload["entry"], entry);
    }
}
