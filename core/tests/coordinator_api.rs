mod support;

use anyhow::{Result, ensure};
use serde_json::Value;
use support::{TestCoordinator, device_id, endpoint, peer_device_id, public_key, register_request};

#[tokio::test]
async fn coordinator_registers_devices_and_lists_peers() -> Result<()> {
    let coordinator = TestCoordinator::start().await?;
    let client = reqwest::Client::new();
    let first_device_id = device_id();
    let second_device_id = device_id();

    let first: Value = client
        .post(coordinator.url("/devices/register")?)
        .json(&register_request(
            &first_device_id,
            public_key(1),
            vec![endpoint(1001)],
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    assert_eq!(peer_device_id(&first["device"])?, first_device_id);
    assert_eq!(first["peers"].as_array().unwrap().len(), 0);
    let first_address = single_virtual_address(&first["device"])?;
    ensure!(
        first_address.starts_with("100."),
        "expected Lattis virtual address, got {first_address}"
    );
    ensure!(
        first_address.ends_with("/32"),
        "expected /32 virtual address, got {first_address}"
    );

    let second: Value = client
        .post(coordinator.url("/devices/register")?)
        .json(&register_request(
            &second_device_id,
            public_key(2),
            vec![endpoint(1002)],
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    assert_eq!(peer_device_id(&second["device"])?, second_device_id);
    let second_peers = second["peers"].as_array().unwrap();
    assert_eq!(second_peers.len(), 1);
    assert_eq!(peer_device_id(&second_peers[0])?, first_device_id);

    let first_peers: Value = client
        .get(coordinator.url(&format!("/devices/{first_device_id}/peers"))?)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let first_peers = first_peers.as_array().unwrap();
    assert_eq!(first_peers.len(), 1);
    assert_eq!(peer_device_id(&first_peers[0])?, second_device_id);

    Ok(())
}

fn single_virtual_address(peer: &Value) -> Result<&str> {
    let addresses = peer["addresses"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("addresses is not an array"))?;
    ensure!(addresses.len() == 1, "expected one virtual address");

    addresses[0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("virtual address is not a string"))
}

#[tokio::test]
async fn coordinator_rejects_invalid_http_requests() -> Result<()> {
    let coordinator = TestCoordinator::start().await?;
    let client = reqwest::Client::new();
    let unknown_device_id = device_id();

    let empty_endpoints = client
        .post(coordinator.url("/devices/register")?)
        .json(&register_request(&device_id(), public_key(1), Vec::new()))
        .send()
        .await?;

    assert_eq!(empty_endpoints.status(), reqwest::StatusCode::BAD_REQUEST);

    let unknown_peer_list = client
        .get(coordinator.url(&format!("/devices/{unknown_device_id}/peers"))?)
        .send()
        .await?;

    assert_eq!(unknown_peer_list.status(), reqwest::StatusCode::BAD_REQUEST);

    Ok(())
}
