import SwiftUI

struct StatusBarView: View {
    @ObservedObject var viewModel: SearchViewModel
    
    var body: some View {
        HStack {
            Text(viewModel.statusText)
                .font(.system(size: 11))
                .foregroundColor(.secondary)
            
            Spacer()
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(height: 24)
    }
}
