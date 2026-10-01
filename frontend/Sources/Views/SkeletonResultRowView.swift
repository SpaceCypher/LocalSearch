import SwiftUI

struct SkeletonResultRowView: View {
    var body: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 6)
                .fill(Color.secondary.opacity(0.18))
                .frame(width: 30, height: 30)

            VStack(alignment: .leading, spacing: 8) {
                RoundedRectangle(cornerRadius: 4)
                    .fill(Color.secondary.opacity(0.2))
                    .frame(height: 12)
                    .frame(maxWidth: 260)

                RoundedRectangle(cornerRadius: 4)
                    .fill(Color.secondary.opacity(0.14))
                    .frame(height: 10)
                    .frame(maxWidth: 340)
            }

            Spacer()
        }
        .padding(.horizontal, 24)
        .frame(height: 56)
    }
}
