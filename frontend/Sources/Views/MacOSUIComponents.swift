import SwiftUI

// MARK: - Constants
enum MacOSDesign {
    static let glassBG = Color.black.opacity(0.78)
    static let glassBorder = Color.white.opacity(0.12)
    static let sectionBG = Color.black.opacity(0.32)
    static let sectionBorder = Color.white.opacity(0.08)
    static let separator = Color.white.opacity(0.045)
    static let textPrimary = Color.white.opacity(0.92)
    static let textSecondary = Color.white.opacity(0.58)
    static let textTertiary = Color.white.opacity(0.42)
    
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
        Text(title.uppercased())
            .font(.system(size: 11, weight: .medium))
            .kerning(0.8)
            .foregroundColor(MacOSDesign.textSecondary)
            .padding(.horizontal, 16)
            .padding(.top, 18)
            .padding(.bottom, 6)
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
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(MacOSDesign.sectionBorder, lineWidth: 0.5)
        )
        .padding(.horizontal, 16)
        .padding(.bottom, 10)
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
                        .font(.system(size: 13))
                        .foregroundColor(MacOSDesign.textPrimary)
                    
                    if let hint = hint {
                        Text(hint)
                            .font(.system(size: 11))
                            .foregroundColor(MacOSDesign.textSecondary)
                    }
                }
                
                Spacer()
                
                content
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 13)
            
            if !isLast {
                Rectangle()
                    .fill(MacOSDesign.separator)
                    .frame(height: 0.5)
                    // .padding(.leading, 16) // Optional: indent separators like System Settings
            }
        }
    }
}

// MARK: - Controls

struct MacOSToggle: View {
    @Binding var isOn: Bool
    
    var body: some View {
        Button {
            withAnimation(.spring(response: 0.2, dampingFraction: 0.7)) {
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
                .font(.system(size: 13, design: .monospaced))
                .foregroundColor(MacOSDesign.textSecondary)
                .frame(width: 36, alignment: .trailing)
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
                    withAnimation(.spring(response: 0.25, dampingFraction: 0.8)) {
                        selection = option
                    }
                } label: {
                    Text(option.description)
                        .font(.system(size: 11))
                        .lineLimit(1)
                        .fixedSize(horizontal: true, vertical: false)
                        .foregroundColor(selection == option ? MacOSDesign.textPrimary : MacOSDesign.textSecondary)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .frame(minWidth: 80)
                        .background(
                            RoundedRectangle(cornerRadius: 6)
                                .fill(selection == option ? Color.white.opacity(0.15) : Color.clear)
                                .shadow(color: .black.opacity(0.2), radius: selection == option ? 2 : 0, y: 1)
                        )
                }
                .buttonStyle(.plain)
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
                Circle()
                    .fill(presetColor(for: preset))
                    .frame(width: 20, height: 20)
                    .overlay(
                        Circle().stroke(Color.white.opacity(0.7), lineWidth: selection == preset ? 2 : 0)
                    )
                    .scaleEffect(selection == preset ? 1.15 : 1.0)
                    .onTapGesture {
                        withAnimation(.spring(response: 0.15)) {
                            selection = preset
                        }
                    }
            }
            
            // Custom Color Option
            Circle()
                .fill(customColor)
                .frame(width: 20, height: 20)
                .overlay(
                    Circle().stroke(Color.white.opacity(0.7), lineWidth: selection == .custom ? 2 : 0)
                )
                .scaleEffect(selection == .custom ? 1.15 : 1.0)
                .onTapGesture {
                    withAnimation(.spring(response: 0.15)) {
                        selection = .custom
                    }
                }
        }
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
