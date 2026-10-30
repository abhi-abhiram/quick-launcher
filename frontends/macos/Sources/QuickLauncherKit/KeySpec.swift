import Foundation

// Key chords: an action's suggested `key` (docs/protocol.md, *Frontend
// items*) and the app's hotkey setting, both written like `ctrl+shift+d`.

/// Modifier keys.
public struct Modifiers: OptionSet, Hashable {
    public let rawValue: Int
    public init(rawValue: Int) { self.rawValue = rawValue }

    public static let control = Modifiers(rawValue: 1 << 0)
    public static let shift = Modifiers(rawValue: 1 << 1)
    public static let option = Modifiers(rawValue: 1 << 2)
    public static let command = Modifiers(rawValue: 1 << 3)

    /// The modifiers that make a key a chord rather than typing.
    public static let chord: Modifiers = [.control, .option, .command]
}

/// A key, independent of the keyboard layout where it can be.
public enum Key: Hashable {
    /// A letter or digit, lowercase.
    case character(Character)
    case space, tab, enter, backspace, delete, insert, home, end
    /// F1 to F12.
    case function(Int)
}

/// A key with the modifiers held.
public struct KeyPress: Hashable {
    public var key: Key
    public var modifiers: Modifiers

    public init(_ key: Key, _ modifiers: Modifiers = []) {
        self.key = key
        self.modifiers = modifiers
    }
}

public enum KeySpec {
    private static let named: [String: Key] = [
        "delete": .delete, "del": .delete, "backspace": .backspace,
        "enter": .enter, "return": .enter, "space": .space, "tab": .tab,
        "insert": .insert, "home": .home, "end": .end,
    ]

    /// Reads a chord such as `ctrl+d`, `shift+delete` or `alt+space`.
    /// `ctrl`/`control`, `shift`, `alt`/`option`/`opt` and
    /// `super`/`meta`/`cmd`/`command` are the modifiers. `nil` when it names
    /// a key or modifier this frontend doesn't know.
    public static func parse(_ spec: String) -> KeyPress? {
        var parts = spec.lowercased().split(separator: "+", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
        guard let name = parts.popLast(), !name.isEmpty else { return nil }
        var modifiers: Modifiers = []
        for part in parts {
            switch part {
            case "ctrl", "control": modifiers.insert(.control)
            case "shift": modifiers.insert(.shift)
            case "alt", "option", "opt": modifiers.insert(.option)
            case "super", "meta", "cmd", "command": modifiers.insert(.command)
            default: return nil
            }
        }
        let key: Key
        if let namedKey = named[name] {
            key = namedKey
        } else if name.count == 1, let char = name.first, char.isASCII,
            char.isLetter || char.isNumber
        {
            key = .character(char)
        } else if name.hasPrefix("f"), let number = Int(name.dropFirst()), (1...12).contains(number),
            name.count <= 3
        {
            key = .function(number)
        } else {
            return nil
        }
        return KeyPress(key, modifiers)
    }

    /// An action's `key` as this frontend binds it: like ``parse(_:)``,
    /// but `nil` for a key that types, moves or runs the default action
    /// without Ctrl, Option or Command, so an action can't take it.
    public static func actionKey(_ spec: String) -> KeyPress? {
        guard let press = parse(spec) else { return nil }
        if press.modifiers.isDisjoint(with: .chord) {
            switch press.key {
            case .character, .space, .backspace, .tab, .enter: return nil
            default: break
            }
        }
        return press
    }

    /// How the app shows a chord, the macOS way: `⌃⇧D`.
    public static func symbols(_ press: KeyPress) -> String {
        var text = ""
        if press.modifiers.contains(.control) { text += "⌃" }
        if press.modifiers.contains(.option) { text += "⌥" }
        if press.modifiers.contains(.shift) { text += "⇧" }
        if press.modifiers.contains(.command) { text += "⌘" }
        switch press.key {
        case let .character(char): text += char.uppercased()
        case .space: text += "Space"
        case .tab: text += "⇥"
        case .enter: text += "↩"
        case .backspace: text += "⌫"
        case .delete: text += "⌦"
        case .insert: text += "Ins"
        case .home: text += "↖"
        case .end: text += "↘"
        case let .function(number): text += "F\(number)"
        }
        return text
    }
}
