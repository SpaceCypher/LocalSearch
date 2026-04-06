import SwiftUI

@MainActor
class SearchViewModel: ObservableObject {
    // Stub implementation for Task F1
    @Published var queryText: String = ""
    @Published var displayResults: [SearchResult] = []
    
    init() {
        // Stub
    }
}

// Placeholder result type
struct SearchResult: Identifiable {
    let id: String
    let filename: String
    let path: String
    let rank: Float
}
