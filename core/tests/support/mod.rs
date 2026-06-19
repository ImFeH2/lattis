use anyhow::Result;
use lattis_core::Coordinator;
use reqwest::Url;
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

pub struct TestCoordinator {
    base_url: Url,
    task: JoinHandle<Result<()>>,
}

impl TestCoordinator {
    pub async fn start() -> Result<Self> {
        let coordinator = Coordinator::new();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move { coordinator.serve(listener).await });

        Ok(Self {
            base_url: Url::parse(&format!("http://{address}"))?,
            task,
        })
    }

    pub fn url(&self, path: &str) -> Result<Url> {
        Ok(self.base_url.join(path)?)
    }
}

impl Drop for TestCoordinator {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn device_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn endpoint(port: u16) -> String {
    SocketAddr::from(([192, 0, 2, 1], port)).to_string()
}

pub fn peer_device_id(peer: &Value) -> Result<&str> {
    peer["device_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("peer device_id is not a string"))
}

pub fn public_key(value: u8) -> Value {
    Value::Array(vec![Value::from(value); 32])
}

pub fn register_request(device_id: &str, public_key: Value, endpoints: Vec<String>) -> Value {
    json!({
        "device_id": device_id,
        "public_key": public_key,
        "endpoints": endpoints,
    })
}
