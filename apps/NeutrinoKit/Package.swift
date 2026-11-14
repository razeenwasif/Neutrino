// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "NeutrinoKit",
    platforms: [.macOS(.v15), .iOS(.v18)],
    products: [
        .library(name: "NeutrinoKit", targets: ["NeutrinoKit"]),
    ],
    targets: [
        .target(name: "NeutrinoKit"),
        .testTarget(name: "NeutrinoKitTests", dependencies: ["NeutrinoKit"]),
    ]
)
