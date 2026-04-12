import SwiftUI

struct ZeroResultsView: View {
    let query: String
    let suggestions: [String]
    let showBroadeningTip: Bool
    let note: String
    let onSelectSuggestion: (String) -> Void
    
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        VStack(spacing: 12) {
            Text("No results for \"\(query)\"")
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            
            if !suggestions.isEmpty {
                HStack(spacing: 8) {
                    Text("Did you mean:")
                        .font(.system(size: 12))
                        .foregroundColor(.secondary)
                    
                    ForEach(suggestions, id: \.self) { suggestion in
                        Button(action: {
                            onSelectSuggestion(suggestion)
                        }) {
                            Text(suggestion)
                                .font(.system(size: 12))
                                .foregroundColor(settings.accentColor.color)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            
            if showBroadeningTip {
                Text("Try removing filters to see more results")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            }
            
            if !note.isEmpty {
                Text(note)
                    .font(.system(size: 11))
                    .foregroundColor(.orange)
            }
        }
        .padding()
    }
}
