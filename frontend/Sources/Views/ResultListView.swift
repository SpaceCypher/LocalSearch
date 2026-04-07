import SwiftUI

struct ResultListView: View {
    @ObservedObject var viewModel: SearchViewModel
    
    var body: some View {
        ScrollView {
            LazyVStack(spacing: 0) {
                ForEach(Array(viewModel.filteredResults.prefix(20).enumerated()), id: \.element.id) { index, result in
                    ResultRowView(result: result, isSelected: viewModel.selectedIndex == index)
                        .onTapGesture {
                            viewModel.selectedIndex = index
                        }
                }
            }
        }
    }
}
