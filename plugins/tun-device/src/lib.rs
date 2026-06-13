use anyhow::Result;
#[cfg(any(target_os = "android", target_os = "ios"))]
use anyhow::bail;
use lattis_core::{TunConfig, TunDevice};
#[cfg(any(target_os = "android", target_os = "ios"))]
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
#[cfg(any(target_os = "android", target_os = "ios"))]
use tauri::plugin::PluginHandle;
use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_tun_device);

pub trait TunDeviceExt<R: Runtime> {
    fn tun_device(&self) -> &TunDevicePlugin<R>;
}

impl<R: Runtime, T: Manager<R>> TunDeviceExt<R> for T {
    fn tun_device(&self) -> &TunDevicePlugin<R> {
        self.state::<TunDevicePlugin<R>>().inner()
    }
}

pub struct TunDevicePlugin<R: Runtime> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    handle: PluginHandle<R>,
    _runtime: PhantomData<fn() -> R>,
}

#[cfg(target_os = "android")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenRequest {
    name: String,
    addresses: Vec<String>,
    routes: Vec<String>,
}

#[cfg(target_os = "android")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenResponse {
    fd: i32,
}

#[cfg(target_os = "ios")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PacketTunnelRequest {
    name: String,
    provider_bundle_identifier: Option<String>,
    addresses: Vec<String>,
    routes: Vec<String>,
}

#[cfg(target_os = "ios")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PacketTunnelProviderRequest {
    provider_bundle_identifier: Option<String>,
}

#[cfg(target_os = "ios")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PacketTunnelStatus {
    pub state: PacketTunnelState,
}

#[cfg(target_os = "ios")]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PacketTunnelState {
    NotConfigured,
    Invalid,
    Disconnected,
    Connecting,
    Connected,
    Reasserting,
    Disconnecting,
    Unknown,
}

impl<R: Runtime> TunDevicePlugin<R> {
    #[cfg(target_os = "android")]
    pub async fn open(&self, config: TunConfig) -> Result<TunDevice> {
        use std::os::fd::{FromRawFd, OwnedFd};

        let request = OpenRequest {
            name: config.name,
            addresses: config
                .addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            routes: config
                .addresses
                .iter()
                .map(|address| address.trunc().to_string())
                .collect::<Vec<_>>(),
        };

        let response = self
            .handle
            .run_mobile_plugin_async::<OpenResponse>("open", request)
            .await?;

        if response.fd < 0 {
            bail!("Android VPN service returned an invalid TUN file descriptor");
        }

        let fd = unsafe { OwnedFd::from_raw_fd(response.fd) };
        TunDevice::from_owned_fd(fd)
    }

    #[cfg(target_os = "ios")]
    pub async fn open(&self, _config: TunConfig) -> Result<TunDevice> {
        bail!(
            "iOS packet tunnels run in a Network Extension and do not expose a TUN file descriptor to the app process"
        );
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub async fn open(&self, config: TunConfig) -> Result<TunDevice> {
        TunDevice::open(config)
    }

    #[cfg(target_os = "ios")]
    pub async fn start_packet_tunnel(&self, config: TunConfig) -> Result<()> {
        self.start_packet_tunnel_with_provider(config, None).await
    }

    #[cfg(target_os = "ios")]
    pub async fn start_packet_tunnel_with_provider(
        &self,
        config: TunConfig,
        provider_bundle_identifier: Option<String>,
    ) -> Result<()> {
        let request = PacketTunnelRequest {
            name: config.name,
            provider_bundle_identifier,
            addresses: config
                .addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            routes: config
                .addresses
                .iter()
                .map(|address| address.trunc().to_string())
                .collect::<Vec<_>>(),
        };

        self.handle
            .run_mobile_plugin_async::<()>("startPacketTunnel", request)
            .await?;

        Ok(())
    }

    #[cfg(target_os = "ios")]
    pub async fn stop_packet_tunnel(&self) -> Result<()> {
        self.stop_packet_tunnel_with_provider(None).await
    }

    #[cfg(target_os = "ios")]
    pub async fn stop_packet_tunnel_with_provider(
        &self,
        provider_bundle_identifier: Option<String>,
    ) -> Result<()> {
        let request = PacketTunnelProviderRequest {
            provider_bundle_identifier,
        };

        self.handle
            .run_mobile_plugin_async::<()>("stopPacketTunnel", request)
            .await?;

        Ok(())
    }

    #[cfg(target_os = "ios")]
    pub async fn packet_tunnel_status(&self) -> Result<PacketTunnelStatus> {
        self.packet_tunnel_status_with_provider(None).await
    }

    #[cfg(target_os = "ios")]
    pub async fn packet_tunnel_status_with_provider(
        &self,
        provider_bundle_identifier: Option<String>,
    ) -> Result<PacketTunnelStatus> {
        let request = PacketTunnelProviderRequest {
            provider_bundle_identifier,
        };

        Ok(self
            .handle
            .run_mobile_plugin_async::<PacketTunnelStatus>("packetTunnelStatus", request)
            .await?)
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("tun-device")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    api.register_android_plugin("im.feh2.tun.device", "TunDevicePlugin")?;
                app.manage(TunDevicePlugin {
                    handle,
                    _runtime: PhantomData,
                });
            }

            #[cfg(target_os = "ios")]
            {
                let handle = api.register_ios_plugin(init_plugin_tun_device)?;
                app.manage(TunDevicePlugin {
                    handle,
                    _runtime: PhantomData,
                });
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let _ = api;
                app.manage(TunDevicePlugin::<R> {
                    _runtime: PhantomData,
                });
            }

            Ok(())
        })
        .build()
}
