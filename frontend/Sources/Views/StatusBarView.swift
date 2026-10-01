import SwiftUI

/// Bottom bar of the search panel: what the search is doing on the left;
/// on the right, the keys that act on the selection, or index activity.
struct StatusBarView: View {
    @ObservedObject var viewModel: SearchViewModel
    @ObservedObject var settings = SettingsManager.shared
    
    private var isBusy: Bool {
        viewModel.queryState == .searching || viewModel.queryState == .typing
    }
    
    var body: some View {
        HStack(spacing: DS.Space.s2) {
            if viewModel.backendUnavailableReason != nil {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: DS.TextSize.xs))
                    .foregroundColor(DS.Palette.warning)
                    .accessibilityHidden(true)
            } else if isBusy {
                Circle()
                    .fill(settings.accentColor.color)
                    .frame(width: 5, height: 5)
                    .accessibilityHidden(true)
            }
            
            Text(viewModel.statusText)
                .font(.system(size: DS.TextSize.xs).monospacedDigit())
                .foregroundColor(DS.Palette.textMuted)
                .lineLimit(1)
            
            Spacer(minLength: DS.Space.s3)
            
            if !viewModel.filteredResults.isEmpty {
                HStack(spacing: DS.Space.s3) {
                    KeyHint(keys: "↩", label: "Open")
                    KeyHint(keys: "⌘↩", label: "Reveal")
                    KeyHint(keys: "⌥↩", label: "Copy path")
                    KeyHint(keys: "⇥", label: "Details")
                }
            } else if let progress = viewModel.indexProgress {
                indexActivity(progress)
            }
        }
        .padding(.horizontal, DS.Space.s3)
        .frame(height: 24)
        .accessibilityElement(children: .combine)
    }
    
    private func indexActivity(_ progress: IndexProgress) -> some View {
        HStack(spacing: DS.Space.s2) {
            if progress.isDeterminate {
                ProgressView(value: progress.percent)
                    .progressViewStyle(.linear)
                    .tint(settings.accentColor.color)
                    .frame(width: 64)
            } else {
                ProgressView()
                    .controlSize(.mini)
            }
            Text(progress.statusText)
                .font(.system(size: DS.TextSize.xs).monospacedDigit())
                .foregroundColor(DS.Palette.textFaint)
                .lineLimit(1)
        }
        .help(progress.etaText ?? "Results may be incomplete until indexing finishes")
    }
}
