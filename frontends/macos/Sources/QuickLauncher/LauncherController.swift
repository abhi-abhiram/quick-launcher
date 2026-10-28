import AppKit
import Carbon.HIToolbox
import QuickLauncherKit
import SwiftUI

/// Sizes shared by the view and the panel, which is resized to fit the
/// rows rather than measuring the view.
enum Layout {
    static let width: CGFloat = 640
    static let padding: CGFloat = 10
    static let header: CGFloat = 44
    static let spacing: CGFloat = 6
    static let errorHeight: CGFloat = 18
    static let rowHeight: CGFloat = 46
    static let rowSpacing: CGFloat = 2
    static let maxVisibleRows = 8
    static let cornerRadius: CGFloat = 14

    static func listHeight(rows: Int) -> CGFloat {
        guard rows > 0 else { return rowHeight * 2 }
        let shown = CGFloat(min(rows, maxVisibleRows))
        return shown * rowHeight + (shown - 1) * rowSpacing
    }

    static func height(rows: Int, hasError: Bool) -> CGFloat {
        padding * 2 + header + spacing + (hasError ? errorHeight + spacing : 0) + listHeight(rows: rows)
    }
}

/// A borderless panel that takes the keyboard without activating the app,
/// so the app the user was in stays frontmost underneath, as with Spotlight.
final class LauncherPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

/// Owns the panel and the core, and turns what the user does into frontend
/// protocol messages through ``Launcher``.
final class LauncherController: NSObject, ObservableObject, NSWindowDelegate {
    @Published private(set) var launcher = Launcher()
    /// Bumped on every open, so the search field takes the focus again.
    @Published private(set) var focusToken = 0

    private let core: CoreProcess
    private var panel: LauncherPanel!
    private var keyMonitor: Any?
    /// The core speaks another protocol major. It has exited then, and isn't
    /// started again until the app restarts.
    private var incompatible = false
    /// The pointer's last position, so a row is selected by hover only when
    /// the pointer moves, not when the list scrolls under it.
    private var lastPointer = NSPoint(x: -1, y: -1)

