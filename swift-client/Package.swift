// swift-tools-version: 5.10

import PackageDescription

let package = Package(
    name: "FocusAgent",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(name: "FocusAgent", targets: ["FocusAgent"])
    ],
    targets: [
        .executableTarget(
            name: "FocusAgent",
            path: "Sources/FocusAgent"
        ),
        .testTarget(
            name: "FocusAgentTests",
            dependencies: ["FocusAgent"],
            path: "Tests/FocusAgentTests"
        )
    ]
)

