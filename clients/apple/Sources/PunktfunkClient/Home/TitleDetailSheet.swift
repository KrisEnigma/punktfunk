// A title's details (design/apple-touch-ui-overhaul.md §2.5): cover, what it is, what the host
// recorded about playing it, and its acts: Play / Resume, a favorite heart, Copy Link. Opened
// from the title menu's Details…; a tap on the poster still just plays.

import PunktfunkKit
import SwiftUI
#if os(iOS) || os(macOS)

struct TitleDetailSheet: View {
    let game: GameEntry
    let artLoader: (any LibraryArtSource)?
    /// Play, Resume (the title is up on the host) or Connect (the desktop entry).
    var playLabel = "Play"
    /// nil hides the heart: favorites belong to the Library tab.
    var isFavorite: Bool?
    var onToggleFavorite: () -> Void = {}
    /// nil ⇒ browse-only, the same gate the poster tap uses.
    var onPlay: (() -> Void)?
    var onCopyLink: (() -> Void)?
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    header
                    if let about = game.description, !about.isEmpty {
                        Text(about)
                            .font(.geist(14, relativeTo: .body))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let stats = PlayStatsText.summary(game.stats) {
                        Label(stats, systemImage: "clock")
                            .font(.geist(13, relativeTo: .subheadline))
                            .foregroundStyle(.secondary)
                    }
                    actions
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(20)
            }
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 16) {
            PosterImage(
                candidates: game.art.posterCandidates, title: game.title, loader: artLoader,
                icon: game.iconToken)
                .aspectRatio(2.0 / 3.0, contentMode: .fit)
                .frame(width: 112)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(alignment: .leading, spacing: 6) {
                Text(game.title)
                    .font(.geist(20, .semibold, relativeTo: .title3))
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(facts, id: \.self) { line in
                    Text(line)
                        .font(.geist(13, relativeTo: .subheadline))
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    /// Store · platform, the credits, the genres (tags when there are none), then the player
    /// count: a line only when it has something.
    private var facts: [String] {
        let origin = [Optional(game.storeLabel), game.platform]
        let credits = [
            game.developer, game.publisher == game.developer ? nil : game.publisher,
            game.releaseYear.map { String($0) },
        ]
        let kinds = (game.genres ?? []).isEmpty ? (game.tags ?? []) : (game.genres ?? [])
        let players = game.players.flatMap { $0 > 1 ? "Up to \($0) players" : nil }
        return [
            origin.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " \u{b7} "),
            credits.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " \u{b7} "),
            kinds.joined(separator: ", "),
            players ?? "",
        ].filter { !$0.isEmpty }
    }

    private var actions: some View {
        HStack(spacing: 10) {
            if let onPlay {
                Button(action: onPlay) {
                    Label(playLabel, systemImage: "play.fill")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
            }
            if let isFavorite {
                Button(action: onToggleFavorite) {
                    Image(systemName: isFavorite ? "heart.fill" : "heart")
                }
                .buttonStyle(.bordered)
                .accessibilityLabel(isFavorite ? "Remove from Favorites" : "Add to Favorites")
            }
            if let onCopyLink {
                Button(action: onCopyLink) { Image(systemName: "link") }
                    .buttonStyle(.bordered)
                    .accessibilityLabel("Copy Link")
            }
        }
        .controlSize(.large)
    }
}
#endif
