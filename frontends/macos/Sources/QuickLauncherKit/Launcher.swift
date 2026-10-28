import Foundation

/// What the launcher shows and what it sends, with no UI in it. The app
/// calls a method for each thing the user does, sends the ``Outgoing`` it
/// returns, and feeds the core's states to ``receive(_:)``. The rules are
/// the Omarchy frontend's (`frontends/omarchy/Launcher.qml`), so both
/// behave the same.
public struct Launcher: Equatable {
    /// The cap on items per state, sent in `hello`.
    public static let maxItems = 50

    /// The mode shown with the text. Taken from a state only when that
    /// state is current (see *Reading input as the user saw it*).
    public private(set) var mode: Mode?
    public private(set) var text = ""
    /// The last `input` sent.
    public private(set) var seq = 0
    public private(set) var items: [Item] = []
    public private(set) var loading = false
    public private(set) var error: String?
    public private(set) var selectedIndex = 0
    /// A session is open in the core: from `open` until `done` or `close`.
    public private(set) var inSession = false
    /// The next state's items start with the first one selected. Set when
    /// the user changes the text or mode; otherwise a new state keeps the
    /// selection on the same item.
    private var resetSelection = true

    public init() {}

    /// A new core has applied no input yet, and its states start at 0.
    public mutating func coreStarted() {
        seq = 0
    }

    /// The window is shown. Nothing from the last session is drawn: the
    /// core answers right away.
    public mutating func open(mode modeId: String? = nil) -> Outgoing {
        mode = nil
        text = ""
        items = []
        loading = true
        error = nil
        selectedIndex = 0
        resetSelection = true
        inSession = true
        return .open(mode: modeId)
    }

    /// The window was hidden without running anything. `nil` when an action
    /// already ended the session.
    public mutating func close() -> Outgoing? {
        guard inSession else { return nil }
        inSession = false
        return .close
    }

    /// The core ran an action and ended the session, or stopped.
    public mutating func sessionEnded() {
        inSession = false
    }

    /// The user changed the text.
    public mutating func setText(_ newText: String) -> Outgoing {
        text = newText
        seq += 1
        resetSelection = true
        return .input(seq: seq, mode: mode?.id, text: newText)
    }

    /// Backspace on empty text. `nil` outside a mode.
    public mutating func leaveMode() -> Outgoing? {
        guard mode != nil else { return nil }
        resetSelection = true
        return .leaveMode
    }

    /// Tab: the selected row's mode when it's a mode hint, or the first
    /// hint's. The core decides which rows are hints.
    public mutating func complete() -> Outgoing {
        resetSelection = true
        return .complete(item: selected?.ref)
    }

    public var selected: Item? {
        items.indices.contains(selectedIndex) ? items[selectedIndex] : nil
    }

    /// Runs action `actionIndex` of the item at `index`; `nil` when there's
    /// no such action, or it's a disabled item's default.
    public func activate(index: Int, action actionIndex: Int) -> Outgoing? {
        guard items.indices.contains(index) else { return nil }
        let item = items[index]
        guard item.actions.indices.contains(actionIndex) else { return nil }
        if actionIndex == 0 && item.disabled { return nil }
        return .activate(item: item.ref, action: item.actions[actionIndex].id)
    }

    /// The selected item's action whose `key` is `press`, if any.
    public func activate(byKey press: KeyPress) -> Outgoing? {
        guard let item = selected else { return nil }
        guard let actionIndex = item.actions.firstIndex(where: { action in
            action.key.flatMap(KeySpec.actionKey) == press
        }) else { return nil }
        return activate(index: selectedIndex, action: actionIndex)
    }

    public mutating func move(_ delta: Int) {
        guard !items.isEmpty else { return }
        selectedIndex = max(0, min(items.count - 1, selectedIndex + delta))
    }

    public mutating func select(_ index: Int) {
        if items.indices.contains(index) { selectedIndex = index }
    }

    /// Draws a state. A stale one (older than the last input sent) updates
    /// the items but never the mode or text, which would undo what the user
    /// typed since.
    public mutating func receive(_ state: CoreState) {
        guard inSession else { return }
        let selectedRef = selected?.ref
        let current = state.seq >= seq
        if current {
            mode = state.mode
            text = state.text
        }
        items = state.items
        loading = state.loading
        error = state.error.flatMap { $0.isEmpty ? nil : $0 }
        if resetSelection {
            selectedIndex = 0
            if current { resetSelection = false }
        } else if let ref = selectedRef, let index = items.firstIndex(where: { $0.ref == ref }) {
            selectedIndex = index
        } else {
            selectedIndex = min(selectedIndex, max(0, items.count - 1))
        }
    }

    /// The search field's placeholder.
    public var placeholder: String {
        guard let mode else { return "Apps…" }
        return mode.placeholder ?? "\(mode.name)…"
    }

    /// What the list shows when it's empty.
    public var emptyMessage: String {
        if loading { return "Loading…" }
        let query = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty ? "Nothing here yet" : "No matches for “\(query)”"
    }

    /// What the selected row shows on its right: its alternate action, then
    /// any action with a key, so every way to reach an action is visible.
    public static func actionHints(_ item: Item) -> String {
        var hints: [String] = []
        for (index, action) in item.actions.enumerated() where index > 0 {
            var keys: [String] = []
            if index == 1 { keys.append("⇧↩") }
            if let key = action.key {
                keys.append(KeySpec.parse(key).map(KeySpec.symbols) ?? key)
            }
            if !keys.isEmpty { hints.append(keys.joined(separator: " / ") + " " + action.title) }
        }
        return hints.joined(separator: "   ")
    }
}
