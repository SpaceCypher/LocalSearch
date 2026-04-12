import SwiftUI

struct ResultRowView: View {
    let result: SearchResult
    let isSelected: Bool
    let isTopHit: Bool
    
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        let isCompact = settings.resultDensity == .compact
        let accentColor = settings.accentColor.color
        
        HStack(spacing: isCompact ? 10 : 16) {
            // File icon 
            ZStack {
                Image(systemName: iconName)
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(width: isTopHit ? (isCompact ? 28 : 36) : (isCompact ? 20 : 24), 
                           height: isTopHit ? (isCompact ? 28 : 36) : (isCompact ? 20 : 24))
                    .foregroundColor(isSelected ? .primary : .secondary)
                    .opacity(iconOpacity)
            }
            .frame(width: isTopHit ? (isCompact ? 36 : 48) : (isCompact ? 28 : 32),
                   height: isTopHit ? (isCompact ? 36 : 48) : (isCompact ? 28 : 32))
            
            VStack(alignment: .leading, spacing: isTopHit ? 4 : 2) {
                // Filename
                Text(displayFilename)
                    .font(.system(size: isTopHit ? (isCompact ? 15 : 17) : (isCompact ? 13 : 14), 
                                weight: isTopHit ? .medium : .regular))
                    .foregroundColor(.primary)
                    .lineLimit(1)
                
                // Path
                Text(pathRowText)
                    .font(.system(size: isTopHit ? (isCompact ? 11 : 12) : (isCompact ? 10 : 11)))
                    .foregroundColor(.secondary.opacity(0.8))
                    .lineLimit(1)
            }
            
            Spacer()
        }
        .padding(.horizontal, isCompact ? 12 : 16)
        .frame(minHeight: isTopHit ? (isCompact ? 56 : 72) : (isCompact ? 44 : 52))
        .background(
            ZStack {
                if isSelected {
                    accentColor.opacity(0.12)
                    LinearGradient(
                        colors: [accentColor.opacity(0.05), Color.clear],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                }
            }
        )
        .cornerRadius(isTopHit ? 12 : 8)
        .overlay(
            RoundedRectangle(cornerRadius: isTopHit ? 12 : 8)
                .stroke(isSelected ? accentColor.opacity(0.2) : Color.clear, lineWidth: 0.5)
        )
        .accessibilityLabel(accessibilityLabel)
    }
    
    var iconName: String {
        if result.permissionState == .revoked {
            return "lock.fill"
        }
        
        switch result.fileKind {
        case .document: return "doc.text"
        case .image: return "photo"
        case .code: return "chevron.left.forwardslash.chevron.right"
        case .folder: return "folder"
        }
    }
    
    var showsLockIcon: Bool {
        result.permissionState == .revoked
    }
    
    var iconOpacity: Double {
        result.permissionState == .revoked ? 0.6 : 1.0
    }
    
    var pathRowText: String {
        if result.permissionState == .revoked {
            return "Permission denied"
        }
        return displayPath
    }
    
    var accessibilityLabel: String {
        let fileType = result.fileKind == .document ? "document" :
                      result.fileKind == .image ? "image" :
                      result.fileKind == .code ? "code file" : "folder"
        
        if result.permissionState == .revoked {
            return "\(result.filename), \(fileType), Permission denied"
        }
        
        return "\(result.filename), \(fileType), \(displayPath)"
    }
    
    var displayFilename: String {
        let maxLength = 50
        if result.filename.count <= maxLength {
            return result.filename
        }
        
        // Middle truncation
        let ext = (result.filename as NSString).pathExtension
        let name = (result.filename as NSString).deletingPathExtension
        
        if name.count > maxLength - ext.count - 3 {
            let keepStart = (maxLength - ext.count - 3) / 2
            let keepEnd = keepStart
            let start = String(name.prefix(keepStart))
            let end = String(name.suffix(keepEnd))
            return "\(start)…\(end).\(ext)"
        }
        
        return result.filename
    }
    
    var displayPath: String {
        var path = result.path
        
        // Substitute home directory with ~
        let homeDir = FileManager.default.homeDirectoryForCurrentUser.path
        if path.hasPrefix(homeDir) {
            path = path.replacingOccurrences(of: homeDir, with: "~")
        }
        
        // Left truncation for long paths
        let maxLength = 60
        if path.count > maxLength {
            let components = path.split(separator: "/")
            if components.count > 3 {
                let lastThree = components.suffix(3).joined(separator: "/")
                return "…/\(lastThree)"
            }
        }
        
        return path
    }
}
