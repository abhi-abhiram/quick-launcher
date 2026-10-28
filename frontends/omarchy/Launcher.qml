import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import qs.Commons
import qs.Ui

// The Omarchy frontend for quick-launcher-core. It holds no launcher logic:
// it sends what was typed or chosen over the frontend protocol
// (docs/protocol.md) and draws the state the core sends back. Apps, modes,
// prefixes, ranking and actions all live in the core and its plugins.
Item {
  id: root

  // Injected by omarchy-shell.
  property var shell: null
  property var manifest: null

  // The frontend protocol's major version, and the cap on items per state.
  readonly property int protocolVersion: 1
  readonly property int maxItems: 50

  // Installed by install.py next to the bundled plugins, which the core finds
  // under the same XDG data folder.
  readonly property string corePath: root.dataHome() + "/quick-launcher/bin/quick-launcher-core"

  property bool opened: false
  // A session is open in the core: from `open` until `done` or `close`.
  property bool inSession: false
  // Set when the core's major version differs from ours. The core has exited
  // then, and isn't started again until the shell restarts.
  property bool incompatible: false
  // Messages written before the core process has started.
  property var pending: []

  // What the frontend shows: its own mode and text, taken from a state only
  // when that state is current (see *Reading input as the user saw it*).
  property var mode: null
  property string text: ""
  property int seq: 0
  property var items: []
  property bool loading: false
  property string error: ""
  property int selectedIndex: 0
  // The next state's items start with the first one selected. Set when the
  // user changes the text or mode; otherwise a new state keeps the
  // selection on the same item.
  property bool resetSelection: true

  readonly property int maxVisibleRows: 8
  readonly property color background: Color.menu.background
  readonly property color foreground: Color.menu.text
  readonly property color scrim: Color.menu.scrim
  readonly property color selectedBackground: Color.menu.selectedBackground
  readonly property color selectedText: Color.menu.selectedText
  readonly property var borderSpec: Border.surfaceSpec("menu", "border", Color.menu.border, Math.max(1, Style.space(2)))
  readonly property var selectedBorderSpec: Border.surfaceSpec("menu", "selected-border", Color.menu.selectedBorder, 0)
  readonly property string fontFamily: Style.font.menuFamily
  readonly property int cornerRadius: Style.cornerRadius
  readonly property int contentMargin: Style.spacing.panelPadding
  readonly property int contentSpacing: Style.spacing.md
  readonly property int headerHeight: Math.max(Style.space(34), Style.font.title + Style.spacing.controlPaddingY * 2)
  readonly property int errorHeight: root.error ? Style.font.bodySmall + Style.space(8) : 0
  readonly property int rowHeight: Math.max(Style.space(58), Style.font.body + Style.font.caption + Style.spacing.rowPaddingX * 2)
  readonly property int rowSpacing: Style.spacing.xs
  readonly property int listHeight: items.length
    ? Math.min(items.length, maxVisibleRows) * (rowHeight + rowSpacing) - rowSpacing
    : rowHeight * 2

  // ------------------------------------------------------------ lifecycle

  // Payload: {"mode": "<plugin id>"} opens straight into a mode.
  function open(payloadJson) {
    var payload = ({})
    try { payload = JSON.parse(payloadJson || "{}") } catch (e) { }
    var modeId = payload && typeof payload.mode === "string" ? payload.mode : null

    if (root.incompatible) {
      root.notify("The launcher's core speaks a different protocol version. Run install.py again.")
      return
    }
    root.startCore()
    // Nothing from the last session is drawn: the core answers right away.
    root.mode = null
    root.text = ""
    root.items = []
    root.loading = true
    root.error = ""
    root.selectedIndex = 0
    root.resetSelection = true
    pointerGate.reset()
    root.inSession = true
    root.opened = true
    root.send(modeId ? { type: "open", mode: modeId } : { type: "open" })
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function close() {
    root.opened = false
  }

  function ping() { return "ok" }

  // Hiding the window by any route (Escape, a click outside, the shell's
  // toggle) ends the session, unless an action already ended it.
  onOpenedChanged: {
    if (!root.opened && root.inSession) {
      root.inSession = false
      root.send({ type: "close" })
    }
  }

  // ----------------------------------------------------------------- core

  function dataHome() {
    var data = Quickshell.env("XDG_DATA_HOME")
    if (data && String(data).charAt(0) === "/") return String(data)
    return (Quickshell.env("HOME") || "") + "/.local/share"
  }

  function startCore() {
    if (core.running) return
    root.pending = []
    // A new core has applied no input yet, and its states start at seq 0.
    root.seq = 0
    startupTimer.restart()
    root.send({ type: "hello", client: "omarchy", client_version: root.manifest && root.manifest.version
      ? String(root.manifest.version) : "", max_items: root.maxItems })
    core.running = true
  }

  function send(message) {
    message.v = root.protocolVersion
    var line = JSON.stringify(message) + "\n"
    if (core.accepting) core.write(line)
    else root.pending = root.pending.concat([line])
  }

  function notify(body) {
    console.warn("quick-launcher:", body)
    Quickshell.execDetached(["notify-send", "--app-name=Quick launcher", "Quick launcher", body])
  }

  function receive(line) {
    var message
    try {
      message = JSON.parse(line)
    } catch (e) {
      console.warn("quick-launcher: the core sent a line that isn't JSON:", line)
      return
    }
    if (!message || typeof message !== "object") return
    if (message.type === "ready") {
      if (message.v !== root.protocolVersion) {
        root.incompatible = true
        root.inSession = false
        root.opened = false
        root.notify("The launcher's core speaks protocol version " + message.v + ", this launcher speaks "
          + root.protocolVersion + ". Run install.py again to install matching versions.")
      }
    } else if (message.type === "state") {
      root.receiveState(message)
    } else if (message.type === "done") {
      root.inSession = false
      root.opened = false
    }
  }

  function receiveState(state) {
    if (!root.inSession) return
    var selected = root.items[root.selectedIndex]
    // A stale state (one older than our last input) updates the items but
    // never the mode or text, which would undo what the user typed since.
    if (state.seq >= root.seq) {
      root.mode = state.mode || null
      root.text = state.text || ""
    }
    root.items = Array.isArray(state.items) ? state.items : []
    root.loading = state.loading === true
    root.error = state.error || ""
    if (root.resetSelection) {
      root.selectedIndex = 0
      if (state.seq >= root.seq) root.resetSelection = false
    } else {
      root.selectedIndex = root.indexOfRef(selected ? selected.ref : null)
    }
    Qt.callLater(root.revealSelection)
  }

  function indexOfRef(ref) {
    for (var i = 0; i < root.items.length; i++) if (root.items[i].ref === ref) return i
    return Math.min(root.selectedIndex, Math.max(0, root.items.length - 1))
  }

  Process {
    id: core
    // Whether the process is running and can take input.
    property bool accepting: false
    command: [root.corePath]
    stdinEnabled: true
    stdout: SplitParser {
      onRead: function(data) { if (data.trim()) root.receive(data) }
    }
    stderr: SplitParser {
      onRead: function(data) { if (data.trim()) console.warn("quick-launcher-core:", data.trim()) }
    }
    onStarted: {
      core.accepting = true
      startupTimer.stop()
      var lines = root.pending
      root.pending = []
      for (var i = 0; i < lines.length; i++) core.write(lines[i])
    }
    onExited: function(exitCode) {
      core.accepting = false
      startupTimer.stop()
      root.pending = []
      if (exitCode !== 0 && !root.incompatible)
        console.warn("quick-launcher: the core exited with status", exitCode, "; it starts again on the next open")
      if (root.inSession) {
        root.inSession = false
        root.opened = false
        if (!root.incompatible) root.notify("The launcher's core stopped. Is it installed? Run install.py.")
      }
    }
  }

  // A core that never starts (not installed, say) would leave the window
  // showing "Loading…".
  Timer {
    id: startupTimer
    interval: 3000
    onTriggered: {
      if (core.accepting) return
      core.running = false
      root.pending = []
      if (root.inSession) {
        root.inSession = false
        root.opened = false
      }
      root.notify("Couldn't start " + root.corePath + ". Run install.py to install the core.")
    }
  }

  // ---------------------------------------------------------------- input

  function setText(text) {
    root.text = text
    root.seq += 1
    root.resetSelection = true
    pointerGate.reset()
    root.send({ type: "input", seq: root.seq, mode: root.mode ? root.mode.id : null, text: text })
  }

  function leaveMode() {
    root.resetSelection = true
    root.send({ type: "leave_mode" })
  }

  // Tab: the selected row's mode when it's a mode hint, or the first hint's.
  // The core decides which rows are hints.
  function complete() {
    var selected = root.items[root.selectedIndex]
    root.resetSelection = true
    root.send(selected ? { type: "complete", item: selected.ref } : { type: "complete" })
  }

  function activate(index, actionIndex) {
    var item = root.items[index]
    if (!item || !Array.isArray(item.actions)) return
    var action = item.actions[actionIndex]
    if (!action) return
    if (actionIndex === 0 && item.disabled) return
    root.send({ type: "activate", item: item.ref, action: String(action.id) })
  }

  function revealSelection() {
    if (root.items.length) resultList.positionViewAtIndex(root.selectedIndex, ListView.Contain)
  }

  function move(delta) {
    if (!root.items.length) return
    pointerGate.reset()
    root.selectedIndex = Math.max(0, Math.min(root.items.length - 1, root.selectedIndex + delta))
    root.revealSelection()
  }

  // An action's `key`, such as "ctrl+d" or "shift+delete", as a modifier
  // mask and a Qt key; null when it names a key this frontend doesn't bind.
  function parseKey(spec) {
    if (typeof spec !== "string" || !spec) return null
    var parts = spec.toLowerCase().split("+")
    var name = parts.pop()
    var modifiers = Qt.NoModifier
    for (var i = 0; i < parts.length; i++) {
      var part = parts[i].trim()
      if (part === "ctrl" || part === "control") modifiers |= Qt.ControlModifier
      else if (part === "shift") modifiers |= Qt.ShiftModifier
      else if (part === "alt") modifiers |= Qt.AltModifier
      else if (part === "super" || part === "meta") modifiers |= Qt.MetaModifier
      else return null
    }
    var named = { "delete": Qt.Key_Delete, "del": Qt.Key_Delete, "backspace": Qt.Key_Backspace,
      "enter": Qt.Key_Return, "return": Qt.Key_Return, "space": Qt.Key_Space, "tab": Qt.Key_Tab,
      "insert": Qt.Key_Insert, "home": Qt.Key_Home, "end": Qt.Key_End }
    var key
    if (named[name] !== undefined) key = named[name]
    else if (/^[a-z0-9]$/.test(name)) key = name.toUpperCase().charCodeAt(0)
    else if (/^f([1-9]|1[0-2])$/.test(name)) key = Qt.Key_F1 + Number(name.slice(1)) - 1
    else return null
    // Without Ctrl, Alt or Super these keys type, move or run the default
    // action, so an action can't take them.
    var typing = [Qt.Key_Space, Qt.Key_Backspace, Qt.Key_Tab, Qt.Key_Return]
    var chord = modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)
    if (!chord && (/^[a-z0-9]$/.test(name) || typing.indexOf(key) >= 0)) return null
    return { modifiers: modifiers, key: key }
  }

  // Enter on the keypad arrives as Key_Enter.
  function keyMatches(spec, event) {
    var parsed = root.parseKey(spec)
    if (!parsed) return false
    var mask = Qt.ControlModifier | Qt.ShiftModifier | Qt.AltModifier | Qt.MetaModifier
    var key = event.key === Qt.Key_Enter ? Qt.Key_Return : event.key
    return key === parsed.key && (event.modifiers & mask) === parsed.modifiers
  }

  // Runs the selected item's action whose `key` this event presses.
  function activateByKey(event) {
    var item = root.items[root.selectedIndex]
    if (!item || !Array.isArray(item.actions)) return false
    for (var i = 0; i < item.actions.length; i++) {
      if (root.keyMatches(item.actions[i].key, event)) {
        root.activate(root.selectedIndex, i)
        return true
      }
    }
    return false
  }

  // What the selected row shows on its right: its alternate action, then
  // any action with a key, so every way to reach an action is visible.
  function actionHints(item) {
    if (!item || !Array.isArray(item.actions)) return ""
    var hints = []
    for (var i = 1; i < item.actions.length; i++) {
      var action = item.actions[i]
      var keys = []
      if (i === 1) keys.push("⇧↵")
      if (action.key) keys.push(action.key)
      if (keys.length) hints.push(keys.join(" / ") + " " + action.title)
    }
    return hints.join("   ")
  }

  function iconSource(icon) {
    if (!icon || typeof icon !== "object") return ""
    if (icon.path) return "file://" + String(icon.path).split("/").map(encodeURIComponent).join("/")
    if (icon.name) return Quickshell.iconPath(icon.name, true)
    return ""
  }

  PointerMoveGate {
    id: pointerGate
    referenceItem: card
  }

  // ------------------------------------------------------------------- UI

  PanelWindow {
    id: panel
    visible: root.opened
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"
    // Shares the Omarchy menu's namespace so its layer rules (no fade) apply.
    WlrLayershell.namespace: "omarchy-menu"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    exclusionMode: ExclusionMode.Ignore

    Rectangle {
      anchors.fill: parent
      color: root.scrim
    }

    MouseArea {
      anchors.fill: parent
      onClicked: root.close()
    }

    BorderSurface {
      id: card
      // A fixed top edge: results grow and shrink downward without the card
      // jumping as you type.
      y: Math.round(panel.height * 0.2)
      anchors.horizontalCenter: parent.horizontalCenter
      width: Math.min(Style.space(560), panel.width - Style.gapsOut * 2)
      height: Math.min(contentTopInset + contentBottomInset + root.headerHeight + root.contentSpacing
        + root.errorHeight + (root.error ? root.contentSpacing : 0) + root.listHeight,
        panel.height - y - Style.gapsOut)
      radius: root.cornerRadius
      color: root.background
      borderSpec: root.borderSpec
      padding: root.contentMargin

      MouseArea { anchors.fill: parent; onClicked: {} }

      Item {
        id: keyCatcher
        anchors.fill: parent
        focus: true

        Keys.priority: Keys.BeforeItem
        Keys.onPressed: function(event) {
          var ctrl = event.modifiers & Qt.ControlModifier
          if (event.key === Qt.Key_Escape) {
            if (root.text) root.setText("")
            else root.close()
          } else if (Util.editsFilter(event, root.text)) {
            root.setText(Util.editedFilter(event, root.text))
          } else if (event.key === Qt.Key_Backspace && root.mode) {
            root.leaveMode()
          } else if (event.key === Qt.Key_Tab) {
            root.complete()
          } else if (event.key === Qt.Key_Up || (ctrl && (event.key === Qt.Key_P || event.key === Qt.Key_K))) {
            root.move(-1)
          } else if (event.key === Qt.Key_Down || (ctrl && (event.key === Qt.Key_N || event.key === Qt.Key_J))) {
            root.move(1)
          } else if (event.key === Qt.Key_PageUp) {
            root.move(-root.maxVisibleRows)
          } else if (event.key === Qt.Key_PageDown) {
            root.move(root.maxVisibleRows)
          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            root.activate(root.selectedIndex, (event.modifiers & (Qt.ShiftModifier | Qt.ControlModifier)) !== 0 ? 1 : 0)
          } else if (root.activateByKey(event)) {
            // The selected item's action with this key ran. The launcher's
            // own editing and navigation keys above always win.
          } else if (event.text && event.text.length === 1 && event.text.charCodeAt(0) >= 32
              && event.text.charCodeAt(0) !== 127
              && (event.modifiers === Qt.NoModifier || event.modifiers === Qt.ShiftModifier)) {
            root.setText(root.text + event.text)
          } else {
            return
          }
          event.accepted = true
        }
      }

      Column {
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset
        spacing: root.contentSpacing

        Item {
          width: parent.width
          height: root.headerHeight

          Rectangle {
            id: chip
            visible: root.mode !== null
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            width: visible ? chipText.implicitWidth + Style.space(16) : 0
            height: Math.round(parent.height * 0.8)
            radius: root.cornerRadius
            color: root.selectedBackground

            Text {
              id: chipText
              anchors.centerIn: parent
              textFormat: Text.PlainText
              text: root.mode
                ? ((root.mode.icon && root.mode.icon.glyph ? root.mode.icon.glyph + " " : "") + root.mode.name)
                : ""
              color: root.selectedText
              font.family: root.fontFamily
              font.pixelSize: Style.font.body
              font.weight: Font.Medium
            }
          }

          Text {
            anchors.left: chip.right
            anchors.leftMargin: chip.visible ? Style.space(10) : 0
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            textFormat: Text.PlainText
            text: root.text || (root.mode ? (root.mode.placeholder || root.mode.name + "…") : "Apps…")
            color: root.foreground
            opacity: root.text ? 1 : 0.58
            font.family: root.fontFamily
            font.pixelSize: Style.font.heading
            elide: Text.ElideRight
          }
        }

        Text {
          width: parent.width
          height: root.errorHeight
          visible: root.error.length > 0
          textFormat: Text.PlainText
          text: root.error
          color: root.foreground
          opacity: 0.75
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
          elide: Text.ElideRight
          verticalAlignment: Text.AlignVCenter
        }

        Item {
          width: parent.width
          // Capped to what the card has left, so rows past the screen edge
          // stay inside the ListView and scroll into view.
          height: Math.max(0, Math.min(root.listHeight, card.height - card.contentTopInset
            - card.contentBottomInset - root.headerHeight - root.contentSpacing - root.errorHeight
            - (root.error ? root.contentSpacing : 0)))

          ListView {
            id: resultList
            anchors.fill: parent
            model: root.items
            clip: true
            spacing: root.rowSpacing
            boundsBehavior: Flickable.StopAtBounds

            delegate: BorderSurface {
              id: row
              required property int index
              required property var modelData

              readonly property bool hasCursor: row.index === root.selectedIndex
              readonly property bool disabled: modelData.disabled === true
              readonly property var icon: modelData.icon || null
              readonly property string iconImage: root.iconSource(row.icon)
              readonly property string hint: row.hasCursor ? root.actionHints(modelData) : ""

              width: ListView.view.width
              height: root.rowHeight
              radius: root.cornerRadius
              color: row.hasCursor ? root.selectedBackground : "transparent"
              borderSpec: row.hasCursor ? root.selectedBorderSpec : Border.none()

              Item {
                id: iconBox
                anchors.left: parent.left
                anchors.leftMargin: Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                width: Style.space(36)
                height: parent.height

                Text {
                  anchors.centerIn: parent
                  visible: row.iconImage === ""
                  textFormat: Text.PlainText
                  text: row.icon && row.icon.glyph ? row.icon.glyph : ""
                  color: row.hasCursor ? root.selectedText : root.foreground
                  opacity: row.disabled ? 0.45 : 1
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.iconLarge
                }

                Image {
                  anchors.centerIn: parent
                  visible: row.iconImage !== ""
                  width: Style.font.iconLarge
                  height: Style.font.iconLarge
                  fillMode: Image.PreserveAspectFit
                  // Decode at physical pixels so icons stay sharp on HiDPI.
                  sourceSize.width: width * Screen.devicePixelRatio
                  sourceSize.height: height * Screen.devicePixelRatio
                  source: row.iconImage
                  opacity: row.disabled ? 0.45 : 1
                  asynchronous: true
                }
              }

              Column {
                anchors.left: iconBox.right
                anchors.leftMargin: Style.space(6)
                anchors.right: badgeText.left
                anchors.rightMargin: Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                spacing: Style.space(3)
                opacity: row.disabled ? 0.45 : 1

                Text {
                  width: parent.width
                  textFormat: Text.PlainText
                  text: modelData.label
                  color: row.hasCursor ? root.selectedText : root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.heading
                  font.weight: Font.Medium
                  elide: Text.ElideRight
                }

                Text {
                  width: parent.width
                  visible: text.length > 0
                  textFormat: Text.PlainText
                  text: modelData.detail || ""
                  color: row.hasCursor ? root.selectedText : root.foreground
                  opacity: 0.52
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.bodySmall
                  elide: Text.ElideMiddle
                }
              }

              Text {
                id: badgeText
                anchors.right: hintText.left
                anchors.rightMargin: text ? Style.space(8) : 0
                anchors.verticalCenter: parent.verticalCenter
                textFormat: Text.PlainText
                text: modelData.badge || ""
                color: row.hasCursor ? root.selectedText : root.foreground
                opacity: 0.6
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
              }

              Text {
                id: hintText
                anchors.right: parent.right
                anchors.rightMargin: Style.space(12)
                anchors.verticalCenter: parent.verticalCenter
                textFormat: Text.PlainText
                text: row.hint
                color: root.selectedText
                opacity: 0.6
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
              }

              MouseArea {
                id: mouseArea
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                cursorShape: row.disabled ? Qt.ArrowCursor : Qt.PointingHandCursor
                onEntered: if (pointerGate.moved(row, { x: mouseArea.mouseX, y: mouseArea.mouseY })) root.selectedIndex = row.index
                onPositionChanged: function(mouse) { if (pointerGate.moved(row, mouse)) root.selectedIndex = row.index }
                // Click runs the default action, right-click the alternate.
                onClicked: function(mouse) {
                  root.selectedIndex = row.index
                  root.activate(row.index, mouse.button === Qt.RightButton ? 1 : 0)
                }
              }
            }
          }

          Text {
            anchors.centerIn: parent
            visible: root.items.length === 0
            textFormat: Text.PlainText
            text: root.loading ? "Loading…"
              : root.text.trim() ? "No matches for “" + root.text.trim() + "”"
              : "Nothing here yet"
            color: root.foreground
            opacity: 0.7
            font.family: root.fontFamily
            font.pixelSize: Style.font.title
          }
        }
      }
    }
  }
}
