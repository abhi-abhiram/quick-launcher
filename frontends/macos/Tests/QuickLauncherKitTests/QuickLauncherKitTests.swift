import Foundation
import XCTest

@testable import QuickLauncherKit

final class ProtocolTests: XCTestCase {
    func json(_ message: Outgoing) throws -> NSDictionary {
        let line = message.line()
        XCTAssertEqual(line.last, 0x0A, "a message is one line")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: line) as? NSDictionary)
    }

    func testOutgoingMessagesFollowTheProtocol() throws {
        XCTAssertEqual(
            try json(.hello(client: "macos", clientVersion: "0.1.0", maxItems: 50)),
            ["v": 1, "type": "hello", "client": "macos", "client_version": "0.1.0", "max_items": 50])
        XCTAssertEqual(try json(.open(mode: nil)), ["v": 1, "type": "open"])
        XCTAssertEqual(try json(.open(mode: "pi")), ["v": 1, "type": "open", "mode": "pi"])
        XCTAssertEqual(
            try json(.input(seq: 3, mode: nil, text: "pi:l")),
            ["v": 1, "type": "input", "seq": 3, "mode": NSNull(), "text": "pi:l"])
        XCTAssertEqual(try json(.leaveMode), ["v": 1, "type": "leave_mode"])
        XCTAssertEqual(try json(.complete(item: nil)), ["v": 1, "type": "complete"])
        XCTAssertEqual(
            try json(.activate(item: "pi/~/x", action: "open")),
            ["v": 1, "type": "activate", "item": "pi/~/x", "action": "open"])
        XCTAssertEqual(try json(.close), ["v": 1, "type": "close"])
    }

    func testAStateIsReadWithDefaultsForMissingFields() throws {
        let line = Data(#"""
            {"v":1,"type":"state","seq":12,
             "mode":{"id":"pi","name":"pi","icon":{"glyph":"π"},"placeholder":"Open pi in…"},
             "text":"launch","loading":false,"error":null,"future":true,
             "items":[{"ref":"pi/x","label":"x","detail":"~/x","badge":"pinned",
                       "actions":[{"id":"open","title":"Open pi"},{"id":"unpin","title":"Unpin","key":"ctrl+p"}]},
                      {"ref":"apps/a","label":"A","icon":{"path":"/Applications/A.app"},"disabled":true}]}
            """#.utf8)
        guard case let .state(state) = try Incoming.decode(line) else {
            return XCTFail("not a state")
        }
        XCTAssertEqual(state.seq, 12)
        XCTAssertEqual(state.mode, Mode(id: "pi", name: "pi", icon: Icon(glyph: "π"), placeholder: "Open pi in…"))
        XCTAssertEqual(state.text, "launch")
        XCTAssertNil(state.error)
        XCTAssertEqual(state.items.count, 2)
        XCTAssertEqual(state.items[0].actions[1], ItemAction(id: "unpin", title: "Unpin", key: "ctrl+p"))
        XCTAssertEqual(state.items[1].icon, Icon(path: "/Applications/A.app"))
        XCTAssertTrue(state.items[1].disabled)
        XCTAssertEqual(state.items[1].actions, [])
    }

    func testReadyDoneAndUnknownMessages() throws {
        XCTAssertEqual(
            try Incoming.decode(Data(#"{"v":2,"type":"ready","core_version":"0.2.0"}"#.utf8)),
            .ready(version: 2, coreVersion: "0.2.0"))
        XCTAssertEqual(try Incoming.decode(Data(#"{"v":1,"type":"done"}"#.utf8)), .done)
        XCTAssertEqual(try Incoming.decode(Data(#"{"v":1,"type":"toast"}"#.utf8)), .unknown("toast"))
        XCTAssertThrowsError(try Incoming.decode(Data("not json".utf8)))
    }

    func testLinesAreSplitAcrossChunks() {
        var splitter = LineSplitter()
        XCTAssertEqual(splitter.append(Data("{\"a\":1}\n{\"b\"".utf8)), [Data("{\"a\":1}".utf8)])
        XCTAssertEqual(splitter.append(Data(":2}\n\n  \n".utf8)), [Data("{\"b\":2}".utf8)])
        XCTAssertEqual(splitter.append(Data()), [])
    }
}

final class LauncherTests: XCTestCase {
    let open = ItemAction(id: "open", title: "Open")
    let pin = ItemAction(id: "pin", title: "Pin", key: "ctrl+p")

    func items(_ refs: String...) -> [Item] {
        refs.map { Item(ref: $0, label: $0, actions: [open, pin]) }
    }

    func testOpenStartsAnEmptySession() {
        var launcher = Launcher()
        XCTAssertEqual(launcher.open(mode: "pi"), .open(mode: "pi"))
        XCTAssertTrue(launcher.inSession)
        XCTAssertTrue(launcher.loading)
        XCTAssertEqual(launcher.emptyMessage, "Loading…")
        XCTAssertEqual(launcher.close(), .close)
        XCTAssertNil(launcher.close(), "a session closes once")
    }

    func testTypingSendsTheShownModeAndANewSeq() {
        var launcher = Launcher()
        _ = launcher.open()
        XCTAssertEqual(launcher.setText("pi:"), .input(seq: 1, mode: nil, text: "pi:"))
        launcher.receive(CoreState(seq: 1, mode: Mode(id: "pi", name: "pi"), text: ""))
        XCTAssertEqual(launcher.setText("l"), .input(seq: 2, mode: "pi", text: "l"))
    }

    func testAStaleStateKeepsTheModeAndText() {
        var launcher = Launcher()
        _ = launcher.open()
        _ = launcher.setText("pi:")
        _ = launcher.setText("pi:l")
        launcher.receive(CoreState(seq: 1, mode: Mode(id: "pi", name: "pi"), text: "", items: items("a")))
        XCTAssertNil(launcher.mode)
        XCTAssertEqual(launcher.text, "pi:l")
        XCTAssertEqual(launcher.items.map(\.ref), ["a"])
        XCTAssertEqual(launcher.setText("pi:la"), .input(seq: 3, mode: nil, text: "pi:la"))
        launcher.receive(CoreState(seq: 3, mode: Mode(id: "pi", name: "pi"), text: "la"))
        XCTAssertEqual(launcher.mode?.id, "pi")
        XCTAssertEqual(launcher.text, "la")
    }

    func testTheSelectionFollowsItsItemUntilTheTextChanges() {
        var launcher = Launcher()
        _ = launcher.open()
        launcher.receive(CoreState(items: items("a", "b", "c")))
        launcher.move(1)
        XCTAssertEqual(launcher.selected?.ref, "b")
        launcher.receive(CoreState(items: items("z", "a", "b")))
        XCTAssertEqual(launcher.selected?.ref, "b", "a refresh keeps the selected item")
        launcher.receive(CoreState(items: items("z")))
        XCTAssertEqual(launcher.selected?.ref, "z", "a vanished item clamps the selection")
        launcher.receive(CoreState(items: items("a", "b", "c")))
        launcher.move(5)
        XCTAssertEqual(launcher.selected?.ref, "c")
        _ = launcher.setText("x")
        launcher.receive(CoreState(seq: 1, items: items("a", "b", "c")))
        XCTAssertEqual(launcher.selectedIndex, 0, "new text selects the first item")
    }

    func testStatesOutsideASessionAreIgnored() {
        var launcher = Launcher()
        launcher.receive(CoreState(items: items("a")))
        XCTAssertEqual(launcher.items, [])
    }

    func testActivationRespectsDisabledItemsAndKeys() {
        var launcher = Launcher()
        _ = launcher.open()
        let disabled = Item(ref: "d", label: "d", disabled: true, actions: [open, pin])
        launcher.receive(CoreState(items: [disabled] + items("a")))
        XCTAssertNil(launcher.activate(index: 0, action: 0), "a disabled item's default can't run")
        XCTAssertEqual(launcher.activate(index: 0, action: 1), .activate(item: "d", action: "pin"))
        XCTAssertEqual(
            launcher.activate(byKey: KeyPress(.character("p"), .control)), .activate(item: "d", action: "pin"))
        XCTAssertNil(launcher.activate(byKey: KeyPress(.character("p"))))
        XCTAssertNil(launcher.activate(index: 5, action: 0))
        launcher.move(1)
        XCTAssertEqual(launcher.activate(index: launcher.selectedIndex, action: 0), .activate(item: "a", action: "open"))
    }

    func testLeaveModeAndComplete() {
        var launcher = Launcher()
        _ = launcher.open()
        XCTAssertNil(launcher.leaveMode(), "nothing to leave in the plain search")
        XCTAssertEqual(launcher.complete(), .complete(item: nil))
        launcher.receive(CoreState(mode: Mode(id: "pi", name: "pi"), items: items("hint")))
        XCTAssertEqual(launcher.leaveMode(), .leaveMode)
        XCTAssertEqual(launcher.complete(), .complete(item: "hint"))
    }

    func testPlaceholderEmptyMessageAndHints() {
        var launcher = Launcher()
        XCTAssertEqual(launcher.placeholder, "Apps…")
        _ = launcher.open()
        launcher.receive(CoreState(mode: Mode(id: "pi", name: "pi", placeholder: "Open pi in…"), text: " x "))
        XCTAssertEqual(launcher.placeholder, "Open pi in…")
        XCTAssertEqual(launcher.emptyMessage, "No matches for “x”")
        XCTAssertEqual(
            Launcher.actionHints(Item(ref: "a", label: "a", actions: [open, pin, ItemAction(id: "x", title: "X")])),
            "⇧↩ / ⌃P Pin")
    }
}

final class KeySpecTests: XCTestCase {
    func testChordsAreParsed() {
        XCTAssertEqual(KeySpec.parse("ctrl+d"), KeyPress(.character("d"), .control))
        XCTAssertEqual(KeySpec.parse("Shift+Delete"), KeyPress(.delete, .shift))
        XCTAssertEqual(KeySpec.parse("alt+space"), KeyPress(.space, .option))
        XCTAssertEqual(KeySpec.parse("super+shift+f12"), KeyPress(.function(12), [.command, .shift]))
        XCTAssertEqual(KeySpec.parse("cmd+1"), KeyPress(.character("1"), .command))
        for spec in ["", "ctrl+", "hyper+a", "ctrl+f13", "ctrl+é", "ctrl+pageup"] {
            XCTAssertNil(KeySpec.parse(spec), spec)
        }
    }

    func testActionsCantTakeTypingKeys() {
        XCTAssertNil(KeySpec.actionKey("d"))
        XCTAssertNil(KeySpec.actionKey("shift+d"))
        XCTAssertNil(KeySpec.actionKey("space"))
        XCTAssertNil(KeySpec.actionKey("enter"))
        XCTAssertEqual(KeySpec.actionKey("delete"), KeyPress(.delete))
        XCTAssertEqual(KeySpec.actionKey("f2"), KeyPress(.function(2)))
        XCTAssertEqual(KeySpec.actionKey("ctrl+enter"), KeyPress(.enter, .control))
    }

    func testSymbols() {
        XCTAssertEqual(KeySpec.symbols(KeyPress(.space, .option)), "⌥Space")
        XCTAssertEqual(KeySpec.symbols(KeyPress(.character("d"), [.control, .shift])), "⌃⇧D")
    }
}
