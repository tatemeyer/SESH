//! The live event feed.
//!
//! Every surface — the TV and, from Arc 3, phones — holds one of these
//! sockets open and re-renders from it. Clients that fall behind the
//! broadcast backlog are dropped and expected to reconnect and catch up
//! via `GET /api/events?after=<last_id>`.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;

use super::AppState;

/// `GET /ws` — upgrade to a live event feed.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let events = state.room.subscribe();
    ws.on_upgrade(move |socket| pump(socket, events))
}

async fn pump(
    mut socket: WebSocket,
    mut events: tokio::sync::broadcast::Receiver<crate::event::Event>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                let text = match serde_json::to_string(&event) {
                    Ok(text) => text,
                    Err(error) => {
                        tracing::error!(
                            %error,
                            event_id = event.id,
                            "event failed to serialize; dropping"
                        );
                        continue;
                    }
                };
                if socket.send(Message::Text(text)).await.is_err() {
                    return; // client hung up
                }
            }
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "surface fell behind the event feed");
            }
            Err(RecvError::Closed) => return,
        }
    }
}
