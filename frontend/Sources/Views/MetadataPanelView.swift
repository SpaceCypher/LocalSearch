import SwiftUI
import QuickLookThumbnailing

enum ThumbnailState: Equatable {
    case loadingIcon
    case thumbnail
}

struct MetadataPanelView: View {
    let result: SearchResult
    @State var thumbnailState: ThumbnailState = .loadingIcon
    @State private var thumbnailImage: NSImage?
    
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
                    .font(.system(size: 16, weight: .bold))
                    .lineLimit(3)
                
                Text(displayPath)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundColor(.secondary)
                    .lineLimit(2)
            }
            
            // Metadata Grid
            VStack(alignment: .leading, spacing: 6) {
                MetadataDetailRow(label: "Kind", value: kindString(for: result.fileKind))
                MetadataDetailRow(label: "Size", value: "-- MB")
                MetadataDetailRow(label: "Modified", value: "Recently")
                MetadataDetailRow(label: "Created", value: "Recently")
            }
            .padding(.vertical, 4)
            
            Spacer()
            
            SwiftUI.Divider()
            
            // Actions
            HStack(spacing: 8) {
                Button(action: {
                    NSWorkspace.shared.open(URL(fileURLWithPath: result.path))
                }) {
                    Text("Open")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                
                Button(action: {
                    NSWorkspace.shared.selectFile(result.path, inFileViewerRootedAtPath: "")
                }) {
                    Image(systemName: "folder")
                        .frame(width: 16)
                }
                .buttonStyle(.bordered)
                .controlSize(.large)
            }
            .padding(.bottom, 8)
        }
        .frame(width: 260)
        .padding()
        .task(id: result.id) {
            thumbnailState = .loadingIcon
            thumbnailImage = nil
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

private struct MetadataDetailRow: View {
    let label: String
    let value: String
    
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Text(label)
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .frame(width: 60, alignment: .leading)
            
            Text(value)
                .font(.system(size: 11))
                .foregroundColor(.primary)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
