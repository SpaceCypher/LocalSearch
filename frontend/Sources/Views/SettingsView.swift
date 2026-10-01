import SwiftUI

enum SettingsTab: String, CaseIterable, Identifiable {
    case indexing = "Indexing"
    case appearance = "Appearance"
    case behaviour = "Behaviour"
    case shortcuts = "Shortcuts"
    case about = "About"
    var id: String { self.rawValue }
}

struct SettingsView: View {
    @ObservedObject var settings = SettingsManager.shared
    @State private var activeTab: SettingsTab = .indexing
    @StateObject private var indexStatus = IndexStatusModel()
    @State private var newExclude = ""
    @State private var folderNotice: String?
    
    var body: some View {
        ZStack {
            // A plain dark surface: settings are read and edited, not admired
            Color(white: 0.11)
                .ignoresSafeArea()
            
            VStack(spacing: 0) {
                Spacer().frame(height: 28) // Space for native title bar
                // Tab Bar
                HStack(spacing: 0) {
                    ForEach(SettingsTab.allCases) { tab in
                        Button {
                            activeTab = tab
                        } label: {
                            VStack(spacing: 0) {
                                // Same weight in both states and never wrapped, so
                                // selecting a tab cannot change the bar's layout
                                Text(tab.rawValue)
                                    .font(.system(size: DS.TextSize.base))
                                    .lineLimit(1)
                                    .fixedSize()
                                    .foregroundColor(activeTab == tab ? MacOSDesign.textPrimary : MacOSDesign.textSecondary)
                                    .padding(.vertical, DS.Space.s2)
                                
                                // Active indicator line
                                Rectangle()
                                    .fill(activeTab == tab ? settings.accentColor.color : Color.clear)
                                    .frame(height: 2)
                            }
                            .frame(maxWidth: .infinity)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(activeTab == tab ? [.isSelected] : [])
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
                        case .indexing:
                            indexingPanel
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
                        Text(activeTab == .indexing ? "Changes are saved and applied as you make them" : "Changes are saved automatically")
                            .font(.system(size: DS.TextSize.xs))
                            .foregroundColor(MacOSDesign.textSecondary)
                        
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
                        .keyboardShortcut(.defaultAction)
                    }
                    .padding(.horizontal, DS.Space.s4)
                    .padding(.vertical, DS.Space.s3)
                }
            }
        }
        .frame(width: 480, height: 600)
        .preferredColorScheme(.dark)
        .onAppear { indexStatus.start() }
        .onDisappear { indexStatus.stop() }
        .onChange(of: settings.displayMode) { _, _ in
            AppDelegate.shared.updateDisplayMode()
        }
    }
    
    // MARK: - Panels
    
    private var appearancePanel: some View {
        Group {
            MacOSSectionHeader(title: "Search panel")
            MacOSSection {
                MacOSRow(label: "Background opacity", hint: "Higher is easier to read over busy windows", isLast: true) {
                    MacOSSlider(value: $settings.glassIntensity, range: 0.1...1.0)
                        .accessibilityLabel("Background opacity")
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
                MacOSRow(label: "Result density", hint: "Compact fits more results on screen", isLast: true) {
                    MacOSSegmentedControl(selection: $settings.resultDensity, options: ResultDensity.allCases)
                }
            }
        }
    }
    
    private var behaviourPanel: some View {
        Group {
            MacOSSectionHeader(title: "App Presence")
            MacOSSection {
                MacOSRow(label: "Show app in", hint: "Where the app icon appears") {
                    // Applied the moment it is chosen, not on the next redraw
                    MacOSSegmentedControl(
                        selection: Binding(
                            get: { settings.displayMode },
                            set: { mode in
                                settings.objectWillChange.send()
                                settings.displayMode = mode
                                AppDelegate.shared.updateDisplayMode()
                            }
                        ),
                        options: AppDisplayMode.allCases
                    )
                }
                MacOSRow(
                    label: "Launch at login",
                    hint: settings.launchAtLoginError ?? "Start LocalSearch when you log in",
                    isLast: true
                ) {
                    MacOSToggle(
                        isOn: Binding(
                            get: { settings.launchAtLogin },
                            set: { settings.setLaunchAtLogin($0) }
                        ),
                        label: "Launch at login"
                    )
                }
            }
        }
    }
    
    private var shortcutsPanel: some View {
        Group {
            MacOSSectionHeader(title: "Anywhere")
            MacOSSection {
                shortcutRow("Show or hide LocalSearch", keys: "⌥ Space", isLast: true)
            }
            
            MacOSSectionHeader(title: "In the search panel")
            MacOSSection {
                shortcutRow("Move through results", keys: "↑ ↓")
                shortcutRow("Open", keys: "↩")
                shortcutRow("Reveal in Finder", keys: "⌘ ↩")
                shortcutRow("Copy path", keys: "⌥ ↩")
                shortcutRow("Show or hide details", keys: "⇥")
                shortcutRow("Previous searches (empty field)", keys: "↑")
                shortcutRow("Settings", keys: "⌘ ,")
                shortcutRow("Close", keys: "esc", isLast: true)
            }
            
            MacOSSectionHeader(title: "Search filters")
            MacOSSection {
                shortcutRow("Only a file type or kind", keys: "kind:pdf")
                shortcutRow("Only inside a folder", keys: "in:projects")
                shortcutRow("Leave out names containing a word", keys: "-draft", isLast: true)
            }
        }
    }
    
    private func shortcutRow(_ label: String, keys: String, isLast: Bool = false) -> some View {
        MacOSRow(label: label, isLast: isLast) {
            Text(keys)
                .font(.system(size: DS.TextSize.sm, design: .monospaced))
                .foregroundColor(MacOSDesign.textPrimary)
                .padding(.horizontal, DS.Space.s2)
                .padding(.vertical, 3)
                .background(DS.Palette.surfaceRaised)
                .cornerRadius(DS.Radius.control)
        }
        .accessibilityElement(children: .combine)
    }
    
    // MARK: - Indexing
    
    private var indexingPanel: some View {
        Group {
            MacOSSectionHeader(title: "Status")
            MacOSSection {
                indexStatusRow
            }
            
            MacOSSectionHeader(title: "Folders to search")
            MacOSSection {
                if settings.indexing.roots.isEmpty {
                    Text("No folders yet. Add one and LocalSearch will index everything inside it.")
                        .font(.system(size: DS.TextSize.sm))
                        .foregroundColor(MacOSDesign.textSecondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(DS.Space.s4)
                    Rectangle().fill(MacOSDesign.separator).frame(height: 0.5)
                }
                ForEach(settings.indexing.roots, id: \.self) { root in
                    folderRow(root)
                }
                HStack(spacing: DS.Space.s3) {
                    Button {
                        addFolders()
                    } label: {
                        Label("Add folder…", systemImage: "plus")
                            .font(.system(size: DS.TextSize.base, weight: .medium))
                            .foregroundColor(settings.accentColor.color)
                    }
                    .buttonStyle(.plain)
                    
                    if let folderNotice {
                        Text(folderNotice)
                            .font(.system(size: DS.TextSize.xs))
                            .foregroundColor(MacOSDesign.textSecondary)
                            .lineLimit(1)
                    }
                    Spacer()
                }
                .padding(.horizontal, DS.Space.s4)
                .padding(.vertical, DS.Space.s3)
            }
            
            MacOSSectionHeader(title: "What to index")
            MacOSSection {
                MacOSRow(label: "Search inside files", hint: "Text, code and PDF contents, not just names") {
                    MacOSToggle(isOn: $settings.indexing.indexContent, label: "Search inside files")
                }
                MacOSRow(label: "Include hidden files", hint: "Names that start with a dot") {
                    MacOSToggle(isOn: $settings.indexing.indexHidden, label: "Include hidden files")
                }
                MacOSRow(label: "Folder depth", hint: "Levels below each folder that are indexed", isLast: true) {
                    Stepper(value: $settings.indexing.maxDepth, in: IndexingSettings.depthRange) {
                        Text("\(settings.indexing.maxDepth)")
                            .font(.system(size: DS.TextSize.base).monospacedDigit())
                            .foregroundColor(MacOSDesign.textPrimary)
                            .frame(minWidth: 20, alignment: .trailing)
                    }
                    .accessibilityLabel("Folder depth")
                    .accessibilityValue("\(settings.indexing.maxDepth) levels")
                }
            }
            
            MacOSSectionHeader(title: "Skipped names")
            MacOSSection {
                VStack(alignment: .leading, spacing: DS.Space.s3) {
                    Text("Files and folders with these names are never indexed.")
                        .font(.system(size: DS.TextSize.xs))
                        .foregroundColor(MacOSDesign.textSecondary)
                    
                    FlowLayout(spacing: DS.Space.s2) {
                        ForEach(settings.indexing.excludes, id: \.self) { name in
                            excludeChip(name)
                        }
                    }
                    
                    HStack(spacing: DS.Space.s2) {
                        TextField("Add a name, e.g. node_modules", text: $newExclude)
                            .textFieldStyle(.plain)
                            .font(.system(size: DS.TextSize.base))
                            .padding(.horizontal, DS.Space.s2)
                            .frame(height: 28)
                            .background(DS.Palette.surfaceRaised)
                            .cornerRadius(DS.Radius.control)
                            .onSubmit(addExclude)
                            .accessibilityLabel("Name to skip")
                        
                        Button("Add", action: addExclude)
                            .buttonStyle(.plain)
                            .font(.system(size: DS.TextSize.base, weight: .medium))
                            .foregroundColor(newExcludeIsValid ? settings.accentColor.color : MacOSDesign.textTertiary)
                            .disabled(!newExcludeIsValid)
                    }
                    
                    if settings.indexing.excludes != IndexingSettings.defaultExcludes {
                        Button("Restore default list") {
                            settings.indexing.excludes = IndexingSettings.defaultExcludes
                        }
                        .buttonStyle(.plain)
                        .font(.system(size: DS.TextSize.xs))
                        .foregroundColor(MacOSDesign.textSecondary)
                    }
                }
                .padding(DS.Space.s4)
            }
        }
    }
    
    @ViewBuilder
    private var indexStatusRow: some View {
        VStack(alignment: .leading, spacing: DS.Space.s2) {
            if let reason = Backend.shared.unavailableReason {
                statusLine(color: DS.Palette.danger, text: "Search engine not loaded")
                Text(reason)
                    .font(.system(size: DS.TextSize.xs))
                    .foregroundColor(MacOSDesign.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let progress = indexStatus.progress {
                statusLine(color: DS.Palette.warning, text: "Indexing")
                IndexProgressView(progress: progress)
                Text("You can keep searching; results fill in as indexing proceeds.")
                    .font(.system(size: DS.TextSize.xs))
                    .foregroundColor(MacOSDesign.textSecondary)
            } else {
                statusLine(color: DS.Palette.success, text: indexStatus.summary ?? "Checking…")
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(DS.Space.s4)
    }
    
    /// Status is always a dot plus words, never colour alone
    private func statusLine(color: Color, text: String) -> some View {
        HStack(spacing: DS.Space.s2) {
            Circle().fill(color).frame(width: 7, height: 7).accessibilityHidden(true)
            Text(text)
                .font(.system(size: DS.TextSize.base).monospacedDigit())
                .foregroundColor(MacOSDesign.textPrimary)
        }
    }
    
    private func folderRow(_ root: String) -> some View {
        let exists = FileManager.default.fileExists(atPath: (root as NSString).expandingTildeInPath)
        return VStack(spacing: 0) {
            HStack(spacing: DS.Space.s3) {
                Image(systemName: exists ? "folder" : "exclamationmark.triangle")
                    .font(.system(size: DS.TextSize.base))
                    .foregroundColor(exists ? MacOSDesign.textSecondary : DS.Palette.warning)
                    .frame(width: 16)
                    .accessibilityHidden(true)
                
                VStack(alignment: .leading, spacing: 2) {
                    Text(root)
                        .font(.system(size: DS.TextSize.base))
                        .foregroundColor(MacOSDesign.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(root)
                    if !exists {
                        Text("Folder not found. It is skipped until it reappears.")
                            .font(.system(size: DS.TextSize.xs))
                            .foregroundColor(DS.Palette.warning)
                    }
                }
                
                Spacer()
                
                Button {
                    removeFolder(root)
                } label: {
                    Image(systemName: "minus.circle")
                        .font(.system(size: DS.TextSize.lg))
                        .foregroundColor(MacOSDesign.textSecondary)
                        .frame(width: 24, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Stop indexing this folder")
                .accessibilityLabel("Remove \(root)")
            }
            .padding(.horizontal, DS.Space.s4)
            .frame(minHeight: 40)
            
            Rectangle().fill(MacOSDesign.separator).frame(height: 0.5)
        }
    }
    
    private func excludeChip(_ name: String) -> some View {
        HStack(spacing: DS.Space.s1) {
            Text(name)
                .font(.system(size: DS.TextSize.sm, design: .monospaced))
                .foregroundColor(MacOSDesign.textPrimary)
                .lineLimit(1)
            Button {
                settings.indexing.excludes.removeAll { $0 == name }
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundColor(MacOSDesign.textSecondary)
                    .frame(width: 16, height: 16)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Stop skipping \(name)")
        }
        .padding(.leading, DS.Space.s2)
        .padding(.trailing, 2)
        .padding(.vertical, 2)
        .background(DS.Palette.surfaceRaised)
        .cornerRadius(DS.Radius.control)
    }
    
    private var newExcludeIsValid: Bool {
        var probe = settings.indexing
        return probe.addExclude(newExclude)
    }
    
    private func addExclude() {
        if settings.indexing.addExclude(newExclude) {
            newExclude = ""
        }
    }
    
    private func addFolders() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        panel.message = "Choose folders for LocalSearch to index"
        guard panel.runModal() == .OK else { return }
        
        var skipped = 0
        for url in panel.urls where !settings.indexing.addRoot(url) {
            skipped += 1
        }
        folderNotice = skipped == 0 ? nil : "Already covered by a folder in the list"
    }
    
    /// Removing is undone by adding the folder back, so it needs no confirmation.
    private func removeFolder(_ root: String) {
        settings.indexing.roots.removeAll { $0 == root }
        folderNotice = "Removed \(root)"
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
                .font(.system(size: DS.TextSize.xs))
                .foregroundColor(MacOSDesign.textSecondary)
            
            Spacer()
        }
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Index status for the settings window

/// Polls the engine while the settings window is open
@MainActor
final class IndexStatusModel: ObservableObject {
    @Published var progress: IndexProgress?
    @Published var summary: String?
    private var task: Task<Void, Never>?
    
    func start() {
        guard task == nil else { return }
        task = Task { [weak self] in
            for await progress in Backend.shared.indexProgress() {
                guard let self else { return }
                self.progress = progress
                self.summary = Backend.shared.indexSummary()
            }
        }
    }
    
    func stop() {
        task?.cancel()
        task = nil
    }
}

// MARK: - Wrapping layout for chips

struct FlowLayout: Layout {
    var spacing: CGFloat = 8
    
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        let rows = arrange(subviews: subviews, width: width)
        return CGSize(width: proposal.width ?? rows.width, height: rows.height)
    }
    
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let arrangement = arrange(subviews: subviews, width: bounds.width)
        for (subview, origin) in zip(subviews, arrangement.origins) {
            subview.place(
                at: CGPoint(x: bounds.minX + origin.x, y: bounds.minY + origin.y),
                proposal: .unspecified
            )
        }
    }
    
    private func arrange(subviews: Subviews, width: CGFloat) -> (origins: [CGPoint], width: CGFloat, height: CGFloat) {
        var origins: [CGPoint] = []
        var (x, y, rowHeight, maxWidth): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            origins.append(CGPoint(x: x, y: y))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
            maxWidth = max(maxWidth, x - spacing)
        }
        return (origins, maxWidth, y + rowHeight)
    }
}
