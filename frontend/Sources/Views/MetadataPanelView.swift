import SwiftUI
import QuickLook

enum ThumbnailState: Equatable {
    case loadingIcon
    case thumbnail
}

enum QuickAction: Equatable {
    case open
    case revealInFinder
    case copyPath
}

struct MetadataPanelView: View {
    let result: SearchResult
    @State var thumbnailState: ThumbnailState = .loadingIcon
    @State private var thumbnailImage: NSImage?
    
    var quickActions: [QuickAction] { [.open, .revealInFinder, .copyPath] }
    
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Thumbnail section
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
                }
            }
            .frame(maxWidth: .infinity)
            .frame(height: 180)
            
            // Quick actions
            QuickActionBarView(actions: quickActions, result: result)
            
            Spacer()
        }
        .frame(width: 260)
        .padding()
        .task {
            await loadThumbnail()
        }
    }
    
    private func iconName(for kind: FileKind) -> String {
        switch kind {
        case .document: return "doc.fill"
        case .image: return "photo.fill"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .folder: return "folder.fill"
        }
    }
    
    private func loadThumbnail() async {
        // Simulate QuickLook thumbnail generation
        try? await Task.sleep(nanoseconds: 200_000_000) // 200ms
        
        // In production, would use QLThumbnailGenerator
        // For now, just transition to thumbnail state
        thumbnailState = .thumbnail
        // Create a placeholder image
        thumbnailImage = NSImage(size: NSSize(width: 240, height: 180))
    }
}

struct QuickActionBarView: View {
    let actions: [QuickAction]
    let result: SearchResult
    
    var body: some View {
        HStack(spacing: 8) {
            ForEach(actions, id: \.self) { action in
                Button(action: { performAction(action) }) {
                    HStack(spacing: 4) {
                        Image(systemName: iconName(for: action))
                        Text(title(for: action))
                            .font(.caption)
                    }
                }
                .buttonStyle(.bordered)
            }
        }
    }
    
    private func iconName(for action: QuickAction) -> String {
        switch action {
        case .open: return "arrow.up.forward.app"
        case .revealInFinder: return "folder"
        case .copyPath: return "doc.on.clipboard"
        }
    }
    
    private func title(for action: QuickAction) -> String {
        switch action {
        case .open: return "Open"
        case .revealInFinder: return "Reveal"
        case .copyPath: return "Copy"
        }
    }
    
    private func performAction(_ action: QuickAction) {
        // Action implementation would go here
    }
}
