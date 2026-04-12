import SwiftUI

enum SettingsTab: String, CaseIterable, Identifiable {
    case appearance = "Appearance"
    case behaviour = "Behaviour"
    case shortcuts = "Shortcuts"
    case about = "About"
    var id: String { self.rawValue }
}

struct SettingsView: View {
    @ObservedObject var settings = SettingsManager.shared
    @State private var activeTab: SettingsTab = .appearance
    
    var body: some View {
        ZStack {
            // Deep Dark Base Layer
            Color.black.opacity(0.85)
                .ignoresSafeArea()
            
            // Background Vibrancy
            VisualEffectView(material: .hudWindow, blendingMode: NSVisualEffectView.BlendingMode.behindWindow)
                .ignoresSafeArea()
                .opacity(0.9) // Slightly mute the vibrancy effect to maintain darkness
            
            // Gloss Sheen
            LinearGradient(
                stops: [
                    .init(color: Color.white.opacity(0.07), location: 0),
                    .init(color: .clear, location: 0.6)
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()
            .allowsHitTesting(false)
            
            VStack(spacing: 0) {
                Spacer().frame(height: 28) // Space for native title bar
                // Tab Bar
                HStack(spacing: 0) {
                    ForEach(SettingsTab.allCases) { tab in
                        Button {
                            withAnimation(.spring(response: 0.2)) {
                                activeTab = tab
                            }
                        } label: {
                            VStack(spacing: 0) {
                                Text(tab.rawValue)
                                    .font(.system(size: 12))
                                    .foregroundColor(activeTab == tab ? MacOSDesign.textPrimary : MacOSDesign.textSecondary)
                                    .padding(.vertical, 8)
                                    .padding(.horizontal, 12)
                                
                                // Active indicator line
                                Rectangle()
                                    .fill(activeTab == tab ? settings.accentColor.color : Color.clear)
                                    .frame(height: 1.5)
                            }
                            .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .overlay(
                    Rectangle()
                        .fill(MacOSDesign.separator)
                        .frame(height: 0.5),
                    alignment: .bottom
                )
                
                // Content
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        switch activeTab {
                        case .appearance:
                            appearancePanel
                        case .behaviour:
                            behaviourPanel
                        case .shortcuts:
                            shortcutsPanel
                        case .about:
                            aboutPanel
                        }
                    }
                    .padding(.top, 4)
                    .padding(.bottom, 24)
                }
                .frame(maxHeight: .infinity)
                
                // Footer
                VStack(spacing: 0) {
                    Rectangle()
                        .fill(MacOSDesign.separator)
                        .frame(height: 0.5)
                    
                    HStack {
                        Text("Changes saved automatically")
                            .font(.system(size: 11))
                            .foregroundColor(MacOSDesign.textTertiary)
                        
                        Spacer()
                        
                        Button {
                            NSApp.keyWindow?.close()
                        } label: {
                            Text("Done")
                                .font(.system(size: 13, weight: .medium))
                                .foregroundColor(.white)
                                .padding(.horizontal, 20)
                                .padding(.vertical, 6)
                                .background(settings.accentColor.color.opacity(0.85))
                                .cornerRadius(7)
                        }
                        .buttonStyle(.plain)
                        .onHover { inside in
                            // Simple hover effect
                        }
                    }
                    .padding(.horizontal, 18)
                    .padding(.vertical, 12)
                    .background(Color.black.opacity(0.12))
                }
            }
        }
        .frame(width: 440, height: 600)
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(MacOSDesign.glassBorder, lineWidth: 0.5)
        )
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .onChange(of: settings.displayMode) { _, _ in
            AppDelegate.shared.updateDisplayMode()
        }
    }
    
    // MARK: - Panels
    
    private var appearancePanel: some View {
        Group {
            MacOSSectionHeader(title: "Glass & Background")
            MacOSSection {
                MacOSRow(label: "Glass intensity", hint: "Blur thickness of background surfaces") {
                    MacOSSlider(value: $settings.glassIntensity, range: 0.1...1.0)
                }
                MacOSRow(label: "Tint opacity", hint: "Colour overlay strength", isLast: true) {
                    MacOSSlider(value: $settings.tintOpacity, range: 0.0...1.0)
                }
            }
            
            MacOSSectionHeader(title: "Accent Colour")
            MacOSSection {
                MacOSRow(label: "Preset") {
                    MacOSColorPicker(selection: $settings.accentColor, customColor: $settings.customColor)
                }
                if settings.accentColor == .custom {
                    MacOSRow(label: "Custom colour", isLast: true) {
                        ColorPicker("", selection: $settings.customColor, supportsOpacity: false)
                            .labelsHidden()
                            .scaleEffect(0.8)
                    }
                }
            }
            
            MacOSSectionHeader(title: "Layout")
            MacOSSection {
                MacOSRow(label: "Result density", hint: "Space between items in list view") {
                    MacOSSegmentedControl(selection: $settings.resultDensity, options: ResultDensity.allCases)
                }
                MacOSRow(label: "Show labels", hint: "Text labels below toolbar icons", isLast: true) {
                    MacOSToggle(isOn: $settings.showLabels)
                }
            }
        }
    }
    
    private var behaviourPanel: some View {
        Group {
            MacOSSectionHeader(title: "App Presence")
            MacOSSection {
                MacOSRow(label: "Show app in", hint: "Where the app icon appears") {
                    MacOSSegmentedControl(selection: $settings.displayMode, options: AppDisplayMode.allCases)
                }
                MacOSRow(label: "Launch at login", hint: "Start automatically on boot", isLast: true) {
                    MacOSToggle(isOn: $settings.launchAtLogin)
                }
            }
        }
    }
    
    private var shortcutsPanel: some View {
        Group {
            MacOSSectionHeader(title: "Shortcuts")
            MacOSSection {
                MacOSRow(label: "Global Toggle", hint: "⌥ Space", isLast: true) {
                    Text("Record...")
                        .font(.system(size: 11))
                        .foregroundColor(MacOSDesign.textSecondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(Color.white.opacity(0.1))
                        .cornerRadius(4)
                }
            }
        }
    }
    
    private var aboutPanel: some View {
        VStack(spacing: 16) {
            Spacer().frame(height: 40)
            
            if let icon = NSApp.applicationIconImage {
                Image(nsImage: icon)
                    .resizable()
                    .frame(width: 80, height: 80)
            } else {
                RoundedRectangle(cornerRadius: 16)
                    .fill(settings.accentColor.color)
                    .frame(width: 80, height: 80)
            }
            
            VStack(spacing: 4) {
                Text("LocalSearch")
                    .font(.system(size: 18, weight: .bold))
                Text("Version 1.0 (Build 1)")
                    .font(.system(size: 12))
                    .foregroundColor(MacOSDesign.textSecondary)
            }
            
            Text("© 2026 findohh. All rights reserved.")
                .font(.system(size: 11))
                .foregroundColor(MacOSDesign.textTertiary)
            
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}
