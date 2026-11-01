import Foundation

/// How the app draws a mode: its tint, the SF Symbol it shows where the
/// mode's own icon can't draw, and whether its labels are code. The
/// bundled modes have their own; any other mode gets a tint picked from
/// its id, so it keeps the same one every time.
public struct ModeStyle: Equatable {
    /// `0xRRGGBB`, or `nil` for the system accent color.
    public var tint: UInt32?
    public var symbol: String
    /// Labels and the typed text are commands: drawn in a monospaced font.
    public var monospaced: Bool

    public init(tint: UInt32?, symbol: String, monospaced: Bool = false) {
        self.tint = tint
        self.symbol = symbol
        self.monospaced = monospaced
    }

    /// The plain search.
    public static let search = ModeStyle(tint: nil, symbol: "square.grid.2x2")

    static let bundled: [String: ModeStyle] = [
        "apps": ModeStyle(tint: 0x8E8E93, symbol: "square.grid.2x2"),
        "projects": ModeStyle(tint: 0x0A84FF, symbol: "folder.fill"),
        "terminal": ModeStyle(tint: 0x30D158, symbol: "terminal", monospaced: true),
        "pi": ModeStyle(tint: 0xBF5AF2, symbol: "sparkle"),
    ]

    /// Orange, pink, teal, indigo, yellow: apart from the bundled modes'.
    static let others: [UInt32] = [0xFF9F0A, 0xFF375F, 0x40C8E0, 0x5E5CE6, 0xFFD60A]

    public static func of(_ mode: Mode?) -> ModeStyle {
        guard let mode else { return search }
        if let style = bundled[mode.id] { return style }
        // A stable pick: Swift's `hashValue` changes between launches.
        let sum = mode.id.unicodeScalars.reduce(0) { $0 &+ Int($1.value) }
        return ModeStyle(tint: others[sum % others.count], symbol: "circle.hexagongrid.fill")
    }
}

/// A row's detail as a path's folders, for drawing as breadcrumbs:
/// `~/Projects/ql` is `["~", "Projects", "ql"]`, and `/System/Applications`
/// is `["System", "Applications"]`. `nil` for a detail that isn't a path.
public enum Breadcrumbs {
    public static func split(_ detail: String) -> [String]? {
        guard detail.hasPrefix("~") || detail.hasPrefix("/") else { return nil }
        let parts = detail.split(separator: "/").map(String.init)
        return parts.isEmpty ? nil : parts
    }
}

/// One key hint in the footer: the keys that run an action, and its title.
public struct KeyHint: Equatable {
    public var keys: [String]
    public var title: String

    public init(keys: [String], title: String) {
        self.keys = keys
        self.title = title
    }
}
