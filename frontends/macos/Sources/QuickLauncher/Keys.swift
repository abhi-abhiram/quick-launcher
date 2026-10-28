import AppKit
import Carbon.HIToolbox
import QuickLauncherKit

/// Translates between AppKit's and Carbon's keys and ``KeyPress``.
enum Keys {
    private static let named: [Int: Key] = [
        kVK_Space: .space, kVK_Tab: .tab, kVK_Return: .enter, kVK_ANSI_KeypadEnter: .enter,
        kVK_Delete: .backspace, kVK_ForwardDelete: .delete, kVK_Help: .insert,
        kVK_Home: .home, kVK_End: .end,
        kVK_F1: .function(1), kVK_F2: .function(2), kVK_F3: .function(3), kVK_F4: .function(4),
        kVK_F5: .function(5), kVK_F6: .function(6), kVK_F7: .function(7), kVK_F8: .function(8),
        kVK_F9: .function(9), kVK_F10: .function(10), kVK_F11: .function(11), kVK_F12: .function(12),
    ]

    /// Letters and digits on an ANSI keyboard, for the hotkey, which Carbon
    /// takes as a key code.
    private static let characters: [Character: Int] = [
        "a": kVK_ANSI_A, "b": kVK_ANSI_B, "c": kVK_ANSI_C, "d": kVK_ANSI_D, "e": kVK_ANSI_E,
        "f": kVK_ANSI_F, "g": kVK_ANSI_G, "h": kVK_ANSI_H, "i": kVK_ANSI_I, "j": kVK_ANSI_J,
        "k": kVK_ANSI_K, "l": kVK_ANSI_L, "m": kVK_ANSI_M, "n": kVK_ANSI_N, "o": kVK_ANSI_O,
        "p": kVK_ANSI_P, "q": kVK_ANSI_Q, "r": kVK_ANSI_R, "s": kVK_ANSI_S, "t": kVK_ANSI_T,
        "u": kVK_ANSI_U, "v": kVK_ANSI_V, "w": kVK_ANSI_W, "x": kVK_ANSI_X, "y": kVK_ANSI_Y,
        "z": kVK_ANSI_Z, "0": kVK_ANSI_0, "1": kVK_ANSI_1, "2": kVK_ANSI_2, "3": kVK_ANSI_3,
        "4": kVK_ANSI_4, "5": kVK_ANSI_5, "6": kVK_ANSI_6, "7": kVK_ANSI_7, "8": kVK_ANSI_8,
        "9": kVK_ANSI_9,
    ]

    /// The key a key-down event pressed, read with the current layout, or
    /// `nil` for one ``Key`` doesn't name.
    static func press(_ event: NSEvent) -> KeyPress? {
        let modifiers = self.modifiers(event.modifierFlags)
        if let key = named[Int(event.keyCode)] {
            return KeyPress(key, modifiers)
        }
        guard let typed = event.characters(byApplyingModifiers: [])?.lowercased(), typed.count == 1,
            let char = typed.first, char.isASCII, char.isLetter || char.isNumber
        else { return nil }
        return KeyPress(.character(char), modifiers)
    }

    static func modifiers(_ flags: NSEvent.ModifierFlags) -> Modifiers {
        var modifiers: Modifiers = []
        if flags.contains(.control) { modifiers.insert(.control) }
        if flags.contains(.shift) { modifiers.insert(.shift) }
        if flags.contains(.option) { modifiers.insert(.option) }
        if flags.contains(.command) { modifiers.insert(.command) }
        return modifiers
    }

    /// Carbon's key code for `key`.
    static func keyCode(_ key: Key) -> UInt32? {
        if case let .character(char) = key {
            return characters[char].map(UInt32.init)
        }
        // Keypad Enter and Return both read as `.enter`; the hotkey means Return.
        if key == .enter { return UInt32(kVK_Return) }
        return named.first(where: { $0.value == key }).map { UInt32($0.key) }
    }

    /// Carbon's modifier mask.
    static func carbonModifiers(_ modifiers: Modifiers) -> UInt32 {
        var mask = 0
        if modifiers.contains(.control) { mask |= controlKey }
        if modifiers.contains(.shift) { mask |= shiftKey }
        if modifiers.contains(.option) { mask |= optionKey }
        if modifiers.contains(.command) { mask |= cmdKey }
        return UInt32(mask)
    }
}

/// A global hotkey, through Carbon's `RegisterEventHotKey`, which needs no
/// accessibility permission.
final class HotKey {
    private let action: () -> Void
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?

    /// Registers `press`; `nil` when its key has no key code or another app
    /// already holds the chord.
    init?(_ press: KeyPress, action: @escaping () -> Void) {
        self.action = action
        guard let code = Keys.keyCode(press.key) else { return nil }
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let installed = InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData in
                guard let userData else { return OSStatus(eventNotHandledErr) }
                Unmanaged<HotKey>.fromOpaque(userData).takeUnretainedValue().action()
                return noErr
            },
            1, &spec, Unmanaged.passUnretained(self).toOpaque(), &handler)
        guard installed == noErr else { return nil }
        let id = EventHotKeyID(signature: OSType(0x514C_4E43), id: 1)  // "QLNC"
        let registered = RegisterEventHotKey(
            code, Keys.carbonModifiers(press.modifiers), id, GetApplicationEventTarget(), 0, &hotKey)
        guard registered == noErr else { return nil }
    }

    deinit {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let handler { RemoveEventHandler(handler) }
    }
}
