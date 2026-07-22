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
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.9.4")
    ],
    targets: [
        .executableTarget(
            name: "VelvtMac",
            dependencies: [
                .product(name: "Sparkle", package: "Sparkle")
            ],
            path: "Sources/VelvtMac"
        ),
        .testTarget(
            name: "VelvtMacTests",
            dependencies: ["VelvtMac"],
            path: "Tests/VelvtMacTests"
        )
    ]
)
	
	
