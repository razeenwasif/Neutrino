import NeutrinoKit
import SwiftUI

/// The paired server (atlas): its hosted repos, pairing, and moving work there.
extension AppModel {
    var isServerOnline: Bool { if case .online = serverState { true } else { false } }

    func connectServer() async {
        guard let link = serverLink else {
            serverService = nil
            serverState = .unpaired
            hostedRepos = []
            return
        }
        let server = ServerService(link: link)
        serverService = server
        if serverState == .unpaired { serverState = .connecting }
        lastServerCheck = Date()
        do {
            let info = try await server.engineInfo()
            hostedRepos = try await server.repos()
            let wasOnline = isServerOnline
            serverState = .online(info)
            if !wasOnline { listenToServer() }
        } catch {
            serverState = .offline(error.localizedDescription)
            serverEventsTask?.cancel()
            serverEventsTask = nil
        }
    }

    /// Called from the connection loop: retry an offline server now and then.
    func checkServer() async {
        guard serverLink != nil else { return }
        let due = isServerOnline ? 60.0 : 15.0
        if Date().timeIntervalSince(lastServerCheck) >= due { await connectServer() }
    }

    private func listenToServer() {
        serverEventsTask?.cancel()
        guard let server = serverService else { return }
        let stream = server.events()
        serverEventsTask = Task { [weak self] in
            for await event in stream {
                guard let self else { return }
                if let list = try? await server.repos() { self.hostedRepos = list }
                // The open repo stopped being hosted (deleted elsewhere): show another.
                if event.kind == "removed", self.source == .server, event.repo == self.repoID {
                    self.recentRepos.removeAll { $0 == "server:\(event.repo)" }
                    if let first = self.hostedRepos.first { await self.selectHosted(first.id) } else { self.source = .local; self.repoID = self.repos.first?.id; await self.reload() }
                    continue
                }
                if self.source == .server, event.repo == self.repoID, event.kind != "pulls" {
                    await self.loadBranches()
                    if self.section == .history { await self.reloadHistory() }
                }
                if event.repo == self.pullRepo, self.section == .pulls {
                    await self.loadPulls()
                }
                if event.repo == self.pullRepo, self.section == .runs, event.kind == "runs" {
                    await self.refreshRuns()
                }
            }
            // The stream ended: the server went away.
            guard let self else { return }
            self.serverState = .offline("the connection to \(self.serverLink?.name ?? "the server") closed")
        }
    }

    func selectHosted(_ id: String) async {
        guard source != .server || repoID != id else { return }
        source = .server
        repoID = id
        noteRecent()
        rev = nil
        selectedSHA = nil
        selectedBranch = nil
        change = nil
        status = nil
        if section == .changes { section = .history }
        await reload()
    }

    /// Stop hosting a repo on the server (it moves to the server's deleted
    /// folder), then show something else if it was open.
    func deleteHosted(_ id: String) async throws {
        guard let server = serverService else { throw ServiceError.daemon("Connect to the server first.") }
        try await server.deleteHosted(repo: id)
        hostedRepos.removeAll { $0.id == id }
        recentRepos.removeAll { $0 == "server:\(id)" }
        if source == .server, repoID == id {
            if let first = hostedRepos.first { await selectHosted(first.id) } else { source = .local; repoID = repos.first?.id; await reload() }
        }
        hostedRepos = (try? await server.repos()) ?? hostedRepos
    }

    func paired(_ link: ServerLink) async {
        serverLink = link
        serverState = .connecting
        await connectServer()
    }

    func forgetServer() {
        ServerStore.forget()
        serverEventsTask?.cancel()
        serverEventsTask = nil
        serverLink = nil
        serverService = nil
        serverState = .unpaired
        hostedRepos = []
        if source == .server {
            source = .local
            repoID = repos.first(where: { $0.error == nil })?.id
            Task { await reload() }
        }
    }

    /// Whether the selected local repo already has a remote at the server.
    func serverRemote(in remotes: [RemoteInfo]) -> RemoteInfo? {
        guard let link = serverLink else { return nil }
        return remotes.first { $0.name == "atlas" } ?? remotes.first { r in hostedRepos.contains { link.pushURL(for: $0.path) == r.url } }
    }

    /// Whether the selected working copy already pushes to a repo on the
    /// server, so there's nothing to publish.
    func selectedIsHosted() async -> Bool {
        guard source == .local, let repoID, serverLink != nil,
              let remotes = try? await service.remotes(repo: repoID) else { return false }
        return serverRemote(in: remotes) != nil
    }

    /// The remote Publish adds: origin, as `ntr` expects, unless the working
    /// copy already has an origin elsewhere; then atlas, and origin is left alone.
    static func publishRemoteName(for remotes: [RemoteInfo]) -> String {
        remotes.contains { $0.name == "origin" } ? "atlas" : "origin"
    }

    func publishRemoteName() async -> String {
        guard let repoID, let remotes = try? await service.remotes(repo: repoID) else { return "origin" }
        return Self.publishRemoteName(for: remotes)
    }

    /// Create a hosted repo for the selected working copy, add it as a remote
    /// (see publishRemoteName) and push the current branch there. Returns
    /// git's output.
    func publish(name: String, description: String) async throws -> String {
        guard source == .local, let repoID, let server = serverService, let link = serverLink else {
            throw ServiceError.daemon("Pick one of this Mac's repositories first.")
        }
        let existing = try await service.remotes(repo: repoID)
        if serverRemote(in: existing) != nil {
            throw ServiceError.daemon("This repository is already on \(link.name).")
        }
        let remote = Self.publishRemoteName(for: existing)
        if existing.contains(where: { $0.name == remote }) {
            throw ServiceError.daemon("This repository already has a remote called \(remote).")
        }
        syncing = "Publishing"
        defer { syncing = nil }
        let hosted = try await server.createHosted(name: name, description: description.isEmpty ? nil : description)
        try await service.addRemote(repo: repoID, name: remote, url: link.pushURL(for: hosted.path))
        let output = try await service.push(repo: repoID, remote: remote, branch: nil, setUpstream: true)
        hostedRepos = (try? await server.repos()) ?? hostedRepos
        await loadStatus()
        await loadBranches()
        return output
    }

    func fetch() async {
        guard source == .local, let repoID, syncing == nil else { return }
        syncing = "Fetching"
        defer { syncing = nil }
        do {
            _ = try await service.fetch(repo: repoID, remote: nil)
        } catch {
            notice = error.localizedDescription
        }
        await loadStatus()
        await loadBranches()
        if section == .history { await reloadHistory() }
    }

    func push() async {
        guard source == .local, let repoID, syncing == nil else { return }
        syncing = "Pushing"
        defer { syncing = nil }
        let firstPush = status?.upstream == nil
        var remote: String?
        if firstPush, let remotes = try? await service.remotes(repo: repoID) {
            // A branch without an upstream goes to atlas if it's there, else origin.
            remote = serverRemote(in: remotes)?.name ?? remotes.first(where: { $0.name == "origin" })?.name ?? remotes.first?.name
            if remote == nil {
                notice = serverLink == nil ? "This repository has no remote to push to." : "This repository has no remote yet. Publish it to \(serverLink!.name) first."
                return
            }
        }
        do {
            _ = try await service.push(repo: repoID, remote: remote, branch: nil, setUpstream: firstPush)
        } catch {
            notice = error.localizedDescription
        }
        await loadStatus()
        await loadBranches()
        if section == .history { await reloadHistory() }
    }
}
