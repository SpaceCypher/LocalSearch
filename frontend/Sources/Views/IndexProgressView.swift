import SwiftUI

struct IndexProgressView: View {
    let progress: IndexProgress
    
    var body: some View {
        VStack(spacing: 8) {
            HStack {
                Text(progress.phase)
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
                
                Spacer()
                
                Text(etaText)
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
            }
            
            ProgressView(value: progress.percent)
                .progressViewStyle(.linear)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .frame(height: 28)
    }
    
    private var etaText: String {
        let minutes = progress.etaMinutes
        if minutes == 1 {
            return "Est. 1 min"
        } else {
            return "Est. \(minutes) min"
        }
    }
}

#if DEBUG
struct IndexProgressView_Previews: PreviewProvider {
    static var previews: some View {
        VStack {
            IndexProgressView(progress: IndexProgress(
                phase: "Indexing Documents",
                percent: 0.32,
                etaMinutes: 4
            ))
            
            IndexProgressView(progress: IndexProgress(
                phase: "Processing Images",
                percent: 0.75,
                etaMinutes: 1
            ))
        }
        .frame(width: 600)
        .padding()
    }
}
#endif
