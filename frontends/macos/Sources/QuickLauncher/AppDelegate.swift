import AppKit
import QuickLauncherKit
import ServiceManagement

enum AppInfo {
    static let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"

    /// The hotkey when the `hotkey` default isn't set: Option+Space, since
    /// Command+Space is Spotlight's.
    static let defaultHotKey = "alt+space"
}

/// The menu bar item, the hotkey and `quick-launcher://` URLs, all of which
/// open the launcher.
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var controller: LauncherController!
    private var statusItem: NSStatusItem!
    private var hotKey: HotKey?
    private var hotKeyTitle = ""

    func applicationDidFinishLaunching(_ notification: Notification) {
        // A write to a core that just exited must fail, not end the app.
        signal(SIGPIPE, SIG_IGN)
        controller = LauncherController(core: CoreProcess(executable: CoreProcess.installedPath()))
        registerHotKey()
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.button?.image = NSImage(
            systemSymbolName: "magnifyingglass", accessibilityDescription: "Quick Launcher")
        statusItem.menu = makeMenu()
    }

    /// Registers the `hotkey` default (such as `ctrl+space` or
    /// `cmd+shift+k`; set it with `defaults write`), else Option+Space.
    private func registerHotKey() {
        let spec = UserDefaults.standard.string(forKey: "hotkey") ?? AppInfo.defaultHotKey
        guard let press = KeySpec.parse(spec) else {
            NSLog("quick-launcher: can't read the hotkey \(spec); use a chord such as alt+space")
            return
        }
        hotKey = HotKey(press) { [weak self] in self?.controller.toggle() }
        if hotKey == nil {
            NSLog("quick-launcher: couldn't register the hotkey \(spec); another app may hold it")
        } else {
            hotKeyTitle = KeySpec.symbols(press)
        }
    }

    private func makeMenu() -> NSMenu {
        let menu = NSMenu()
        let open = NSMenuItem(title: "Open Quick Launcher", action: #selector(openLauncher), keyEquivalent: "")
        if !hotKeyTitle.isEmpty { open.title += "  (\(hotKeyTitle))" }
        open.target = self
        menu.addItem(open)
        menu.addItem(.separator())
        let login = NSMenuItem(title: "Open at Login", action: #selector(toggleOpenAtLogin(_:)), keyEquivalent: "")
        login.target = self
        login.state = SMAppService.mainApp.status == .enabled ? .on : .off
        menu.addItem(login)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit Quick Launcher", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
        return menu
    }

    @objc private func openLauncher() {
        // The menu closes first, so the panel can take the keyboard.
        DispatchQueue.main.async { self.controller.show() }
    }

    @objc private func toggleOpenAtLogin(_ item: NSMenuItem) {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            NSLog("quick-launcher: changing Open at Login: \(error)")
        }
        item.state = SMAppService.mainApp.status == .enabled ? .on : .off
    }

    /// `quick-launcher://toggle`, or `quick-launcher://open` with an
    /// optional `?mode=<plugin id>`, so other tools (a Shortcuts action,
    /// skhd) can open it, straight into a mode if they like.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls where url.scheme == "quick-launcher" {
            let mode = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                .queryItems?.first { $0.name == "mode" }?.value
            if url.host == "toggle" && mode == nil {
                controller.toggle()
            } else {
                controller.show(mode: mode.flatMap { $0.isEmpty ? nil : $0 })
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        controller.stopCore()
    }
}
