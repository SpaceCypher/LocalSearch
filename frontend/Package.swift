// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "LocalSearch",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .executable(
            name: "LocalSearch",
            targets: ["LocalSearch"]
        )
    ],
    dependencies: [
        // Add dependencies here as needed
    ],
    targets: [
        .executableTarget(
            name: "LocalSearch",
            dependencies: [],
            path: "Sources"
        ),
        .testTarget(
            name: "LocalSearchTests",
            dependencies: ["LocalSearch"],
            path: "Tests/LocalSearchTests"
        )
    ]
)
