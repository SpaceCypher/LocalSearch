import SwiftUI

/// Full-width index progress, used in the settings window.
struct IndexProgressView: View {
    let progress: IndexProgress
    
    var body: some View {
        VStack(spacing: DS.Space.s2) {
            HStack {
                Text(progress.phase)
                    .font(.system(size: DS.TextSize.sm))
                    .foregroundColor(DS.Palette.textMuted)
                    .lineLimit(1)
                
                Spacer()
                
                if let etaText = progress.etaText {
                    Text(etaText)
                        .font(.system(size: DS.TextSize.sm).monospacedDigit())
                        .foregroundColor(DS.Palette.textMuted)
                }
            }
            
            if progress.isDeterminate {
                ProgressView(value: progress.percent)
                    .progressViewStyle(.linear)
            } else {
                // How much work there is isn't known yet; don't invent a percentage
                ProgressView()
                    .progressViewStyle(.linear)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(progress.statusText)
    }
}

#if DEBUG
struct IndexProgressView_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 16) {
            IndexProgressView(progress: IndexProgress(
                phase: "Reading file contents",
                percent: 0.32,
                etaMinutes: 4
            ))
            
            IndexProgressView(progress: IndexProgress(
                phase: "Scanning folders · 12,480 items",
                percent: 0,
                etaMinutes: nil
            ))
        }
        .frame(width: 400)
        .padding()
    }
}
#endif
