//! App launcher endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::config::AppSpec;

use super::AppState;

/// The app registry plus what is running right now.
#[derive(Debug, Serialize)]
pub struct AppsResponse {
    /// Every launchable app, in registry order.
    pub apps: Vec<AppSpec>,
    /// The id of the running app, if any.
    pub current: Option<String>,
}

/// `GET /api/apps`
pub async fn list_apps(State(state): State<AppState>) -> Json<AppsResponse> {
    Json(AppsResponse {
        apps: state.launcher.apps().to_vec(),
        current: state.launcher.current(),
    })
}

/// `POST /api/apps/:id/launch`
pub async fn launch_app(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    if !state.launcher.apps().iter().any(|a| a.id == id) {
        return Err(StatusCode::NOT_FOUND);
    }
    state
        .launcher
        .launch(&id)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            tracing::error!(%error, %id, "launching app failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `POST /api/apps/quit`
pub async fn quit_app(State(state): State<AppState>) -> Result<StatusCode, StatusCode> {
    state
        .launcher
        .quit()
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            tracing::error!(%error, "quitting app failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}
