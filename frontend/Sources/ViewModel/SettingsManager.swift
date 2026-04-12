import SwiftUI
import Combine

enum ResultDensity: String, CaseIterable, Identifiable {
    case comfortable = "Comfortable"
    case compact = "Compact"
    var id: String { self.rawValue }
}

enum AppDisplayMode: String, CaseIterable, Identifiable {
    case dock = "Dock"
    case menuBar = "Menu Bar"
    case both = "Both"
    var id: String { self.rawValue }
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
    
    private init() {}
    
    var customColor: Color {
        get { Color(hex: customAccentHex) }
        set { customAccentHex = newValue.toHex() ?? "#007AFF" }
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
