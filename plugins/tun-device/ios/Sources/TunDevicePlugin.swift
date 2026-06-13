import Foundation
import NetworkExtension
import Tauri

struct PacketTunnelArgs: Decodable {
    let name: String
    let providerBundleIdentifier: String?
    let addresses: [String]
    let routes: [String]
}

struct PacketTunnelStatusPayload: Encodable {
    let state: String
}

struct PacketTunnelProviderArgs: Decodable {
    let providerBundleIdentifier: String?
}

enum TunDevicePluginError: LocalizedError {
    case missingBundleIdentifier
    case invalidProviderBundleIdentifier

    var errorDescription: String? {
        switch self {
        case .missingBundleIdentifier:
            return "App bundle identifier is not available"
        case .invalidProviderBundleIdentifier:
            return "Packet Tunnel provider bundle identifier is empty"
        }
    }
}

class TunDevicePlugin: Plugin {
    @objc public func startPacketTunnel(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(PacketTunnelArgs.self)
            let providerBundleIdentifier = try resolveProviderBundleIdentifier(args.providerBundleIdentifier)

            loadOrCreateManager(providerBundleIdentifier: providerBundleIdentifier) { result in
                switch result {
                case .success(let manager):
                    self.configure(manager, args: args, providerBundleIdentifier: providerBundleIdentifier)
                    self.saveAndStart(manager, invoke: invoke)
                case .failure(let error):
                    invoke.reject(error.localizedDescription)
                }
            }
        } catch {
            invoke.reject(error.localizedDescription)
        }
    }

    @objc public func stopPacketTunnel(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(PacketTunnelProviderArgs.self)
            let providerBundleIdentifier = try resolveProviderBundleIdentifier(args.providerBundleIdentifier)

            loadManager(providerBundleIdentifier: providerBundleIdentifier) { result in
                switch result {
                case .success(let manager):
                    manager?.connection.stopVPNTunnel()
                    invoke.resolve()
                case .failure(let error):
                    invoke.reject(error.localizedDescription)
                }
            }
        } catch {
            invoke.reject(error.localizedDescription)
        }
    }

    @objc public func packetTunnelStatus(_ invoke: Invoke) {
        do {
            let args = try invoke.parseArgs(PacketTunnelProviderArgs.self)
            let providerBundleIdentifier = try resolveProviderBundleIdentifier(args.providerBundleIdentifier)

            loadManager(providerBundleIdentifier: providerBundleIdentifier) { result in
                switch result {
                case .success(let manager):
                    let state = manager.map { self.stateName($0.connection.status) } ?? "notConfigured"
                    invoke.resolve(PacketTunnelStatusPayload(state: state))
                case .failure(let error):
                    invoke.reject(error.localizedDescription)
                }
            }
        } catch {
            invoke.reject(error.localizedDescription)
        }
    }

    private func configure(
        _ manager: NETunnelProviderManager,
        args: PacketTunnelArgs,
        providerBundleIdentifier: String
    ) {
        let protocolConfiguration = NETunnelProviderProtocol()
        protocolConfiguration.providerBundleIdentifier = providerBundleIdentifier
        protocolConfiguration.serverAddress = args.name
        protocolConfiguration.providerConfiguration = [
            "name": args.name,
            "addresses": args.addresses,
            "routes": args.routes,
        ]

        manager.localizedDescription = args.name
        manager.protocolConfiguration = protocolConfiguration
        manager.isEnabled = true
    }

    private func saveAndStart(_ manager: NETunnelProviderManager, invoke: Invoke) {
        manager.saveToPreferences { error in
            if let error = error {
                invoke.reject(error.localizedDescription)
                return
            }

            manager.loadFromPreferences { error in
                if let error = error {
                    invoke.reject(error.localizedDescription)
                    return
                }

                do {
                    try manager.connection.startVPNTunnel()
                    invoke.resolve()
                } catch {
                    invoke.reject(error.localizedDescription)
                }
            }
        }
    }

    private func loadOrCreateManager(
        providerBundleIdentifier: String,
        completion: @escaping (Result<NETunnelProviderManager, Error>) -> Void
    ) {
        loadManager(providerBundleIdentifier: providerBundleIdentifier) { result in
            switch result {
            case .success(let manager):
                completion(.success(manager ?? NETunnelProviderManager()))
            case .failure(let error):
                completion(.failure(error))
            }
        }
    }

    private func loadManager(
        providerBundleIdentifier: String,
        completion: @escaping (Result<NETunnelProviderManager?, Error>) -> Void
    ) {
        NETunnelProviderManager.loadAllFromPreferences { managers, error in
            if let error = error {
                completion(.failure(error))
                return
            }

            let manager = managers?.first { manager in
                guard let tunnelProtocol = manager.protocolConfiguration as? NETunnelProviderProtocol else {
                    return false
                }

                return tunnelProtocol.providerBundleIdentifier == providerBundleIdentifier
            }

            completion(.success(manager))
        }
    }

    private func resolveProviderBundleIdentifier(_ value: String?) throws -> String {
        if let value = value {
            let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty {
                throw TunDevicePluginError.invalidProviderBundleIdentifier
            }

            return trimmed
        }

        guard let bundleIdentifier = Bundle.main.bundleIdentifier else {
            throw TunDevicePluginError.missingBundleIdentifier
        }

        return "\(bundleIdentifier).PacketTunnel"
    }

    private func stateName(_ status: NEVPNStatus) -> String {
        switch status {
        case .invalid:
            return "invalid"
        case .disconnected:
            return "disconnected"
        case .connecting:
            return "connecting"
        case .connected:
            return "connected"
        case .reasserting:
            return "reasserting"
        case .disconnecting:
            return "disconnecting"
        @unknown default:
            return "unknown"
        }
    }
}

@_cdecl("init_plugin_tun_device")
func initPlugin() -> Plugin {
    return TunDevicePlugin()
}
