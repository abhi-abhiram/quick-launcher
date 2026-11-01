import AppKit

// A menu bar app with no Dock icon: the launcher panel opens on the hotkey.
let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
