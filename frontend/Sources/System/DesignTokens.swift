import SwiftUI

/// Design tokens for the app. Views take spacing, type sizes, radii, colours
/// and motion from here rather than using literal values, so the search panel
/// and the settings window stay consistent.
enum DS {
    /// 4pt spacing scale
    enum Space {
        static let s1: CGFloat = 4
        static let s2: CGFloat = 8
        static let s3: CGFloat = 12
        static let s4: CGFloat = 16
        static let s5: CGFloat = 20
        static let s6: CGFloat = 24
    }

    /// UI type scale. Hierarchy comes mostly from weight and colour, not size.
    enum TextSize {
        static let xs: CGFloat = 11
        static let sm: CGFloat = 12
        static let base: CGFloat = 13
        static let lg: CGFloat = 15
        static let query: CGFloat = 20
    }

    enum Radius {
        static let control: CGFloat = 6
        static let row: CGFloat = 8
        static let section: CGFloat = 10
    }

    /// Colours for the dark, translucent surfaces the app draws on.
    /// Text opacities are chosen to stay above 4.5:1 against those surfaces.
    enum Palette {
        static let text = Color.white.opacity(0.92)
        static let textMuted = Color.white.opacity(0.64)
        static let textFaint = Color.white.opacity(0.50)
        static let surface = Color.white.opacity(0.05)
        static let surfaceRaised = Color.white.opacity(0.10)
        static let border = Color.white.opacity(0.12)
        static let borderStrong = Color.white.opacity(0.22)
        static let separator = Color.white.opacity(0.08)
        static let success = Color(red: 0.30, green: 0.82, blue: 0.47)
        static let warning = Color(red: 0.98, green: 0.76, blue: 0.30)
        static let danger = Color(red: 1.0, green: 0.45, blue: 0.42)
    }

    /// Motion explains a change and gets out of the way: 100–200ms, ease-out,
    /// and nothing at all when the user has asked for reduced motion.
    enum Motion {
        static let instant: Double = 0.10
        static let fast: Double = 0.15
        static let base: Double = 0.20

        static func ease(_ duration: Double = fast) -> Animation? {
            AnimationTokens.isReduceMotionEnabled ? nil : .easeOut(duration: duration)
        }
    }
}

/// A keyboard shortcut rendered as a key cap followed by what it does
struct KeyHint: View {
    let keys: String
    let label: String

    var body: some View {
        HStack(spacing: DS.Space.s1) {
            Text(keys)
                .font(.system(size: DS.TextSize.xs, weight: .medium))
                .foregroundColor(DS.Palette.textMuted)
                .padding(.horizontal, 5)
                .padding(.vertical, 1)
                .background(DS.Palette.surfaceRaised)
                .cornerRadius(4)
            Text(label)
                .font(.system(size: DS.TextSize.xs))
                .foregroundColor(DS.Palette.textFaint)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(label): \(keys)")
    }
}
