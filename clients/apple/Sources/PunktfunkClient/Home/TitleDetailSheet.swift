// A title's details (design/apple-touch-ui-overhaul.md §2.5): cover, what it is, what the host
// recorded about playing it, and its acts: Play / Resume, a favorite heart, Copy Link. Opened
// from the title menu's Details…; a tap on the poster still just plays.

import PunktfunkKit
import SwiftUI

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

    #if os(tvOS)
    // 10-foot sizes. Back closes the sheet, so it carries no Done.
    private let posterWidth: CGFloat = 300
    private let titleSize: CGFloat = 40
    private let factSize: CGFloat = 28
    private let margin: CGFloat = 60
    private let actionSpacing: CGFloat = 24
    #else
    private let posterWidth: CGFloat = 112
    private let titleSize: CGFloat = 20
    private let factSize: CGFloat = 13
    private let margin: CGFloat = 20
    private let actionSpacing: CGFloat = 10
    #endif

    var body: some View {
        NavigationStack {
            ScrollView {
                layout
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(margin)
            }
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            #if !os(tvOS)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            #endif
        }
    }

    /// A TV sets the facts and the acts beside the poster, as its own detail pages do; elsewhere
    /// they stack under it.
    @ViewBuilder private var layout: some View {
        #if os(tvOS)
        HStack(alignment: .top, spacing: 48) {
            poster
            VStack(alignment: .leading, spacing: 24) {
                info
                stats
                actions
            }
        }
        #else
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 16) {
                poster
                info
            }
            stats
            actions
        }
        #endif
    }

    private var poster: some View {
        PosterImage(
            candidates: game.art.posterCandidates, title: game.title, loader: artLoader,
            icon: game.iconToken)
            .aspectRatio(2.0 / 3.0, contentMode: .fit)
            .frame(width: posterWidth)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private var info: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(game.title)
                .font(.geist(titleSize, .semibold, relativeTo: .title3))
                .fixedSize(horizontal: false, vertical: true)
            ForEach(facts, id: \.self) { line in
                Text(line)
                    .font(.geist(factSize, relativeTo: .subheadline))
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder private var stats: some View {
        if let stats = PlayStatsText.summary(game.stats) {
            Label(stats, systemImage: "clock")
                .font(.geist(factSize, relativeTo: .subheadline))
                .foregroundStyle(.secondary)
        }
    }

    /// Store · platform, then the credits, then the genres: a line only when it has something.
    private var facts: [String] {
        let origin = [Optional(game.storeLabel), game.platform]
        let credits = [
            game.developer, game.publisher == game.developer ? nil : game.publisher,
            game.releaseYear.map { String($0) },
        ]
        return [
            origin.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " \u{b7} "),
            credits.compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " \u{b7} "),
            (game.genres ?? []).joined(separator: ", "),
        ].filter { !$0.isEmpty }
    }

    private var actions: some View {
        HStack(spacing: actionSpacing) {
            if let onPlay {
                Button(action: onPlay) {
                    Label(playLabel, systemImage: "play.fill")
                        // A TV button keeps its own width; stretched, it read as a banner.
                        #if !os(tvOS)
                        .frame(maxWidth: .infinity)
                        #endif
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
        #if !os(tvOS)
        .controlSize(.large)
        #endif
    }
}
