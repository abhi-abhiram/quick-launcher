import Foundation
import QuickLauncherKit

/// `quick-launcher-core` as a child process, spoken to over the frontend
/// protocol. Its stderr is the log, passed on to ours. Callbacks run on the
/// main queue.
final class CoreProcess {
    let executable: URL
    var onMessage: (Incoming) -> Void = { _ in }
    /// The core exited, with its status.
    var onExit: (Int32) -> Void = { _ in }

    private var process: Process?
    private var stdin: FileHandle?
    private var splitter = LineSplitter()

    init(executable: URL) {
        self.executable = executable
    }

    var isRunning: Bool { process?.isRunning == true }

    /// Starts the core unless it's running, and sends `hello` first. Throws
    /// when it can't be started (not installed, say).
    func start(hello: Outgoing) throws {
        if isRunning { return }
        let process = Process()
        process.executableURL = executable
        process.currentDirectoryURL = FileManager.default.homeDirectoryForCurrentUser
        process.environment = CoreProcess.environment()
        let input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.standardError
        splitter = LineSplitter()
        output.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                return
            }
            DispatchQueue.main.async { self?.received(data) }
        }
        process.terminationHandler = { [weak self] ended in
            let status = ended.terminationStatus
            DispatchQueue.main.async {
                guard let self, self.process === ended else { return }
                self.process = nil
                self.stdin = nil
                self.onExit(status)
            }
        }
        try process.run()
        self.process = process
        stdin = input.fileHandleForWriting
        send(hello)
    }

    func send(_ message: Outgoing) {
        guard let stdin else { return }
        do {
            try stdin.write(contentsOf: message.line())
        } catch {
            NSLog("quick-launcher: writing to the core: \(error)")
        }
    }

    /// Closes the core's stdin, which ends it.
    func stop() {
        try? stdin?.close()
        stdin = nil
    }

    private func received(_ data: Data) {
        for line in splitter.append(data) {
            do {
                onMessage(try Incoming.decode(line))
            } catch {
                NSLog("quick-launcher: the core sent a line that isn't a message: \(String(decoding: line, as: UTF8.self))")
            }
        }
    }

    /// Ours, with Homebrew's folders on `PATH`. An app started from Finder
    /// gets only the system folders, and `exec` actions of plugins that
    /// don't ask for the login environment should still find `code`, say.
    static func environment(_ base: [String: String] = ProcessInfo.processInfo.environment) -> [String: String] {
        var environment = base
        var path = (base["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin").split(separator: ":").map(String.init)
        for folder in ["/opt/homebrew/bin", "/usr/local/bin"] where !path.contains(folder) {
            path.append(folder)
        }
        environment["PATH"] = path.joined(separator: ":")
        return environment
    }

    /// Where `install.py` puts the core: under `$XDG_DATA_HOME` when it's an
    /// absolute path, else `~/.local/share`, as the core's macOS platform
    /// reads it. The `corePath` default replaces it, for development.
    static func installedPath() -> URL {
        if let path = UserDefaults.standard.string(forKey: "corePath"), !path.isEmpty {
            return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        var data = home.appendingPathComponent(".local/share")
        if let xdg = ProcessInfo.processInfo.environment["XDG_DATA_HOME"], xdg.hasPrefix("/") {
            data = URL(fileURLWithPath: xdg)
        }
        return data.appendingPathComponent("quick-launcher/bin/quick-launcher-core")
    }
}
