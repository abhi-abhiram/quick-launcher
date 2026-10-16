import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import QtQuick
import qs.Commons
import qs.Ui
import qs.services
import "LauncherModel.js" as Model

// App launcher with prefix modes. With no prefix it searches applications;
// typing a mode's prefix (e.g. `pi:` or `>`) switches the suggestions to that
// mode's rows. Modes are declared in modes.json and backed by a list command
// that prints {"rows": [...]}; the launcher filters those rows as you type.
Item {
  id: root

  // Injected by omarchy-shell. Menu plugins are meant to receive the shared
  // app library, but omarchy 4.0.4 checks the manifest's kinds after they have
  // passed through an Instantiator model, where Array.isArray() is false, so
  // third-party menus get null. Until then, run our own copy of the service.
  property string omarchyPath: Quickshell.env("OMARCHY_PATH")
  property var shell: null
  property var manifest: null
  readonly property var appLibrary: shell && shell.appLibrary ? shell.appLibrary : ownAppLibrary.item

  Loader {
    id: ownAppLibrary
    active: root.shell !== null && !root.shell.appLibrary
    sourceComponent: AppLibrary { }
  }

  readonly property string pluginDir: localPath(Qt.resolvedUrl("."))

  property bool opened: false
  property var modes: []
  // The active mode, or null while searching apps.
  property var mode: null
  property string query: ""
  property var rows: []
  property int selectedIndex: 0
  // Mode id -> rows from its list command; kept between opens so a mode shows
  // its last rows instantly while they refresh.
  property var modeRows: ({})
  property var listQueue: []

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
  readonly property int rowHeight: Math.max(Style.space(58), Style.font.body + Style.font.caption + Style.spacing.rowPaddingX * 2)
  readonly property int rowSpacing: Style.spacing.xs
  readonly property int listHeight: rows.length
    ? Math.min(rows.length, maxVisibleRows) * (rowHeight + rowSpacing) - rowSpacing
    : rowHeight * 2

  // ------------------------------------------------------------ lifecycle

  // Payload: {"mode": "<id>"} opens straight into a mode.
  function open(payloadJson) {
    var payload = ({})
    try { payload = JSON.parse(payloadJson || "{}") } catch (e) { }
    root.mode = root.modeById(payload && payload.mode)
    root.query = ""
    root.selectedIndex = 0
    pointerGate.reset()
    root.opened = true
    for (var i = 0; i < root.modes.length; i++) root.loadMode(root.modes[i].id)
    if (root.appLibrary) root.appLibrary.refreshIcons()
    root.rebuild()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function close() {
    root.opened = false
  }

  function ping() { return "ok" }

  // ---------------------------------------------------------------- modes

  function localPath(url) {
    var path = url.toString().replace(/^file:\/\//, "")
    try { return decodeURIComponent(path) } catch (e) { return path }
  }

  function modeById(id) {
    for (var i = 0; i < root.modes.length; i++) if (root.modes[i].id === id) return root.modes[i]
    return null
  }

  function enterMode(mode) {
    root.mode = mode
    root.setQuery("")
  }

  // Backspace past the chip returns to apps with the prefix minus its last
  // character, as if the user had deleted the ":" they typed.
  function leaveMode() {
    var prefix = root.mode.prefixes[0]
    root.mode = null
    root.setQuery(prefix.slice(0, -1))
  }

  function setQuery(text) {
    if (!root.mode) {
      var parsed = Model.parsePrefix(text, root.modes)
      if (parsed) {
        root.mode = parsed.mode
        text = parsed.query
      }
    }
    root.query = text
    root.selectedIndex = 0
    pointerGate.reset()
    root.rebuild()
  }

  // Runs argv from the plugin folder through a login shell, so modes see the
  // same PATH and environment as a terminal and can use relative script paths.
  function shellCommand(argv) {
    return ["bash", "-lc", 'cd -- "$1" && shift && exec "$@"', "bash", root.pluginDir].concat(argv)
  }

  function loadMode(id) {
    if (root.listQueue.indexOf(id) < 0) root.listQueue = root.listQueue.concat([id])
    root.startNextList()
  }

  function startNextList() {
    while (!listProc.running && root.listQueue.length > 0) {
      var mode = root.modeById(root.listQueue[0])
      root.listQueue = root.listQueue.slice(1)
      if (!mode) continue
      listProc.modeId = mode.id
      listProc.collected = ""
      listProc.command = root.shellCommand(mode.list)
      listProc.running = true
    }
  }

  function receiveRows(modeId, text) {
    var rows
    try {
      rows = Model.parseRows(text)
    } catch (e) {
      console.warn("quick-launcher: mode", modeId, "printed invalid rows:", e)
      return
    }
    var next = ({})
    for (var id in root.modeRows) next[id] = root.modeRows[id]
    next[modeId] = rows
    root.modeRows = next
    if (root.opened && root.mode && root.mode.id === modeId) root.rebuild(true)
  }

  // ----------------------------------------------------------------- rows

  function rebuild(keepSelection) {
    var out = []
    if (!root.mode) {
      var hints = Model.modeHints(root.query, root.modes)
      var hintRow = function(mode) {
        return { kind: "mode", modeId: mode.id, label: mode.name, icon: mode.icon || "",
          detail: mode.prefixes.join("  ") + "  ·  " + (mode.description || "") }
      }
      var named = hints.filter(function(mode) { return Model.namesMode(root.query, mode) })
      var others = hints.filter(function(mode) { return !Model.namesMode(root.query, mode) })
      out = named.map(hintRow)
      var apps = root.appLibrary ? root.appLibrary.sortedEntries(root.query) : []
      for (var i = 0; i < apps.length; i++) {
        var entry = apps[i].entry
        if (!entry || !entry.id) continue
        out.push({ kind: "app", appId: String(entry.id), appIcon: String(entry.icon || ""),
          label: root.appLibrary.entryName(entry), detail: root.appLibrary.entrySubtext(entry), icon: "" })
      }
      out = out.concat(others.map(hintRow))
    } else {
      var listed = Model.filterRows(root.modeRows[root.mode.id] || [], root.query)
      var typed = Model.queryRow(root.mode, root.query, listed)
      if (typed) out.push(Object.assign({}, typed, { kind: "action" }))
      for (var j = 0; j < listed.length; j++)
        out.push(Object.assign({}, listed[j], { kind: "action", icon: listed[j].icon || root.mode.icon || "" }))
    }
    root.rows = out
    root.selectedIndex = keepSelection ? Math.min(root.selectedIndex, Math.max(0, out.length - 1)) : 0
    Qt.callLater(root.revealSelection)
  }

  function revealSelection() {
    if (root.rows.length) resultList.positionViewAtIndex(root.selectedIndex, ListView.Contain)
  }

  function move(delta) {
    if (!root.rows.length) return
    pointerGate.reset()
    root.selectedIndex = Math.max(0, Math.min(root.rows.length - 1, root.selectedIndex + delta))
    root.revealSelection()
  }

  // Tab completes into the selected mode hint, or the first one listed.
  function completeMode() {
    var selected = root.rows[root.selectedIndex]
    if (selected && selected.kind === "mode") return root.enterMode(root.modeById(selected.modeId))
    for (var i = 0; i < root.rows.length; i++)
      if (root.rows[i].kind === "mode") return root.enterMode(root.modeById(root.rows[i].modeId))
  }

  function activate(index, secondary) {
    var row = root.rows[index]
    if (!row) return
    if (row.kind === "mode") {
      root.enterMode(root.modeById(row.modeId))
    } else if (row.kind === "app") {
      if (secondary) return
      root.close()
      if (root.appLibrary) root.appLibrary.launch(row.appId, row.label)
    } else if (secondary) {
      // A secondary action changes the mode's state (pin, forget), so the
      // launcher stays open and reloads the mode once it finishes.
      if (!row.secondary || !Array.isArray(row.secondary.action) || actionProc.running) return
      actionProc.modeId = root.mode.id
      actionProc.command = root.shellCommand(row.secondary.action)
      actionProc.running = true
    } else if (!row.disabled) {
      root.close()
      Quickshell.execDetached(root.shellCommand(row.action))
    }
  }

  // ---------------------------------------------------------------- data

  FileView {
    path: root.pluginDir + "/modes.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: {
      try {
        root.modes = Model.validModes(JSON.parse(text()))
      } catch (e) {
        console.warn("quick-launcher: ignoring invalid modes.json:", e)
        return
      }
      if (root.mode) root.mode = root.modeById(root.mode.id)
      if (root.opened) root.rebuild()
    }
  }

  Process {
    id: listProc
    property string modeId: ""
    property string collected: ""
    stdout: SplitParser {
      onRead: function(data) { listProc.collected += data + "\n" }
    }
    stderr: SplitParser {
      onRead: function(data) { if (data.trim()) console.warn(data.trim()) }
    }
    onExited: function(exitCode) {
      if (exitCode === 0) root.receiveRows(listProc.modeId, listProc.collected)
      else console.warn("quick-launcher: mode", listProc.modeId, "list exited with", exitCode)
      Qt.callLater(root.startNextList)
    }
  }

  Process {
    id: actionProc
    property string modeId: ""
    stderr: SplitParser {
      onRead: function(data) { if (data.trim()) console.warn(data.trim()) }
    }
    onExited: root.loadMode(actionProc.modeId)
  }

  Connections {
    target: root.appLibrary
    function onAppsChanged() { if (root.opened && !root.mode) root.rebuild(true) }
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
      height: Math.min(contentTopInset + contentBottomInset + root.headerHeight + root.contentSpacing + root.listHeight,
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
            if (root.query) root.setQuery("")
            else root.close()
          } else if (Util.editsFilter(event, root.query)) {
            root.setQuery(Util.editedFilter(event, root.query))
          } else if (event.key === Qt.Key_Backspace && root.mode) {
            root.leaveMode()
          } else if (event.key === Qt.Key_Tab) {
            root.completeMode()
          } else if (event.key === Qt.Key_Up || (ctrl && (event.key === Qt.Key_P || event.key === Qt.Key_K))) {
            root.move(-1)
          } else if (event.key === Qt.Key_Down || (ctrl && (event.key === Qt.Key_N || event.key === Qt.Key_J))) {
            root.move(1)
          } else if (event.key === Qt.Key_PageUp) {
            root.move(-root.maxVisibleRows)
          } else if (event.key === Qt.Key_PageDown) {
            root.move(root.maxVisibleRows)
          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            root.activate(root.selectedIndex, (event.modifiers & (Qt.ShiftModifier | Qt.ControlModifier)) !== 0)
          } else if (event.text && event.text.length === 1 && event.text.charCodeAt(0) >= 32
              && event.text.charCodeAt(0) !== 127
              && (event.modifiers === Qt.NoModifier || event.modifiers === Qt.ShiftModifier)) {
            root.setQuery(root.query + event.text)
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
              text: root.mode ? ((root.mode.icon ? root.mode.icon + " " : "") + root.mode.name) : ""
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
            text: root.query || (root.mode
              ? (root.mode.placeholder || root.mode.name + "…")
              : "Apps…   " + root.modes.map(function(mode) { return mode.prefixes[0] + " " + mode.name }).join("   "))
            color: root.foreground
            opacity: root.query ? 1 : 0.58
            font.family: root.fontFamily
            font.pixelSize: Style.font.heading
            elide: Text.ElideRight
          }
        }

        Item {
          width: parent.width
          height: root.listHeight

          ListView {
            id: resultList
            anchors.fill: parent
            model: root.rows
            clip: true
            spacing: root.rowSpacing
            boundsBehavior: Flickable.StopAtBounds

            delegate: BorderSurface {
              id: row
              required property int index
              required property var modelData

              readonly property bool hasCursor: row.index === root.selectedIndex
              readonly property bool isApp: modelData.kind === "app"
              readonly property bool disabled: modelData.disabled === true
              readonly property string hint: row.hasCursor
                ? (modelData.kind === "mode" ? "Tab"
                  : modelData.secondary && modelData.secondary.label ? "⇧↵ " + modelData.secondary.label : "")
                : ""

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
                  visible: !row.isApp
                  textFormat: Text.PlainText
                  text: modelData.icon || ""
                  color: row.hasCursor ? root.selectedText : root.foreground
                  opacity: row.disabled ? 0.45 : 1
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.iconLarge
                }

                Image {
                  anchors.centerIn: parent
                  visible: row.isApp
                  width: Style.font.iconLarge
                  height: Style.font.iconLarge
                  fillMode: Image.PreserveAspectFit
                  // Decode at physical pixels so icons stay sharp on HiDPI.
                  sourceSize.width: width * Screen.devicePixelRatio
                  sourceSize.height: height * Screen.devicePixelRatio
                  source: row.isApp && root.appLibrary ? root.appLibrary.iconSource(modelData.appIcon) : ""
                  asynchronous: true
                }
              }

              Column {
                anchors.left: iconBox.right
                anchors.leftMargin: Style.space(6)
                anchors.right: hintText.left
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
                // Right-click runs the secondary action (pin, forget).
                onClicked: function(mouse) {
                  root.selectedIndex = row.index
                  root.activate(row.index, mouse.button === Qt.RightButton)
                }
              }
            }
          }

          Text {
            anchors.centerIn: parent
            visible: root.rows.length === 0
            textFormat: Text.PlainText
            text: root.mode && root.modeRows[root.mode.id] === undefined ? "Loading…"
              : root.query ? "No matches for “" + root.query + "”"
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
