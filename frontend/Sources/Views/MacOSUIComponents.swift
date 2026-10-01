import SwiftUI

// MARK: - Constants
enum MacOSDesign {
    static let glassBG = Color(white: 0.1, opacity: 0.72)
    static let glassBorder = DS.Palette.borderStrong
    static let sectionBG = DS.Palette.surface
    static let sectionBorder = DS.Palette.border
    static let separator = DS.Palette.separator
    static let textPrimary = DS.Palette.text
    static let textSecondary = DS.Palette.textMuted
    static let textTertiary = DS.Palette.textFaint
    
    // Apple System Green for Toggles
    static let systemGreen = Color(red: 48/255, green: 209/255, blue: 88/255)
}

struct VisualEffectView: NSViewRepresentable {
    let material: NSVisualEffectView.Material
    let blendingMode: NSVisualEffectView.BlendingMode
    
    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = blendingMode
        view.state = .active
        return view
    }
    
    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = material
        nsView.blendingMode = blendingMode
    }
}

// MARK: - UI Components

struct MacOSSectionHeader: View {
    let title: String
    
    var body: some View {
        Text(title)
            .font(.system(size: DS.TextSize.sm, weight: .semibold))
            .foregroundColor(MacOSDesign.textSecondary)
            .padding(.horizontal, DS.Space.s4 + DS.Space.s1)
            .padding(.top, DS.Space.s4)
            .padding(.bottom, DS.Space.s2)
            .accessibilityAddTraits(.isHeader)
    }
}

struct MacOSSection<Content: View>: View {
    let content: Content
    
    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }
    
    var body: some View {
        VStack(spacing: 0) {
            content
        }
        .background(MacOSDesign.sectionBG)
        .cornerRadius(DS.Radius.section)
        .overlay(
            RoundedRectangle(cornerRadius: DS.Radius.section)
                .stroke(MacOSDesign.sectionBorder, lineWidth: 0.5)
        )
        .padding(.horizontal, DS.Space.s4)
        .padding(.bottom, DS.Space.s2)
    }
}

struct MacOSRow<Content: View>: View {
    let label: String
    let hint: String?
    let content: Content
    let isLast: Bool
    
    init(
        label: String,
        hint: String? = nil,
        isLast: Bool = false,
        @ViewBuilder content: () -> Content
    ) {
        self.label = label
        self.hint = hint
        self.isLast = isLast
        self.content = content()
    }
    
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(label)
                        .font(.system(size: DS.TextSize.base))
                        .foregroundColor(MacOSDesign.textPrimary)
                    
                    if let hint = hint {
                        Text(hint)
                            .font(.system(size: DS.TextSize.xs))
                            .foregroundColor(MacOSDesign.textSecondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                
                Spacer()
                
                content
            }
            .padding(.horizontal, DS.Space.s4)
            .padding(.vertical, DS.Space.s3)
            
            if !isLast {
                Rectangle()
                    .fill(MacOSDesign.separator)
                    .frame(height: 0.5)
            }
        }
    }
}

// MARK: - Controls

struct MacOSToggle: View {
    @Binding var isOn: Bool
    var label: String = ""
    
    var body: some View {
        Button {
            withAnimation(DS.Motion.ease()) {
                isOn.toggle()
            }
        } label: {
            ZStack {
                Capsule()
                    .fill(isOn ? MacOSDesign.systemGreen : Color.white.opacity(0.15))
                    .frame(width: 36, height: 20)
                
                Circle()
                    .fill(.white)
                    .shadow(radius: 1, y: 1)
                    .frame(width: 16, height: 16)
                    .offset(x: isOn ? 8 : -8)
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityValue(isOn ? "On" : "Off")
        .accessibilityAddTraits(.isToggle)
    }
}

struct MacOSSlider: View {
    @Binding var value: Double
    let range: ClosedRange<Double>
    
    var body: some View {
        HStack(spacing: 12) {
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    // Track
                    Capsule()
                        .fill(Color.white.opacity(0.15))
                        .frame(height: 3)
                    
                    // Thumb
                    Circle()
                        .fill(.white)
                        .shadow(color: .black.opacity(0.5), radius: 3, y: 1)
                        .frame(width: 16, height: 16)
                        .offset(x: geo.size.width * CGFloat((value - range.lowerBound) / (range.upperBound - range.lowerBound)) - 8)
                }
                .frame(maxHeight: .infinity)
                .background(Color.white.opacity(0.001)) // Capture hits to prevent window drag
                .contentShape(Rectangle()) 
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { drag in
                            let percent = Double(drag.location.x / geo.size.width)
                            let newValue = range.lowerBound + percent * (range.upperBound - range.lowerBound)
                            value = min(max(newValue, range.lowerBound), range.upperBound)
                        }
                )
            }
            .frame(height: 20)
            .frame(width: 120)
            
            Text("\(Int(value * 100))%")
                .font(.system(size: DS.TextSize.sm).monospacedDigit())
                .foregroundColor(MacOSDesign.textSecondary)
                .frame(width: 36, alignment: .trailing)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityValue("\(Int(value * 100)) percent")
        .accessibilityAdjustableAction { direction in
            let step = (range.upperBound - range.lowerBound) / 10
            switch direction {
            case .increment: value = min(value + step, range.upperBound)
            case .decrement: value = max(value - step, range.lowerBound)
            @unknown default: break
            }
        }
    }
}

struct MacOSSegmentedControl<T: Identifiable & Equatable & CustomStringConvertible>: View {
    @Binding var selection: T
    let options: [T]
    
    var body: some View {
        HStack(spacing: 2) {
            ForEach(options) { option in
                Button {
                    withAnimation(DS.Motion.ease()) {
                        selection = option
                    }
                } label: {
                    Text(option.description)
                        .font(.system(size: DS.TextSize.sm))
                        .lineLimit(1)
                        .fixedSize(horizontal: true, vertical: false)
                        .foregroundColor(selection == option ? MacOSDesign.textPrimary : MacOSDesign.textSecondary)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .frame(minWidth: 72)
                        .background(
                            RoundedRectangle(cornerRadius: DS.Radius.control)
                                .fill(selection == option ? Color.white.opacity(0.16) : Color.clear)
                        )
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == option ? [.isSelected] : [])
            }
        }
        .padding(2)
        .background(Color.white.opacity(0.08))
        .cornerRadius(8)
    }
}

struct MacOSColorPicker: View {
    @Binding var selection: AccentColor
    @Binding var customColor: Color
    
    let presets: [AccentColor] = [.blue, .purple, .monochrome]
    
    var body: some View {
        HStack(spacing: 10) {
            ForEach(presets) { preset in
                swatch(color: presetColor(for: preset), option: preset)
            }
            
            // Custom Color Option
            swatch(color: customColor, option: .custom)
        }
    }

    private func swatch(color: Color, option: AccentColor) -> some View {
        Button {
            withAnimation(DS.Motion.ease()) {
                selection = option
            }
        } label: {
            Circle()
                .fill(color)
                .frame(width: 20, height: 20)
                .overlay(
                    Circle().stroke(Color.white.opacity(0.85), lineWidth: selection == option ? 2 : 0)
                )
        }
        .buttonStyle(.plain)
        .accessibilityLabel(option.rawValue)
        .accessibilityAddTraits(selection == option ? [.isSelected] : [])
    }
    
    private func presetColor(for preset: AccentColor) -> Color {
        switch preset {
        case .blue: return .blue
        case .purple: return .purple
        case .monochrome: return Color(white: 0.6)
        case .custom: return customColor
        }
    }
}
