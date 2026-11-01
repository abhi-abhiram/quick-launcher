// swift-tools-version:5.9
// The macOS frontend for quick-launcher-core. `QuickLauncherKit` holds the
// frontend protocol and the launcher's state and has no AppKit in it, so its
// tests run anywhere Swift does; `QuickLauncher` is the menu bar app.
import PackageDescription

let package = Package(
    name: "QuickLauncher",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "QuickLauncher", targets: ["QuickLauncher"]),
    ],
    targets: [
        .target(name: "QuickLauncherKit"),
        .executableTarget(name: "QuickLauncher", dependencies: ["QuickLauncherKit"]),
        .testTarget(name: "QuickLauncherKitTests", dependencies: ["QuickLauncherKit"]),
    ]
)
