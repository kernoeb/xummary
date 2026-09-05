// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Xummary",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "Xummary", path: "Sources/Xummary")
    ]
)
