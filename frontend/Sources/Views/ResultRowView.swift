import SwiftUI

struct ResultRowView: View {
    let result: SearchResult
    let isSelected: Bool
    
    var body: some View {
        HStack(spacing: 12) {
            // File icon placeholder
            Image(systemName: iconName)
                .font(.system(size: 24))
                .foregroundColor(.secondary)
                .frame(width: 32, height: 32)
                .opacity(iconOpacity)
            
            VStack(alignment: .leading, spacing: 4) {
                // Filename
                Text(displayFilename)
                    .font(.system(size: 14))
                    .lineLimit(1)
                
                // Path
                Text(pathRowText)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }
            
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .frame(height: 56)
        .background(isSelected ? Color.accentColor.opacity(0.1) : Color.clear)
        .opacity(Double(result.opacity))
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
