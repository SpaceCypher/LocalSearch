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
                        .padding(.horizontal, DS.Space.s3)
                        .contentShape(Rectangle())
                        // Click selects, double-click opens (as in Finder)
                        .onTapGesture(count: 2) {
                            viewModel.open(result)
                        }
                        .onTapGesture {
                            viewModel.selectedIndex = index
                            viewModel.syncExpandedResult()
                        }
                        .accessibilityAddTraits(viewModel.selectedIndex == index ? [.isSelected] : [])
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
        // Results from the previous keystroke stay fully legible while the
        // next search runs; the status bar shows that one is in flight.
    }
}
