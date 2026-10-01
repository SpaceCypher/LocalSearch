import SwiftUI
import Combine
import ServiceManagement

enum ResultDensity: String, CaseIterable, Identifiable, CustomStringConvertible {
    case comfortable = "Comfortable"
    case compact = "Compact"
    var id: String { self.rawValue }
    var description: String { self.rawValue }
}

enum AppDisplayMode: String, CaseIterable, Identifiable, CustomStringConvertible {
    case dock = "Dock"
    case menuBar = "Menu Bar"
    case both = "Both"
    var id: String { self.rawValue }
    var description: String { self.rawValue }
}

enum AccentColor: String, CaseIterable, Identifiable {
    case blue = "Blue"
    case purple = "Purple"
    case monochrome = "Monochrome"
    case custom = "Custom"
    var id: String { self.rawValue }
    
    var color: Color {
        switch self {
        case .blue: return .blue
        case .purple: return .purple
        case .monochrome: return .primary
        case .custom:
            let hex = UserDefaults.standard.string(forKey: "customAccentColor") ?? "#007AFF"
            return Color(hex: hex)
        }
    }
}

final class SettingsManager: ObservableObject {
    static let shared = SettingsManager()
    
    @AppStorage("glassIntensity") var glassIntensity: Double = 0.6
    @AppStorage("resultDensity") var resultDensity: ResultDensity = .comfortable
    @AppStorage("accentColor") var accentColor: AccentColor = .blue
    @AppStorage("customAccentColor") var customAccentHex: String = "#007AFF"
    @AppStorage("displayMode") var displayMode: AppDisplayMode = .dock
    @AppStorage("tintOpacity") var tintOpacity: Double = 0.3
    @AppStorage("showLabels") var showLabels: Bool = true
    @AppStorage("launchAtLogin") var launchAtLogin: Bool = false
    
    private init() {
        indexing = IndexingSettings.load(from: .standard) ?? IndexingSettings()
    }
    
    // MARK: - Indexing
    
    private let defaults = UserDefaults.standard
    
    /// What the engine indexes. Every change is saved and sent to the engine,
    /// which re-scans in the background.
    @Published var indexing: IndexingSettings {
        didSet {
            guard indexing != oldValue else { return }
            indexing.save(to: defaults)
            Backend.shared.applyIndexingConfig(indexing.json)
        }
    }
    
    /// On launch, send the saved settings to the engine, but only if the user
    /// has ever changed them: otherwise the engine's own defaults stand.
    func pushIndexingConfigIfCustomised() {
        if IndexingSettings.load(from: defaults) != nil {
            Backend.shared.applyIndexingConfig(indexing.json)
        }
    }
    
    // MARK: - Launch at login
    
    /// Why the last attempt to change "Launch at login" failed, if it did
    @Published var launchAtLoginError: String?
    
    /// Register or unregister the app as a login item. Reverts the stored
    /// setting and explains if macOS refuses.
    func setLaunchAtLogin(_ enabled: Bool) {
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            launchAtLogin = enabled
            launchAtLoginError = nil
        } catch {
            launchAtLogin = SMAppService.mainApp.status == .enabled
            launchAtLoginError = "Couldn\u{2019}t change this. Move LocalSearch to the Applications folder and try again."
        }
        objectWillChange.send()
    }
    
    var customColor: Color {
        get { Color(hex: customAccentHex) }
        set { customAccentHex = newValue.toHex() ?? "#007AFF" }
    }
}

// MARK: - Indexing settings

/// Mirrors `EngineConfig` in `src/engine.rs`
struct IndexingSettings: Equatable, Codable {
    var roots: [String] = IndexingSettings.defaultRoots
    var excludes: [String] = IndexingSettings.defaultExcludes
    var maxDepth: Int = 12
    var indexHidden: Bool = false
    var indexContent: Bool = true
    
    enum CodingKeys: String, CodingKey {
        case roots, excludes
        case maxDepth = "max_depth"
        case indexHidden = "index_hidden"
        case indexContent = "index_content"
    }
    
    static let depthRange = 1...32
    private static let storageKey = "indexingSettings"
    
    static var defaultRoots: [String] {
        ["Downloads", "Documents", "Desktop"].map { "~/" + $0 }
    }
    
    static let defaultExcludes = [
        "node_modules", "target", "dist", "build", "tmp", "temp", "cache", "caches",
        "deriveddata", "modulecache", "trash", "__pycache__", "venv", "site-packages", "pods",
    ]
    
    /// The JSON the engine's `localsearch_configure` expects
    var json: String {
        let data = (try? JSONEncoder().encode(self)) ?? Data("{}".utf8)
        return String(decoding: data, as: UTF8.self)
    }
    
    static func load(from defaults: UserDefaults) -> IndexingSettings? {
        guard let data = defaults.data(forKey: storageKey) else { return nil }
        return try? JSONDecoder().decode(IndexingSettings.self, from: data)
    }
    
    func save(to defaults: UserDefaults) {
        defaults.set(try? JSONEncoder().encode(self), forKey: Self.storageKey)
    }
    
    /// Add a folder, stored with `~` for the home directory. Returns false if
    /// it is already covered by a folder in the list.
    @discardableResult
    mutating func addRoot(_ url: URL) -> Bool {
        let path = Self.abbreviate(url.standardizedFileURL.path)
        let covered = roots.contains { path == $0 || path.hasPrefix($0 + "/") }
        guard !covered else { return false }
        // A newly added parent replaces the children it contains
        roots.removeAll { $0.hasPrefix(path + "/") }
        roots.append(path)
        return true
    }
    
    /// Add a name to skip. Returns false for blanks, paths and duplicates.
    @discardableResult
    mutating func addExclude(_ name: String) -> Bool {
        let cleaned = name.trimmingCharacters(in: .whitespaces).lowercased()
        guard !cleaned.isEmpty, !cleaned.contains("/"), !excludes.contains(cleaned) else { return false }
        excludes.append(cleaned)
        return true
    }
    
    static func abbreviate(_ path: String) -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        if path == home { return "~" }
        return path.hasPrefix(home + "/") ? "~" + path.dropFirst(home.count) : path
    }
}

// MARK: - Color Hex Helpers
extension Color {
    init(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 3: // RGB (12-bit)
            (a, r, g, b) = (255, (int >> 8) * 17, (int >> 4 & 0xF) * 17, (int & 0xF) * 17)
        case 6: // RGB (24-bit)
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8: // ARGB (32-bit)
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            (a, r, g, b) = (1, 1, 1, 0)
        }
        self.init(.sRGB, red: Double(r) / 255, green: Double(g) / 255, blue: Double(b) / 255, opacity: Double(a) / 255)
    }
    
    func toHex() -> String? {
        guard let components = NSColor(self).usingColorSpace(.sRGB)?.cgColor.components, components.count >= 3 else {
            return nil
        }
        let r = Float(components[0])
        let g = Float(components[1])
        let b = Float(components[2])
        return String(format: "#%02lX%02lX%02lX", lroundf(r * 255), lroundf(g * 255), lroundf(b * 255))
    }
}
