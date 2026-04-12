import SwiftUI

enum TahoeMode: String, CaseIterable {
    case apps = "Apps"
    case files = "Files"
    case actions = "Actions"
    case clipboard = "Clipboard"
    
    var iconName: String {
        switch self {
        case .apps: return "app.dashed"
        case .files: return "folder"
        case .actions: return "bolt.horizontal"
        case .clipboard: return "doc.on.clipboard"
        }
    }
}

struct ModeSelectorView: View {
    @State private var activeMode: TahoeMode = .files
    
    var body: some View {
        HStack(spacing: 16) {
            ForEach(TahoeMode.allCases, id: \.self) { mode in
                Button(action: {
                    withAnimation(.spring(response: 0.2, dampingFraction: 0.8)) {
                        activeMode = mode
                    }
                }) {
                    Image(systemName: mode.iconName)
                        .font(.system(size: 18, weight: .medium))
                        .frame(width: 44, height: 44)
                        .background(
                            activeMode == mode 
                                ? Color.accentColor.opacity(0.15)
                                : Color.gray.opacity(0.1)
                        )
                        .clipShape(Circle())
                        .foregroundColor(activeMode == mode ? .accentColor : .primary.opacity(0.8))
                }
                .buttonStyle(.plain)
                .help(mode.rawValue) // Tooltip on hover
            }
            Spacer()
        }
    }
}