    init(core: CoreProcess) {
        self.core = core
        super.init()
        panel = makePanel()
        core.onMessage = { [weak self] message in self?.receive(message) }
        core.onExit = { [weak self] status in self?.coreExited(status) }
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.window === self.panel else { return event }
            return self.handleKey(event) ? nil : event
        }
    }

    private func makePanel() -> LauncherPanel {
        let panel = LauncherPanel(
            contentRect: NSRect(x: 0, y: 0, width: Layout.width, height: Layout.height(rows: 0, hasError: false)),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: true)
        panel.isFloatingPanel = true
        panel.level = .modalPanel
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.hidesOnDeactivate = false
        panel.isMovable = false
        panel.backgroundColor = .clear
        panel.isOpaque = false
        panel.hasShadow = true
        panel.delegate = self
        let host = NSHostingView(rootView: LauncherView(controller: self))
        host.autoresizingMask = [.width, .height]
        panel.contentView = host
        return panel
    }

    var isShown: Bool { panel.isVisible }

    // MARK: - Showing and hiding

    func toggle() {
        if panel.isVisible { hide() } else { show() }
    }

    /// Shows the panel on the screen with the pointer, optionally straight
    /// into a mode (a plugin id).
    func show(mode: String? = nil) {
        if incompatible {
            alert("The launcher's core speaks a different protocol version. Run install.py again.")
            return
        }
        do {
            if !core.isRunning {
                launcher.coreStarted()
                try core.start(hello: .hello(client: "macos", clientVersion: AppInfo.version, maxItems: Launcher.maxItems))
            }
        } catch {
            alert("Couldn't start \(core.executable.path): \(error.localizedDescription). Run install.py to install the core.")
            return
        }
        if panel.isVisible, let close = launcher.close() {
            core.send(close)
        }
        core.send(launcher.open(mode: mode))
        lastPointer = NSEvent.mouseLocation
        place()
        panel.makeKeyAndOrderFront(nil)
        if !panel.isKeyWindow {
            NSApp.activate(ignoringOtherApps: true)
            panel.makeKey()
        }
        focusToken += 1
    }

    /// Hides the panel. Hiding by any route (Escape, a click elsewhere, the
    /// hotkey) ends the session, unless an action already ended it.
    func hide() {
        dismiss()
        if let close = launcher.close() {
            core.send(close)
        }
    }

    /// Takes the panel off screen. If showing it had to activate the app,
    /// the app the user was in gets the keyboard back.
    private func dismiss() {
        panel.orderOut(nil)
        if NSApp.isActive { NSApp.hide(nil) }
    }

    func windowDidResignKey(_ notification: Notification) {
        if panel.isVisible { hide() }
    }

    /// Places the panel a fifth of the way down the pointer's screen, with
    /// its top edge fixed so rows grow and shrink downward.
    private func place() {
        let pointer = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(pointer, $0.frame, false) } ?? NSScreen.main
        guard let frame = screen?.visibleFrame else { return }
        let height = Layout.height(rows: launcher.items.count, hasError: launcher.error != nil)
        let top = frame.maxY - frame.height * 0.2
        panel.setFrame(
            NSRect(x: frame.midX - Layout.width / 2, y: top - height, width: Layout.width, height: height),
            display: true)
    }

    /// Fits the panel to the rows, keeping its top edge where it is.
    private func resize() {
        let height = Layout.height(rows: launcher.items.count, hasError: launcher.error != nil)
        var frame = panel.frame
        guard frame.height != height else { return }
        frame.origin.y = frame.maxY - height
        frame.size.height = height
        panel.setFrame(frame, display: true)
    }

    // MARK: - The core

    /// Ends the core, which stops its plugins.
    func stopCore() {
        core.stop()
    }

    private func receive(_ message: Incoming) {
        switch message {
        case let .ready(version, coreVersion):
            if version != protocolVersion {
                incompatible = true
                launcher.sessionEnded()
                dismiss()
                alert(
                    "The launcher's core (\(coreVersion)) speaks protocol version \(version), this app speaks "
                        + "\(protocolVersion). Run install.py again to install matching versions.")
            }
        case let .state(state):
            launcher.receive(state)
            if panel.isVisible { resize() }
        case .done:
            launcher.sessionEnded()
            dismiss()
        case .unknown:
            break
        }
    }

    private func coreExited(_ status: Int32) {
        if status != 0 && !incompatible {
            NSLog("quick-launcher: the core exited with status \(status); it starts again on the next open")
        }
        guard launcher.inSession else { return }
        launcher.sessionEnded()
        dismiss()
        if !incompatible {
            alert("The launcher's core stopped. Is it installed? Run install.py.")
        }
    }

    // MARK: - Input

    func setText(_ text: String) {
        core.send(launcher.setText(text))
    }

    func activate(_ index: Int, action: Int) {
        launcher.select(index)
        if let message = launcher.activate(index: index, action: action) {
            core.send(message)
        }
    }

    /// Selects the row under the pointer, when the pointer moved there.
    func hover(_ index: Int) {
        let pointer = NSEvent.mouseLocation
        guard pointer != lastPointer else { return }
        lastPointer = pointer
        launcher.select(index)
    }

    /// The launcher's own keys, then the selected item's action keys. The
    /// launcher's editing and navigation keys always win over an action's
    /// `key`. Returns whether the event was used.
    private func handleKey(_ event: NSEvent) -> Bool {
        // Keys that confirm or cancel an input method's composition are
        // the input method's.
        if let editor = panel.firstResponder as? NSTextView, editor.hasMarkedText() {
            return false
        }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let press = Keys.press(event)
        switch Int(event.keyCode) {
        case kVK_Escape:
            if launcher.text.isEmpty { hide() } else { setText("") }
            return true
        case kVK_UpArrow:
            launcher.move(-1)
            return true
        case kVK_DownArrow:
            launcher.move(1)
            return true
        case kVK_PageUp:
            launcher.move(-Layout.maxVisibleRows)
            return true
        case kVK_PageDown:
            launcher.move(Layout.maxVisibleRows)
            return true
        case kVK_Return, kVK_ANSI_KeypadEnter:
            let alternate = !flags.isDisjoint(with: [.shift, .control])
            activate(launcher.selectedIndex, action: alternate ? 1 : 0)
            return true
        case kVK_Tab:
            core.send(launcher.complete())
            return true
        case kVK_Delete where launcher.text.isEmpty:
            if let leave = launcher.leaveMode() {
                core.send(leave)
                return true
            }
            return false
        default:
            break
        }
        if let press, press.modifiers == [.control], case let .character(char) = press.key {
            switch char {
            case "n", "j":
                launcher.move(1)
                return true
            case "p", "k":
                launcher.move(-1)
                return true
            default:
                break
            }
        }
        if let press, press.modifiers == [.command], case let .character(char) = press.key,
            let action = Self.editActions[char]
        {
            // No Edit menu reaches a panel of an app that isn't active, so
            // the standard editing chords are sent from here.
            return NSApp.sendAction(action, to: nil, from: nil)
        }
        if let press, let message = launcher.activate(byKey: press) {
            core.send(message)
            return true
        }
        return false
    }

    private static let editActions: [Character: Selector] = [
        "x": #selector(NSText.cut(_:)),
        "c": #selector(NSText.copy(_:)),
        "v": #selector(NSText.paste(_:)),
        "a": #selector(NSText.selectAll(_:)),
        "z": Selector(("undo:")),
    ]

    private func alert(_ text: String) {
        NSLog("quick-launcher: \(text)")
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Quick Launcher"
        alert.informativeText = text
        alert.runModal()
    }
}
