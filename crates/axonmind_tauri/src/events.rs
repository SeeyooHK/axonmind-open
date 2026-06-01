//! Forward EngineEvents to the Tauri frontend as typed events.
use axonmind_engine::events::EngineEvent;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::broadcast;

/// Spawn a background task that forwards every `EngineEvent` to the frontend.
/// Event name: `"axonmind://event"`. Payload: the event as JSON (tagged union via `type` field).
pub fn spawn_event_forwarder<R: Runtime + 'static>(
    app: AppHandle<R>,
    mut rx: broadcast::Receiver<EngineEvent>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    app.emit("axonmind://event", &event).ok();
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
