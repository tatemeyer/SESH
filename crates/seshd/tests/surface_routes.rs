//! The surface bundle is served on every path the phone and TV use.
//!
//! This exists because it was broken and nothing noticed. Arc 1's surface only
//! ever lived at `/`, so `not_found_service` — which serves the fallback body
//! but forces the status to **404** — looked correct for a year of use. D9
//! then built the phone on the assumption that unknown paths serve
//! `index.html`, and `/join` began returning a 404 carrying a perfectly good
//! page. Browsers render that, so it would have limped along in the room while
//! every status-code check called the surface broken.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use seshd::api::surface_service;
use tower::ServiceExt;

/// A throwaway bundle shaped like `surfaces/dist`.
fn bundle() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<div id=app></div>").unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets").join("index.js"), "// bundle").unwrap();
    dir
}

async fn get(dir: &std::path::Path, path: &str) -> (StatusCode, String) {
    let response = surface_service(dir)
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();

    use http_body_util::BodyExt;
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn the_tv_gets_the_bundle_at_the_root() {
    let dir = bundle();
    let (status, body) = get(dir.path(), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("id=app"), "got {body:?}");
}

#[tokio::test]
async fn a_real_asset_is_served_as_itself() {
    let dir = bundle();
    let (status, body) = get(dir.path(), "/assets/index.js").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "// bundle");
}

// The two the phone actually uses. Both are handled entirely in the browser,
// so the server has nothing at those paths and must say so with a 200.
#[tokio::test]
async fn the_phone_routes_get_the_bundle_with_a_200() {
    let dir = bundle();

    for path in ["/join", "/join?c=abc123", "/phone"] {
        let (status, body) = get(dir.path(), path).await;
        assert_eq!(status, StatusCode::OK, "{path} should serve the surface");
        assert!(body.contains("id=app"), "{path} served {body:?}");
    }
}

#[tokio::test]
async fn an_unknown_path_still_lands_on_the_surface() {
    let dir = bundle();
    let (status, _) = get(dir.path(), "/whatever").await;
    assert_eq!(status, StatusCode::OK);
}

// A missing bundle is a broken install, not a page. Serving a 200 with an
// empty body would make `deploy/build.sh` never having run look like a
// surface bug.
#[tokio::test]
async fn a_missing_bundle_is_not_dressed_up_as_a_page() {
    let dir = tempfile::tempdir().unwrap();
    let (status, _) = get(dir.path(), "/join").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
