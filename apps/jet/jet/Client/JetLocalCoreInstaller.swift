#if os(macOS)
import Foundation
import Darwin

/// Installs the core shipped in a direct-download app before its first connection.
/// Development builds have no bundled payload and continue to use an existing Plane.
enum JetLocalCoreInstaller {
    private nonisolated static let label = "com.apexgang.jet.jetd"
    private nonisolated static let executables = ["jetd", "jetfueld", "jet-craft-claude", "jet-craft-codex"]

    nonisolated static func ensureRunning() async throws -> Bool {
        guard let payload = Bundle.main.resourceURL?.appending(path: "jet-core", directoryHint: .isDirectory),
              FileManager.default.fileExists(atPath: payload.appending(path: "manifest.json").path)
        else { return false }

        try await Task.detached(priority: .userInitiated) {
            try install(payload: payload)
        }.value
        return true
    }

    private nonisolated static func install(payload: URL) throws {
        let manifestData = try Data(contentsOf: payload.appending(path: "manifest.json"))
        guard manifestData.count <= 65_536 else { throw InstallerError.invalidPayload }
        // ASVS 1.5.2: read only the expected fields from the bundled manifest.
        guard let manifest = try JSONSerialization.jsonObject(with: manifestData) as? [String: Any],
              let version = manifest["version"] as? String,
              manifest["target"] as? String == "universal-apple-darwin",
              executables.allSatisfy({ FileManager.default.isExecutableFile(atPath: payload.appending(path: $0).path) })
        else { throw InstallerError.invalidPayload }

        let core = payload.appending(path: "jetd")
        guard let status = try JSONSerialization.jsonObject(with: run(core, ["core", "status"])) as? [String: Any],
              let owner = status["owner"] as? [String: Any]
        else { throw InstallerError.invalidPayload }
        let current = status["current"] as? String
        let channel = (owner["daemon"] as? [String: Any])?["channel"] as? String
        // The Homebrew channel owns an active daemon. Connect to it without
        // replacing its files or service.
        if channel == "homebrew" { return }
        if let channel, channel != "gui" {
            throw InstallerError.otherChannel(channel)
        }

        if current != version {
            _ = try run(core, ["core", "stage", "--payload", payload.path])
            _ = try run(core, ["core", "activate", "--version", version])
        }
        try startAgent(restart: current != version)
    }

    private nonisolated static func startAgent(restart: Bool) throws {
        let fileManager = FileManager.default
        let home = fileManager.homeDirectoryForCurrentUser
        let agents = home.appending(path: "Library/LaunchAgents", directoryHint: .isDirectory)
        let plist = agents.appending(path: "\(label).plist")
        try fileManager.createDirectory(at: agents, withIntermediateDirectories: true)
        let executable = home.appending(path: ".jet/core/current/jetd").path
        let domain = "gui/\(getuid())"
        let service = "\(domain)/\(label)"
        let definition: [String: Any] = [
            "Label": label,
            "ProgramArguments": [executable, "serve", "--channel", "gui"],
            "RunAtLoad": true,
            "KeepAlive": true,
            "AbandonProcessGroup": true,
            "ExitTimeOut": 15,
            "EnvironmentVariables": ["PATH": "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/local/sbin:/usr/bin:/bin:/usr/sbin:/sbin"]
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: definition, format: .xml, options: 0)
        if (try? Data(contentsOf: plist)) != data {
            try data.write(to: plist, options: .atomic)
        }
        if (try? run(URL(fileURLWithPath: "/bin/launchctl"), ["print", service])) == nil {
            _ = try run(URL(fileURLWithPath: "/bin/launchctl"), ["bootstrap", domain, plist.path])
        } else if restart {
            _ = try run(URL(fileURLWithPath: "/bin/launchctl"), ["kickstart", "-k", service])
        }
    }

    @discardableResult
    private nonisolated static func run(_ executable: URL, _ arguments: [String]) throws -> Data {
        // ASVS 1.2.5: pass every value as a Process argument, never through a shell.
        let process = Process()
        let output = Pipe()
        let errors = Pipe()
        process.executableURL = executable
        process.arguments = arguments
        process.standardOutput = output
        process.standardError = errors
        try process.run()
        let stdout = output.fileHandleForReading.readDataToEndOfFile()
        let stderr = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let detail = String(decoding: stderr.isEmpty ? stdout : stderr, as: UTF8.self)
            throw InstallerError.commandFailed(executable.lastPathComponent, detail.trimmingCharacters(in: .whitespacesAndNewlines))
        }
        return stdout
    }

    /// Why the bundled core could not start. `DesktopSession` maps each case to a
    /// stable `core.*` code; the command detail stays out of casual copy.
    enum InstallerError: LocalizedError {
        case invalidPayload
        case otherChannel(String)
        case commandFailed(String, String)

        var code: String {
            switch self {
            case .invalidPayload: "core.install_incomplete"
            case .otherChannel: "core.owned_by_other_channel"
            case .commandFailed: "core.start_failed"
            }
        }

        /// A failed launchctl or core command may succeed on a later attempt.
        var isRetryable: Bool {
            if case .commandFailed = self { return true }
            return false
        }

        var errorDescription: String? {
            switch self {
            case .invalidPayload:
                String(localized: "Jet's helper is incomplete. Reinstall Jet from its latest release.")
            case .otherChannel:
                String(localized: "Another copy of Jet already runs this Mac's helper.")
            case .commandFailed:
                String(localized: "Jet couldn't start its helper on this Mac.")
            }
        }
    }
}
#endif
