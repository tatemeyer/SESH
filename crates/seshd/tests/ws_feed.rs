//! The live event feed, exercised against a real server on a real port.

use std::sync::Arc;

use futures_util::StreamExt;
use seshd::api::{router_with_ws, AppState};
use seshd::config::AppSpec;
use seshd::event::NewEvent;
use seshd::launcher::platform::MockPlatform;
use seshd::launcher::Launcher;
use seshd::room::Room;
use seshd::store::Store;

async fn serve() -> (String, Arc<Room>) {
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
    });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("ws://{addr}/ws"), room)
}

#[tokio::test]
async fn a_connected_client_receives_events_recorded_after_it_connects() {
    let (url, room) = serve().await;
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
    let (url, room) = serve().await;
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
