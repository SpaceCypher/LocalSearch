import SwiftUI

struct WelcomeView: View {
    let onContinue: () -> Void

    var body: some View {
        ZStack {
            LinearGradient(
                colors: [
                    Color.black.opacity(0.96),
                    Color(red: 0.10, green: 0.12, blue: 0.16).opacity(0.94),
                    Color.black.opacity(0.98)
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            VStack(alignment: .leading, spacing: 20) {
                header

                infoCard(
                    title: "What LocalSearch Is",
                    body: "A fast local file search app for macOS with keyboard-first workflow, fuzzy matching, and live index updates."
                )

                infoCard(
                    title: "How It Works",
                    body: "LocalSearch builds a local index on your machine and queries it through the native Rust backend. No cloud search is required for core usage."
                )

                infoCard(
                    title: "Quick Start",
                    body: "1. Press Option + Space\n2. Type your query\n3. Use Arrow keys + Return to open results\n4. Command + Return reveals in Finder"
                )

                HStack {
                    Text("You can change appearance, shortcuts, and behavior anytime in Settings.")
                        .font(.system(size: 12))
                        .foregroundColor(Color.white.opacity(0.62))

                    Spacer()

                    Button(action: onContinue) {
                        Text("Get Started")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundColor(.white)
                            .padding(.horizontal, 20)
                            .padding(.vertical, 10)
                            .background(Color.accentColor.opacity(0.95))
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                    }
                    .buttonStyle(.plain)
                }
                .padding(.top, 4)
            }
            .padding(24)
        }
        .frame(width: 680, height: 500)
        .preferredColorScheme(.dark)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Welcome to LocalSearch")
                .font(.system(size: 30, weight: .bold))
                .foregroundColor(.white)

            Text("Search your files instantly from anywhere")
                .font(.system(size: 14))
                .foregroundColor(Color.white.opacity(0.72))
        }
        .padding(.bottom, 2)
    }

    private func infoCard(title: String, body: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.white)

            Text(body)
                .font(.system(size: 13))
                .foregroundColor(Color.white.opacity(0.78))
                .lineSpacing(2)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.06))
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color.white.opacity(0.10), lineWidth: 0.5)
        )
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}
