import SwiftUI

struct ResultRowView: View {
    let result: SearchResult
    let isSelected: Bool
    let isTopHit: Bool
    /// The line of the file's content that matched, if it matched on content
    var snippet: AttributedString? = nil
    
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
                    .foregroundColor(isSelected ? DS.Palette.text : DS.Palette.textMuted)
                    .opacity(iconOpacity)
            }
            .frame(width: isTopHit ? (isCompact ? 36 : 48) : (isCompact ? 28 : 32),
                   height: isTopHit ? (isCompact ? 36 : 48) : (isCompact ? 28 : 32))
            
            VStack(alignment: .leading, spacing: isTopHit && snippet == nil ? 4 : 1) {
                // Filename
                Text(displayFilename)
                    .font(.system(size: isTopHit ? (isCompact ? 15 : 17) : (isCompact ? 13 : 14), 
                                weight: isTopHit ? .medium : .regular))
                    .foregroundColor(DS.Palette.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
                
                // Why it matched. Rows have a fixed height, so in compact
                // density the matching line takes the place of the path
                // (which stays available as the row's tooltip and in details).
                if let snippet, result.permissionState == .granted {
                    Text(snippet)
                        .font(.system(size: DS.TextSize.xs))
                        .foregroundColor(DS.Palette.textMuted)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                
                // Path
                if snippet == nil || !isCompact || result.permissionState == .revoked {
                Text(pathRowText)
                    .font(.system(size: isTopHit ? DS.TextSize.sm : DS.TextSize.xs))
                    .foregroundColor(result.permissionState == .revoked ? DS.Palette.warning : DS.Palette.textMuted)
                    .lineLimit(1)
                    .truncationMode(.head)
                }
            }
            
            Spacer()
        }
        .padding(.horizontal, isCompact ? 12 : 16)
        .frame(minHeight: isTopHit ? (isCompact ? 56 : 72) : (isCompact ? 44 : 52))
        // Selection is the one place the accent colour appears in the list
        .background(isSelected ? accentColor.opacity(0.22) : Color.clear)
        .cornerRadius(DS.Radius.row)
        .help(result.path)
        .accessibilityElement(children: .ignore)
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
        return displayFolder
    }
    
    /// Where the file lives. The name is already on the line above, so the
    /// row shows the containing folder, with `~` for the home directory. Long
    /// paths are shortened from the left by SwiftUI (`truncationMode(.head)`),
    /// keeping the nearest folders visible.
    var displayFolder: String {
        let folder = (result.path as NSString).deletingLastPathComponent
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        if folder == home { return "~" }
        if folder.hasPrefix(home + "/") { return "~" + folder.dropFirst(home.count) }
        return folder.isEmpty ? "/" : folder
    }
    
    var accessibilityLabel: String {
        let fileType = result.fileKind == .document ? "document" :
                      result.fileKind == .image ? "image" :
                      result.fileKind == .code ? "code file" : "folder"
        
        if result.permissionState == .revoked {
            return "\(result.filename), \(fileType), Permission denied"
        }
        
        if let snippet {
            return "\(result.filename), \(fileType), \(displayPath). Matches: \(SnippetFormatter.plain(snippet))"
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
