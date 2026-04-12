import SwiftUI

struct StatusBarView: View {
    @ObservedObject var viewModel: SearchViewModel
    @ObservedObject var settings = SettingsManager.shared
    
    var body: some View {
        HStack(spacing: 8) {
            if viewModel.queryState == .searching || viewModel.queryState == .typing {
                Circle()
                    .fill(settings.accentColor.color)
                    .frame(width: 4, height: 4)
                    .opacity(0.8)
            }
            
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
