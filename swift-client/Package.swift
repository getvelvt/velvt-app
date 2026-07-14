// swift-tools-version: 5.10

import PackageDescription

let package = Package(
    name: "VelvtMac",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "Velvt", targets: ["VelvtMac"])
    ],
    targets: [
        .executableTarget(
            name: "VelvtMac",
            path: "Sources/VelvtMac"
        ),
        .testTarget(
            name: "VelvtMacTests",
            dependencies: ["VelvtMac"],
            path: "Tests/VelvtMacTests"
        )
    ]
)
	
	
