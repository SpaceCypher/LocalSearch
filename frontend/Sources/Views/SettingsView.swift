import SwiftUI

struct SettingsView: View {
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                Text("Styles & Aesthetics")
                    .font(.title2)
                    .bold()
                Spacer()
            }
            .padding(.top, 32)
            .padding(.horizontal, 32)
            .padding(.bottom, 16)
            
            // Scrollable Content
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    Form {
                        // Glass Intensity
                        VStack(alignment: .leading, spacing: 10) {
                            HStack {
                                Text("Glass Intensity")
                                Spacer()
                                Text("\(Int(settings.glassIntensity * 100))%")
                                    .foregroundColor(.secondary)
                                    .font(.system(.body, design: .monospaced))
                            }
                            Slider(value: $settings.glassIntensity, in: 0.1...1.0)
                                .tint(settings.accentColor.color)
                            Text("Adjust how 'thick' or transparent the blur background appears.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .padding(.vertical, 4)
                        
                        Divider()
                        
                        // Result Density
                        VStack(alignment: .leading, spacing: 10) {
                            Text("Result Density")
                            Picker("Result Density", selection: $settings.resultDensity) {
                                ForEach(ResultDensity.allCases) { density in
                                    Text(density.rawValue).tag(density)
                                }
                            }
                            .pickerStyle(.segmented)
                            Text(settings.resultDensity == .comfortable ? "Comfortable view with large icons and more space." : "Compact view to see more results at once.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .padding(.vertical, 4)
                        
                        Divider()
                        
                        // Accent Color
                        VStack(alignment: .leading, spacing: 10) {
                            Text("Accent Color")
                            Picker("Accent Color", selection: $settings.accentColor) {
                                ForEach(AccentColor.allCases) { color in
                                    Text(color.rawValue).tag(color)
                                }
                            }
                            .pickerStyle(.segmented)
                            
                            if settings.accentColor == .custom {
                                ColorPicker("Custom Accent", selection: $settings.customColor, supportsOpacity: false)
                                    .padding(.top, 4)
                            }
                            
                            Text("Select the primary color used for highlights and buttons.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .padding(.vertical, 4)
                        
                        Divider()
                        
                        // App Presence
                        VStack(alignment: .leading, spacing: 10) {
                            Text("App Presence")
                            Picker("App Presence", selection: $settings.displayMode) {
                                ForEach(AppDisplayMode.allCases) { mode in
                                    Text(mode.rawValue).tag(mode)
                                }
                            }
                            .pickerStyle(.segmented)
                            
                            Text(settings.displayMode == .menuBar ? "App hides from the Dock and stays only in your Menu Bar." : settings.displayMode == .dock ? "App stays in the Dock like a standard application." : "App appears in both the Dock and Menu Bar for easy access.")
                                .font(.caption)
                                .foregroundColor(.secondary)
                        }
                        .padding(.vertical, 4)
                    }
                    .formStyle(.grouped)
                }
                .padding(.horizontal, 32)
                .padding(.bottom, 24)
            }
            
            Divider()
            
            // Fixed Bottom Bar
            HStack {
                Text("Settings are saved automatically")
                    .font(.caption)
                    .foregroundColor(.secondary)
                
                Spacer()
                
                Button("Done") {
                    // Close the current window (Settings)
                    NSApp.keyWindow?.close()
                }
                .keyboardShortcut(.defaultAction)
                .controlSize(.large)
            }
            .padding(.horizontal, 32)
            .padding(.vertical, 16)
            .background(Color(NSColor.windowBackgroundColor))
        }
        .frame(width: 440, height: 600)
        .background(VisualEffectView(material: .menu, blendingMode: .behindWindow).ignoresSafeArea())
        .onChange(of: settings.displayMode) { _, _ in
            // Immediately trigger a refresh of the Dock/Menu Bar state
            AppDelegate.shared.updateDisplayMode()
        }
    }
}

// Reuse the native blur helper if needed, or define a simple one for settings
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
