import SwiftUI
import QuickLookThumbnailing

enum ThumbnailState: Equatable {
    case loadingIcon
    case thumbnail
}

struct MetadataPanelView: View {
    let result: SearchResult
    var onOpen: (() -> Void)? = nil
    var onReveal: (() -> Void)? = nil
    @State var thumbnailState: ThumbnailState = .loadingIcon
    @State private var thumbnailImage: NSImage?
    @State private var details = FileDetails()
    
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // QuickLook Thumbnail
            ZStack {
                if thumbnailState == .loadingIcon {
                    Image(systemName: iconName(for: result.fileKind))
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(width: 64, height: 64)
                        .foregroundColor(.secondary)
                } else if let image = thumbnailImage {
                    Image(nsImage: image)
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(maxWidth: 240, maxHeight: 180)
                } else {
                    Image(systemName: iconName(for: result.fileKind))
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(width: 80, height: 80)
                        .foregroundColor(.secondary)
                }
            }
            .frame(maxWidth: .infinity)
            .frame(height: 180)
            
            // Header Info
            VStack(alignment: .leading, spacing: 4) {
                Text(result.filename)
                    .font(.system(size: DS.TextSize.lg, weight: .semibold))
                    .foregroundColor(DS.Palette.text)
                    .lineLimit(3)
                    .textSelection(.enabled)
                
                Text(displayPath)
                    .font(.system(size: DS.TextSize.xs, design: .monospaced))
                    .foregroundColor(DS.Palette.textMuted)
                    .lineLimit(3)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            
            // Metadata Grid
            VStack(alignment: .leading, spacing: 6) {
                MetadataDetailRow(label: "Kind", value: kindString(for: result.fileKind))
                if result.fileKind != .folder {
                    MetadataDetailRow(label: "Size", value: details.size)
                }
                MetadataDetailRow(label: "Modified", value: details.modified)
                MetadataDetailRow(label: "Created", value: details.created)
            }
            .padding(.vertical, 4)
            
            Spacer()
            
            SwiftUI.Divider()
            
            // Actions
            HStack(spacing: 8) {
                // One primary action; revealing is the quieter secondary
                Button(action: {
                    if let onOpen { onOpen() } else { NSWorkspace.shared.open(URL(fileURLWithPath: result.path)) }
                }) {
                    Text("Open")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(details.isMissing)
                
                Button(action: {
                    if let onReveal { onReveal() } else { NSWorkspace.shared.selectFile(result.path, inFileViewerRootedAtPath: "") }
                }) {
                    Image(systemName: "folder")
                        .frame(width: 16)
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
                .disabled(details.isMissing)
                .help("Reveal in Finder (⌘↩)")
                .accessibilityLabel("Reveal in Finder")
            }
            .padding(.bottom, 8)
        }
        .frame(width: 260)
        .padding()
        .task(id: result.id) {
            thumbnailState = .loadingIcon
            thumbnailImage = nil
            details = FileDetails(path: result.path)
            await loadThumbnail()
        }
    }
    
    private func iconName(for kind: FileKind) -> String {
        switch kind {
        case .document: return "doc.text.fill"
        case .image: return "photo.fill"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .folder: return "folder.fill"
        }
    }
    
    private func kindString(for kind: FileKind) -> String {
        switch kind {
        case .document: return "Document"
        case .image: return "Image"
        case .code: return "Source Code"
        case .folder: return "Folder"
        }
    }
    
    var displayPath: String {
        var path = result.path
        let homeDir = FileManager.default.homeDirectoryForCurrentUser.path
        if path.hasPrefix(homeDir) {
            path = path.replacingOccurrences(of: homeDir, with: "~")
        }
        return path
    }
    
    private func loadThumbnail() async {
        let url = URL(fileURLWithPath: result.path)
        let size = CGSize(width: 240, height: 180)
        
        do {
            let request = QLThumbnailGenerator.Request(fileAt: url, size: size, scale: NSScreen.main?.backingScaleFactor ?? 2.0, representationTypes: .thumbnail)
            let generator = QLThumbnailGenerator.shared
            let thumbnail = try await generator.generateBestRepresentation(for: request)
            
            await MainActor.run {
                self.thumbnailImage = thumbnail.nsImage
                self.thumbnailState = .thumbnail
            }
        } catch {
            await MainActor.run {
                self.thumbnailState = .thumbnail // fail silently to icon fallback
            }
        }
    }
}

/// Size and dates read from the filesystem when a result is expanded
struct FileDetails: Equatable {
    var size = "—"
    var modified = "—"
    var created = "—"
    /// The file has gone since it was indexed
    var isMissing = false
    
    init() {}
    
    init(path: String) {
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: path) else {
            size = "File no longer exists"
            isMissing = true
            return
        }
        if let bytes = attributes[.size] as? Int64 {
            size = ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
        }
        if let date = attributes[.modificationDate] as? Date {
            modified = Self.format(date)
        }
        if let date = attributes[.creationDate] as? Date {
            created = Self.format(date)
        }
    }
    
    private static func format(_ date: Date) -> String {
        date.formatted(date: .abbreviated, time: .shortened)
    }
}

private struct MetadataDetailRow: View {
    let label: String
    let value: String
    
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text(label)
                .font(.system(size: DS.TextSize.xs))
                .foregroundColor(DS.Palette.textMuted)
                .frame(width: 60, alignment: .leading)
            
            Text(value)
                .font(.system(size: DS.TextSize.xs).monospacedDigit())
                .foregroundColor(DS.Palette.text)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityElement(children: .combine)
    }
}
