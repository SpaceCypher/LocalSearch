import SwiftUI

struct ResultListView: View {
    @ObservedObject var viewModel: SearchViewModel
    
    var body: some View {
        ScrollView {
            ScrollViewReader { proxy in
                LazyVStack(spacing: 0) {
                    ForEach(Array(viewModel.filteredResults.prefix(100).enumerated()), id: \.element.id) { index, result in
                        ResultRowView(
                            result: result, 
                            isSelected: viewModel.selectedIndex == index,
                            isTopHit: index == 0
                        )
                        .padding(.horizontal, 12)
                        .onTapGesture {
                            viewModel.selectedIndex = index
                            viewModel.syncExpandedResult()
                        }
                    }
                }
                .onChange(of: viewModel.selectedIndex) { _, newIndex in
                    if let index = newIndex, index < viewModel.filteredResults.count {
                        let targetId = viewModel.filteredResults[index].id
                        proxy.scrollTo(targetId, anchor: .none)
                    }
                }
            }
        }
        .opacity(viewModel.queryState == .typing || viewModel.queryState == .searching ? 0.4 : 1.0)
    }
}
