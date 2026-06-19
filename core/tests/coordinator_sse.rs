mod support;

use anyhow::{Result, bail};
use futures_util::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde_json::Value;
use std::time::Duration;
use support::{TestCoordinator, device_id, endpoint, peer_device_id, public_key, register_request};
use tokio::time::timeout;

#[tokio::test]
async fn coordinator_streams_initial_peers_and_incremental_updates() -> Result<()> {
    let coordinator = TestCoordinator::start().await?;
    let client = reqwest::Client::new();
    let first_device_id = device_id();
    let second_device_id = device_id();

    client
        .post(coordinator.url("/devices/register")?)
        .json(&register_request(
            &first_device_id,
            public_key(1),
            vec![endpoint(1001)],
        ))
        .send()
        .await?
        .error_for_status()?;

    let mut events =
        EventSource::get(coordinator.url(&format!("/devices/{first_device_id}/peers/events"))?);
    let (initial_event, initial_data) = next_message(&mut events).await?;

    assert_eq!(initial_event, "peers");
    let peers: Vec<Value> = serde_json::from_str(&initial_data)?;
    assert!(peers.is_empty());

    client
        .post(coordinator.url("/devices/register")?)
        .json(&register_request(
            &second_device_id,
            public_key(2),
            vec![endpoint(1002)],
        ))
        .send()
        .await?
        .error_for_status()?;

    let (update_event, update_data) = next_message(&mut events).await?;

    assert_eq!(update_event, "peer");
    let peer: Value = serde_json::from_str(&update_data)?;
    assert_eq!(peer_device_id(&peer)?, second_device_id);

    events.close();
    Ok(())
}

async fn next_message(events: &mut EventSource) -> Result<(String, String)> {
    timeout(Duration::from_secs(2), async {
        loop {
            match events.next().await {
                Some(Ok(Event::Open)) => {}
                Some(Ok(Event::Message(message))) => return Ok((message.event, message.data)),
                Some(Err(error)) => return Err(error.into()),
                None => bail!("event stream closed"),
            }
        }
    })
    .await?
}
