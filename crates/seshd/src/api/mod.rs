//! The LAN-facing HTTP and WebSocket API. Arc 1 is unauthenticated by
//! design; the per-person token model arrives with phones in Arc 3.

pub mod apps;
pub mod events;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use crate::launcher::Launcher;
use crate::room::Room;

/// Everything the API handlers need. Cheap to clone — both fields are `Arc`.
#[derive(Clone)]
pub struct AppState {
    /// The event log and its projections.
    pub room: Arc<Room>,
    /// The app launcher.
    pub launcher: Arc<Launcher>,
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
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppSpec;
    use crate::launcher::platform::MockPlatform;
    use crate::store::Store;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> (Router, Arc<Room>, Arc<Launcher>) {
        let room = Room::new(Store::open_in_memory().unwrap()).unwrap();
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
        let state = AppState {
            room: room.clone(),
            launcher: launcher.clone(),
        };
        (router(state), room, launcher)
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn post_empty(uri: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_events_is_empty_on_a_fresh_log() {
        let (app, _, _) = app();
        let response = app.oneshot(get("/api/events")).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json(response).await, serde_json::json!([]));
    }

    #[tokio::test]
    async fn post_events_appends_and_returns_the_stored_event() {
        let (app, room, _) = app();
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

    #[tokio::test]
    async fn get_events_honours_after_and_limit() {
        let (app, room, _) = app();
        for i in 0..4 {
            room.record(crate::event::NewEvent::new(format!("k{i}")))
                .unwrap();
        }

        let response = app
            .oneshot(get("/api/events?after=1&limit=2"))
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
        let (app, room, _) = app();
        room.record(crate::event::NewEvent::new(crate::event::kind::PRESENCE_ARRIVED).actor("sam"))
            .unwrap();

        let response = app.oneshot(get("/api/roster")).await.unwrap();
        assert_eq!(json(response).await, serde_json::json!(["sam"]));
    }

    #[tokio::test]
    async fn get_apps_lists_the_registry_and_nothing_current() {
        let (app, _, _) = app();
        let body = json(app.oneshot(get("/api/apps")).await.unwrap()).await;

        assert_eq!(body["apps"][0]["id"], "kodi");
        assert_eq!(body["apps"][0]["name"], "Kodi");
        assert_eq!(body["current"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn launching_an_app_returns_no_content_and_updates_current() {
        let (app, _, launcher) = app();
        let response = app
            .oneshot(post_empty("/api/apps/kodi/launch"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(launcher.current(), Some("kodi".to_string()));
    }

    #[tokio::test]
    async fn launching_an_unknown_app_is_not_found() {
        let (app, _, _) = app();
        let response = app
            .oneshot(post_empty("/api/apps/nintendo64/launch"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn quitting_returns_no_content_and_clears_current() {
        let (app, _, launcher) = app();
        launcher.launch("kodi").unwrap();

        let response = app.oneshot(post_empty("/api/apps/quit")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(launcher.current(), None);
    }
}
