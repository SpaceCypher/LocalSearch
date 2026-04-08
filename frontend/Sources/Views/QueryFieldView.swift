import SwiftUI

struct QueryFieldView: View {
    @ObservedObject var viewModel: SearchViewModel
    @FocusState private var isFocused: Bool
    
    var body: some View {
        HStack(spacing: 8) {
            // Search icon
            Image(systemName: "magnifyingglass")
                .foregroundColor(.secondary)
                .frame(width: 16, height: 16)
            
            // Text field
            TextField("Search", text: $viewModel.queryText)
                .textFieldStyle(.plain)
                .font(.system(size: 14))
                .focused($isFocused)
                .onChange(of: viewModel.queryText) { newValue in
                    viewModel.onQueryChange(newValue)
                }
                .accessibilityLabel(viewModel.searchFieldAccessibilityLabel)
                .accessibilityHint(viewModel.searchFieldAccessibilityHint)
            
            // Spinner (reserved space, shown after 80ms debounce)
            if viewModel.showSpinner {
                ProgressView()
                    .controlSize(.small)
                    .frame(width: 12, height: 12)
            } else {
                // Reserved space for spinner to prevent layout shift
                Color.clear
                    .frame(width: 12, height: 12)
            }
            
            // Clear button
            if viewModel.showClearButton {
                Button(action: {
                    viewModel.clearQuery()
                    isFocused = true
                }) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                        .frame(width: 16, height: 16)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color(NSColor.controlBackgroundColor))
        .cornerRadius(6)
        .onAppear {
            isFocused = true
        }
    }
}
