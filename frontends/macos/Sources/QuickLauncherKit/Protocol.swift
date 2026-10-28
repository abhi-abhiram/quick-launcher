import Foundation

// The frontend protocol (docs/protocol.md, *Frontend protocol*): JSON Lines
// over the core's stdin and stdout.

/// The frontend protocol major this frontend speaks.
public let protocolVersion = 1

/// An icon, always one explicit form. A form this frontend can't draw shows
/// no icon.
public struct Icon: Decodable, Equatable, Hashable {
    public var glyph: String?
    public var name: String?
    public var path: String?

    public init(glyph: String? = nil, name: String? = nil, path: String? = nil) {
        self.glyph = glyph
        self.name = name
        self.path = path
    }
}

/// One of an item's actions. The first is the default, the second the
/// alternate.
public struct ItemAction: Decodable, Equatable, Hashable {
    public var id: String
    public var title: String
    /// A suggested shortcut such as `ctrl+d`; see ``KeySpec``.
    public var key: String?

    public init(id: String, title: String, key: String? = nil) {
        self.id = id
        self.title = title
        self.key = key
    }
}

/// A row to draw.
public struct Item: Decodable, Equatable, Hashable, Identifiable {
    /// Identifies the item to the core; opaque to the frontend.
    public var ref: String
    public var label: String
    public var detail: String?
    public var icon: Icon?
    public var badge: String?
    /// Drawn dimmed; the default action can't run.
    public var disabled: Bool
    public var actions: [ItemAction]

    public var id: String { ref }

    public init(
        ref: String, label: String, detail: String? = nil, icon: Icon? = nil,
        badge: String? = nil, disabled: Bool = false, actions: [ItemAction] = []
    ) {
        self.ref = ref
        self.label = label
        self.detail = detail
        self.icon = icon
        self.badge = badge
        self.disabled = disabled
        self.actions = actions
    }

    enum CodingKeys: String, CodingKey {
        case ref, label, detail, icon, badge, disabled, actions
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ref = try c.decode(String.self, forKey: .ref)
        label = try c.decode(String.self, forKey: .label)
        detail = try c.decodeIfPresent(String.self, forKey: .detail)
        icon = try c.decodeIfPresent(Icon.self, forKey: .icon)
        badge = try c.decodeIfPresent(String.self, forKey: .badge)
        disabled = try c.decodeIfPresent(Bool.self, forKey: .disabled) ?? false
        actions = try c.decodeIfPresent([ItemAction].self, forKey: .actions) ?? []
    }
}

/// The active mode.
public struct Mode: Decodable, Equatable, Hashable {
    public var id: String
    public var name: String
    public var icon: Icon?
    public var placeholder: String?

    public init(id: String, name: String, icon: Icon? = nil, placeholder: String? = nil) {
        self.id = id
        self.name = name
        self.icon = icon
        self.placeholder = placeholder
    }
}

/// A `state` message: everything to draw.
public struct CoreState: Decodable, Equatable {
    /// The newest `input` the core has applied.
    public var seq: Int
    public var mode: Mode?
    public var text: String
    public var loading: Bool
    public var error: String?
    public var items: [Item]

    public init(
        seq: Int = 0, mode: Mode? = nil, text: String = "", loading: Bool = false,
        error: String? = nil, items: [Item] = []
    ) {
        self.seq = seq
        self.mode = mode
        self.text = text
        self.loading = loading
        self.error = error
        self.items = items
    }

    enum CodingKeys: String, CodingKey {
        case seq, mode, text, loading, error, items
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        seq = try c.decodeIfPresent(Int.self, forKey: .seq) ?? 0
        mode = try c.decodeIfPresent(Mode.self, forKey: .mode)
        text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
        loading = try c.decodeIfPresent(Bool.self, forKey: .loading) ?? false
        error = try c.decodeIfPresent(String.self, forKey: .error)
        items = try c.decodeIfPresent([Item].self, forKey: .items) ?? []
    }
}

/// A message from the core.
public enum Incoming: Equatable {
    /// The reply to `hello`, with the core's major and build.
    case ready(version: Int, coreVersion: String)
    case state(CoreState)
    /// An action ran and the session is over.
    case done
    /// A type this frontend doesn't know, which it ignores.
    case unknown(String)

    private struct Envelope: Decodable {
        var v: Int?
        var type: String
        var core_version: String?
    }

    /// Reads one line the core wrote.
    public static func decode(_ line: Data) throws -> Incoming {
        let decoder = JSONDecoder()
        let envelope = try decoder.decode(Envelope.self, from: line)
        switch envelope.type {
        case "ready":
            return .ready(version: envelope.v ?? 0, coreVersion: envelope.core_version ?? "")
        case "state":
            return .state(try decoder.decode(CoreState.self, from: line))
        case "done":
            return .done
        default:
            return .unknown(envelope.type)
        }
    }
}

/// A message to the core.
public enum Outgoing: Equatable {
    case hello(client: String, clientVersion: String, maxItems: Int)
    /// A session starts, optionally straight into a mode.
    case open(mode: String?)
    case input(seq: Int, mode: String?, text: String)
    case leaveMode
    case complete(item: String?)
    case activate(item: String, action: String)
    case close

    /// The message's fields, `v` included.
    public var fields: [String: Any] {
        var fields: [String: Any] = ["v": protocolVersion]
        switch self {
        case let .hello(client, clientVersion, maxItems):
            fields["type"] = "hello"
            fields["client"] = client
            fields["client_version"] = clientVersion
            fields["max_items"] = maxItems
        case let .open(mode):
            fields["type"] = "open"
            if let mode { fields["mode"] = mode }
        case let .input(seq, mode, text):
            fields["type"] = "input"
            fields["seq"] = seq
            // `null` is meaningful here: the plain search.
            fields["mode"] = mode ?? NSNull()
            fields["text"] = text
        case .leaveMode:
            fields["type"] = "leave_mode"
        case let .complete(item):
            fields["type"] = "complete"
            if let item { fields["item"] = item }
        case let .activate(item, action):
            fields["type"] = "activate"
            fields["item"] = item
            fields["action"] = action
        case .close:
            fields["type"] = "close"
        }
        return fields
    }

    /// One JSON line, `\n` terminated.
    public func line() -> Data {
        var data = try! JSONSerialization.data(
            withJSONObject: fields, options: [.sortedKeys, .withoutEscapingSlashes])
        data.append(0x0A)
        return data
    }
}

/// Splits what the core writes into lines, keeping a partial line until the
/// rest arrives.
public struct LineSplitter {
    private var buffer = Data()

    public init() {}

    /// The complete lines in `chunk` and what came before it, without their
    /// `\n`. Blank lines are skipped.
    public mutating func append(_ chunk: Data) -> [Data] {
        buffer.append(chunk)
        var lines: [Data] = []
        while let newline = buffer.firstIndex(of: 0x0A) {
            let line = buffer[buffer.startIndex..<newline]
            buffer = Data(buffer[buffer.index(after: newline)...])
            if !line.allSatisfy({ $0 == 0x20 || $0 == 0x0D || $0 == 0x09 }) {
                lines.append(Data(line))
            }
        }
        return lines
    }
}
