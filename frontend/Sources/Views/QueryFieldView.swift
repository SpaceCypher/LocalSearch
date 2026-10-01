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
                    .foregroundColor(DS.Palette.textMuted)
                    .accessibilityHidden(true)
                
                TextField("Search", text: $viewModel.queryText)
                    .textFieldStyle(.plain)
                    .font(.system(size: DS.TextSize.query, weight: .regular))
                    .foregroundColor(DS.Palette.text)
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
                            .foregroundColor(DS.Palette.textFaint)
                            .font(.system(size: 16))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Clear search")
                    .help("Clear search")
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
