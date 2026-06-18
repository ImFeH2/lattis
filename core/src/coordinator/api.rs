use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use std::{convert::Infallible, time::Duration};

use super::registry::Coordinator;
use crate::model::{DeviceID, PeerInfo, RegisterDeviceRequest, RegisterDeviceResponse};

struct CoordinatorApiError(anyhow::Error);

impl IntoResponse for CoordinatorApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.0.to_string()).into_response()
    }
}

impl<E> From<E> for CoordinatorApiError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

pub(crate) fn router(coordinator: Coordinator) -> Router {
    Router::new()
        .route("/devices/register", post(register_device))
        .route("/devices/{device_id}/peers", get(list_device_peers))
        .route(
            "/devices/{device_id}/peers/events",
            get(stream_device_peer_events),
        )
        .with_state(coordinator)
}

async fn register_device(
    State(coordinator): State<Coordinator>,
    Json(request): Json<RegisterDeviceRequest>,
) -> std::result::Result<Json<RegisterDeviceResponse>, CoordinatorApiError> {
    let response = coordinator.register(request).await?;

    Ok(Json(response))
}

async fn list_device_peers(
    State(coordinator): State<Coordinator>,
    Path(device_id): Path<DeviceID>,
) -> std::result::Result<Json<Vec<PeerInfo>>, CoordinatorApiError> {
    let peers = coordinator.peers_for(&device_id).await?;

    Ok(Json(peers))
}

async fn stream_device_peer_events(
    State(coordinator): State<Coordinator>,
    Path(device_id): Path<DeviceID>,
) -> std::result::Result<
    Sse<impl futures_core::Stream<Item = std::result::Result<Event, Infallible>>>,
    CoordinatorApiError,
> {
    let mut peer_events = coordinator.subscribe_peer_events();
    let peers = coordinator.peers_for(&device_id).await?;

    let stream = async_stream::stream! {
        match peers_event(&peers) {
            Ok(event) => yield Ok(event),
            Err(error) => {
                yield Ok(error_event(error));
                return;
            }
        }

        loop {
            match peer_events.recv().await {
                Ok(peer) => {
                    if peer.device_id == device_id {
                        continue;
                    }

                    match peer_event(&peer) {
                        Ok(event) => yield Ok(event),
                        Err(error) => {
                            yield Ok(error_event(error));
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    match coordinator.peers_for(&device_id).await {
                        Ok(peers) => match peers_event(&peers) {
                            Ok(event) => yield Ok(event),
                            Err(error) => {
                                yield Ok(error_event(error));
                                break;
                            }
                        },
                        Err(error) => {
                            yield Ok(error_event(error));
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

fn peers_event(peers: &[PeerInfo]) -> anyhow::Result<Event> {
    Ok(Event::default()
        .event("peers")
        .data(serde_json::to_string(peers)?))
}

fn peer_event(peer: &PeerInfo) -> anyhow::Result<Event> {
    Ok(Event::default()
        .event("peer")
        .data(serde_json::to_string(peer)?))
}

fn error_event(error: impl std::fmt::Display) -> Event {
    Event::default().event("error").data(error.to_string())
}
