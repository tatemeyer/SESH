//! The live event feed, exercised against a real server on a real port.

use std::sync::Arc;

use futures_util::StreamExt;
use seshd::api::{router_with_ws, AppState};
use seshd::config::AppSpec;
use seshd::event::NewEvent;
use seshd::join::JoinCodes;
use seshd::launcher::platform::MockPlatform;
use seshd::launcher::Launcher;
use seshd::presence::Presence;
use seshd::room::Room;
use seshd::store::Store;

async fn serve() -> (String, String, Arc<Room>) {
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
    let app = router_with_ws(AppState {
        room: room.clone(),
        launcher,
        join: Arc::new(JoinCodes::new()),
        presence: Arc::new(Presence::new()),
        join_base: "http://pi.test:7373".into(),
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("ws://{addr}/ws"), addr.to_string(), room)
}

/// Issues a bare HTTP/1.1 GET over a raw TCP stream and returns the full
/// response text. No HTTP client dependency is added for this — the
/// request is minimal enough to hand-write, and `Connection: close`
/// lets us read to EOF instead of parsing `Content-Length`.
async fn http_get(addr: &str, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_string(&mut response),
    )
    .await
    .expect("timed out reading the HTTP response")
    .unwrap();
    response
}

#[tokio::test]
async fn the_http_api_is_still_reachable_through_router_with_ws() {
    let (_, addr, _) = serve().await;

    let response = http_get(&addr, "/api/apps").await;

    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("kodi"));
}

#[tokio::test]
async fn a_connected_client_receives_events_recorded_after_it_connects() {
    let (url, _, room) = serve().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    room.record(NewEvent::new("app.launched").subject("kodi"))
        .unwrap();

    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("timed out waiting for an event")
        .expect("socket closed")
        .unwrap();

    let event: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert_eq!(event["kind"], "app.launched");
    assert_eq!(event["subject"], "kodi");
}

#[tokio::test]
async fn two_clients_both_receive_the_same_event() {
    let (url, _, room) = serve().await;
    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    room.record(NewEvent::new("moment.captured").subject("clip-1"))
        .unwrap();

    for socket in [&mut a, &mut b] {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("timed out")
            .expect("socket closed")
            .unwrap();
        let event: serde_json::Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(event["subject"], "clip-1");
    }
}
