import SwiftUI

struct ScopeBarView: View {
    @ObservedObject var viewModel: SearchViewModel
    
    var body: some View {
        HStack(spacing: 8) {
            ForEach(SearchScope.allCases, id: \.self) { scope in
                Button(action: {
                    viewModel.setActiveScope(scope)
                }) {
                    Text(scope.displayName)
                        .font(.system(size: 12))
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .background(
                            viewModel.activeScope == scope
                                ? Color.accentColor.opacity(0.2)
                                : Color.clear
                        )
                        .cornerRadius(6)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

extension SearchScope {
    var displayName: String {
        switch self {
        case .all: return "All"
        case .documents: return "Documents"
        case .images: return "Images"
        case .code: return "Code"
        case .folders: return "Folders"
        }
    }
}
