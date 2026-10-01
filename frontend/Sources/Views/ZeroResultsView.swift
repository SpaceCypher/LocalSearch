import SwiftUI

/// Shown when a finished search has nothing to list. Says why, as far as we
/// know, and offers the one action most likely to help.
struct ZeroResultsView: View {
    let query: String
    let suggestions: [String]
    let showBroadeningTip: Bool
    let note: String
    let onSelectSuggestion: (String) -> Void
    /// The search engine is not loaded: nothing can match, whatever is typed.
    var unavailableReason: String? = nil
    /// Indexing is still running: the file may simply not be indexed yet.
    var indexingStatus: String? = nil
    var onClearFilters: (() -> Void)? = nil
    
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        VStack(spacing: DS.Space.s3) {
            if let unavailableReason {
                message(
                    icon: "exclamationmark.triangle",
                    tint: DS.Palette.warning,
                    title: "Search isn\u{2019}t available",
                    detail: unavailableReason
                )
            } else {
                message(
                    icon: "magnifyingglass",
                    tint: DS.Palette.textFaint,
                    title: "No results for \u{201C}\(query)\u{201D}",
                    detail: detail
                )
                
                if !suggestions.isEmpty {
                    HStack(spacing: DS.Space.s2) {
                        Text("Did you mean")
                            .font(.system(size: DS.TextSize.sm))
                            .foregroundColor(DS.Palette.textMuted)
                        
                        ForEach(suggestions, id: \.self) { suggestion in
                            Button(suggestion) { onSelectSuggestion(suggestion) }
                                .buttonStyle(.plain)
                                .font(.system(size: DS.TextSize.sm, weight: .medium))
                                .foregroundColor(settings.accentColor.color)
                        }
                    }
                }
                
                if showBroadeningTip, let onClearFilters {
                    Button("Clear filters", action: onClearFilters)
                        .buttonStyle(.plain)
                        .font(.system(size: DS.TextSize.sm, weight: .medium))
                        .foregroundColor(settings.accentColor.color)
                }
                
                if !note.isEmpty {
                    Text(note)
                        .font(.system(size: DS.TextSize.xs))
                        .foregroundColor(DS.Palette.warning)
                }
            }
        }
        .padding(DS.Space.s4)
        .frame(maxWidth: 420)
    }
    
    /// The most useful explanation available for the empty list
    private var detail: String? {
        if showBroadeningTip {
            return "Filters in your search are narrowing the results."
        }
        if let indexingStatus {
            return "Still indexing (\(indexingStatus)). Newer files may not appear yet."
        }
        return "Check the spelling, or add the folder in Settings \u{2192} Indexing."
    }
    
    private func message(icon: String, tint: Color, title: String, detail: String?) -> some View {
        VStack(spacing: DS.Space.s2) {
            Image(systemName: icon)
                .font(.system(size: 20))
                .foregroundColor(tint)
                .accessibilityHidden(true)
            Text(title)
                .font(.system(size: DS.TextSize.base, weight: .medium))
                .foregroundColor(DS.Palette.text)
                .lineLimit(1)
                .truncationMode(.middle)
            if let detail {
                Text(detail)
                    .font(.system(size: DS.TextSize.sm))
                    .foregroundColor(DS.Palette.textMuted)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
