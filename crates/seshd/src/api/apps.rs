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

/// Run blocking launcher work somewhere other than a runtime worker.
///
/// `Launcher::launch` and `Launcher::quit` spawn or kill a process while
/// holding a `std::sync::Mutex`, and `quit` waits out a SIGTERM grace period
/// measured in seconds. Awaiting that on an `async` worker parks the worker
/// for the whole duration — invisible with four workers and one viewer, and
/// wrong regardless.
async fn offload<F>(work: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|join| Err(anyhow::anyhow!("launcher task failed to run: {join}")))
}

/// `POST /api/apps/:id/launch`
pub async fn launch_app(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    // The registry lookup takes no lock and spawns nothing, so an unknown id
    // is rejected without paying for a blocking task.
    if !state.launcher.apps().iter().any(|a| a.id == id) {
        return Err(StatusCode::NOT_FOUND);
    }
    let launcher = state.launcher.clone();
    let wanted = id.clone();
    offload(move || launcher.launch(&id))
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            tracing::error!(%error, id = %wanted, "launching app failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `POST /api/apps/quit`
pub async fn quit_app(State(state): State<AppState>) -> Result<StatusCode, StatusCode> {
    let launcher = state.launcher.clone();
    offload(move || launcher.quit())
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            tracing::error!(%error, "quitting app failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}
