//
//  LaunchHoldView.swift
//  Punktfunk
//
//  The launch hold: the launched title's poster over the live stream until its game is up.
//

import PunktfunkKit
import SwiftUI

/// Opaque, so the launcher and desktop behind never show — every launch used to open on them.
/// A tap (the button, on tvOS) shows the stream anyway; `SessionModel.launchHold` decides the rest.
struct LaunchHoldView: View {
    let entry: GameEntry
    let host: StoredHost?
    let onShow: () -> Void
    @State private var loader: LibraryArtLoader?

    /// `Steam · PC` — the store, and the platform when the host filed one.
    private var detail: String {
        [entry.storeLabel, entry.platform].compactMap { $0 }.joined(separator: " \u{b7} ")
    }

    var body: some View {
        GeometryReader { geo in
            let posterHeight = min(geo.size.height * 0.40, 300)
            let poster = CGSize(width: posterHeight * 2 / 3, height: posterHeight)
            ZStack {
                LinearGradient(
                    colors: [Color(white: 0.11), Color(white: 0.03)],
                    startPoint: .top, endPoint: .bottom)
                VStack(spacing: 12) {
                    PosterImage(
                        candidates: entry.art.posterCandidates, title: entry.title,
                        loader: loader, icon: entry.iconToken, drawnSize: poster)
                        .frame(width: poster.width, height: poster.height)
                        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .shadow(color: .black.opacity(0.6), radius: 24, y: 12)
                        .padding(.bottom, 18)
                    Text(entry.title)
                        .font(.geist(24, .semibold, relativeTo: .title2))
                        .foregroundStyle(.white)
                        .multilineTextAlignment(.center)
                    if !detail.isEmpty {
                        Text(detail)
                            .font(.geist(15, .regular, relativeTo: .callout))
                            .foregroundStyle(.white.opacity(0.6))
                    }
                    ProgressView().tint(.white).padding(.top, 10)
                    Button("Show stream", action: onShow)
                        .buttonStyle(.bordered)
                        .padding(.top, 14)
                }
                .padding(.horizontal, 32)
            }
            .frame(width: geo.size.width, height: geo.size.height)
        }
        .environment(\.colorScheme, .dark)
        .ignoresSafeArea()
        #if !os(tvOS)
        .contentShape(Rectangle())
        .onTapGesture(perform: onShow)
        #endif
        .task {
            // The shelf's own loader: host-origin art over the paired identity, CDNs plain.
            guard let host, let identity = (try? ClientIdentityStore.shared.load())?.identity
            else { return }
            loader = try? LibraryArtLoader(
                address: host.address, port: host.effectiveMgmtPort,
                certPEM: identity.certPEM, keyPEM: identity.keyPEM,
                hostFingerprint: host.pinnedSHA256)
        }
    }
}
