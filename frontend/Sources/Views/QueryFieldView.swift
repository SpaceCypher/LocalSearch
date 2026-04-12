import SwiftUI

struct QueryFieldView: View {
    @ObservedObject var viewModel: SearchViewModel
    @FocusState private var isFocused: Bool
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: "magnifyingglass")
                    .resizable()
                    .aspectRatio(contentMode: .fit)
                    .frame(width: 22, height: 22)
                    .foregroundColor(settings.accentColor.color.opacity(0.8))
                
                TextField("Search", text: $viewModel.queryText)
                    .textFieldStyle(.plain)
                    .font(.system(size: 20, weight: .light))
                    .focused($isFocused)
                    .onChange(of: viewModel.queryText) { _, newValue in
                        viewModel.onQueryChange(newValue)
                    }
                    .accessibilityLabel(viewModel.searchFieldAccessibilityLabel)
                    .accessibilityHint(viewModel.searchFieldAccessibilityHint)
                
                if viewModel.showSpinner {
                    ProgressView()
                        .controlSize(.small)
                        .scaleEffect(0.8)
                        .tint(settings.accentColor.color)
                } else if !viewModel.queryText.isEmpty {
                    Button(action: { viewModel.clearQuery() }) {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundColor(.secondary.opacity(0.5))
                            .font(.system(size: 16))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 4)
            .padding(.vertical, 12)
        }
        .onAppear {
            isFocused = true
        }
    }
}
