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

            Button(action: {
                if let appDelegate = NSApp.delegate as? AppDelegate {
                    appDelegate.openSettings()
                }
            }) {
                Image(systemName: "gearshape.fill")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary.opacity(0.8))
            }
            .buttonStyle(.plain)
            .help("Open Settings")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(height: 24)
    }
}
