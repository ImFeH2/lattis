use anyhow::Result;
use reqwest::Url;

use super::coordinator::CoordinatorClient;
use super::runtime::Device;

const DEFAULT_COORDINATOR_URL: &str = "http://127.0.0.1:52170";
const DEFAULT_INTERFACE_NAME: &str = "lattis0";

pub struct DeviceBuilder {
    coordinator_url: Url,
    interface_name: String,
    listen_port: u16,
}

impl Device {
    pub fn builder() -> DeviceBuilder {
        DeviceBuilder {
            coordinator_url: Url::parse(DEFAULT_COORDINATOR_URL).expect("default URL is valid"),
            interface_name: DEFAULT_INTERFACE_NAME.to_string(),
            listen_port: 52171,
        }
    }
}

impl DeviceBuilder {
    pub fn coordinator_url(mut self, url: Url) -> Self {
        self.coordinator_url = url;
        self
    }

    pub fn interface_name(mut self, name: &str) -> Self {
        self.interface_name = name.to_string();
        self
    }

    pub fn listen_port(mut self, port: u16) -> Self {
        self.listen_port = port;
        self
    }

    pub async fn start(self) -> Result<Device> {
        Device::start(
            self.interface_name,
            self.listen_port,
            CoordinatorClient::new(self.coordinator_url),
        )
        .await
    }
}
