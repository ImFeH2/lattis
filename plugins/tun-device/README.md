# Tauri Plugin tun-device

This plugin gives the Tauri app shell access to platform VPN/TUN setup.

## Android

Android uses `VpnService` to create a TUN interface and returns the detached file descriptor to Rust. The Rust side wraps that descriptor as `lattis_core::TunDevice`.

## iOS

iOS Packet Tunnel traffic is owned by a `NEPacketTunnelProvider` Network Extension, not by the main app process. The plugin therefore manages the Packet Tunnel configuration from the Tauri app:

- create or update the `NETunnelProviderManager`
- start the Packet Tunnel
- stop the Packet Tunnel
- read the Packet Tunnel status

The app must include a Packet Tunnel Extension target whose bundle identifier defaults to:

```text
<app bundle identifier>.PacketTunnel
```

Packet IO must be implemented inside that extension with `NEPacketTunnelProvider.packetFlow`.
