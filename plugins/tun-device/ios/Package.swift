// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "tauri-plugin-tun-device",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v13),
    ],
    products: [
        .library(
            name: "tauri-plugin-tun-device",
            type: .static,
            targets: ["tauri-plugin-tun-device"]),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api")
    ],
    targets: [
        .target(
            name: "tauri-plugin-tun-device",
            dependencies: [
                .byName(name: "Tauri")
            ],
            path: "Sources")
    ]
)
