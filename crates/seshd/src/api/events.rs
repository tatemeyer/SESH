//! Event log endpoints.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::event::{Event, NewEvent};

use super::AppState;

/// Query parameters for reading history.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Return events with an id greater than this. Defaults to 0.
    #[serde(default)]
    pub after: i64,
    /// Maximum number of events. Defaults to 500.
    pub limit: Option<i64>,
}

/// `GET /api/events` — read history.
pub async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<Vec<Event>>, StatusCode> {
    let limit = query.limit.unwrap_or(500);
    state
        .room
        .events_since(query.after, limit)
        .map(Json)
        .map_err(|error| {
            tracing::error!(%error, "reading events failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `POST /api/events` — the ingest port. Any producer may append here;
/// this is where the deferred game-capture strategy will plug in.
pub async fn post_event(
    State(state): State<AppState>,
    Json(new): Json<NewEvent>,
) -> Result<(StatusCode, Json<Event>), StatusCode> {
    state
        .room
        .record(new)
        .map(|event| (StatusCode::CREATED, Json(event)))
        .map_err(|error| {
            tracing::error!(%error, "recording event failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `GET /api/roster` — who is in the room.
pub async fn get_roster(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.room.roster())
}
