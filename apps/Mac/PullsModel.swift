import NeutrinoKit
import SwiftUI

/// Pull requests live on the server. A local repo's pull requests are those of
/// the hosted repo one of its remotes points at.
extension AppModel {
    var pullService: (any PullService)? { isServerOnline ? serverService : nil }

    var visiblePulls: [PullRequest] {
        pullList.filter { pullFilter == .open ? $0.state == .open : $0.state != .open }
    }

    /// Which hosted repo the selected repo's pull requests are in.
    func resolvePullRepo() async -> String? {
        guard let repoID else { return nil }
        if source == .server { return repoID }
        guard let link = serverLink else { return nil }
        let remotes = (try? await service.remotes(repo: repoID)) ?? []
        let match = hostedRepos.first { hosted in
            remotes.contains { r in
                r.url == link.pushURL(for: hosted.path) || r.url == hosted.path
                    || URL(string: r.url)?.path == hosted.path || r.url.hasSuffix(":" + hosted.path)
            }
        }
        return match?.id
    }

    func loadPulls() async {
        guard let service = pullService else {
            pullRepo = nil
            pullList = []
            pullDetail = nil
            pullRepoProblem = serverLink == nil
                ? "Pull requests live on your server. Pair this Mac with it from the sidebar."
                : "\(serverLink?.name ?? "The server") is offline."
            return
        }
        guard let hosted = await resolvePullRepo() else {
            pullRepo = nil
            pullList = []
            pullDetail = nil
            pullRepoProblem = "\(repo?.name ?? "This repository") isn't on \(serverLink?.name ?? "the server") yet. Publish it from the sidebar to open pull requests."
            return
        }
        if pullRepo != hosted {
            pullNumber = nil
            pullDetail = nil
            pullFile = nil
            pullDiff = nil
        }
        pullRepo = hosted
        pullRepoProblem = nil
        loadingPulls = true
        defer { loadingPulls = false }
        do {
            pullList = try await service.pulls(repo: hosted)
            pullError = nil
        } catch {
            pullList = []
            pullError = error.localizedDescription
            return
        }
        if let n = pullNumber, pullList.contains(where: { $0.number == n }) {
            await select(pull: n, keepFile: true)
        } else if let first = visiblePulls.first {
            await select(pull: first.number)
        } else {
            pullNumber = nil
            pullDetail = nil
        }
    }

    func select(pull number: UInt32, keepFile: Bool = false) async {
        guard let service = pullService, let pullRepo else { return }
        if pullNumber != number {
            pullFile = nil
            pullDiff = nil
            pullInterdiffFrom = nil
            pullInterdiffTo = nil
            pullInterdiff = nil
        }
        pullNumber = number
        pullGeneration += 1
        let generation = pullGeneration
        do {
            let d = try await service.pull(repo: pullRepo, number: number)
            guard generation == pullGeneration else { return }
            show(d, keepFile: keepFile)
        } catch {
            guard generation == pullGeneration else { return }
            pullDetail = nil
            pullError = error.localizedDescription
        }
    }

    func orderedFiles(for detail: PullDetail) -> [FileChange] {
        let base = pullInterdiff?.files ?? detail.files
        guard pullOrder == .tour else { return base }
        let tour = pullInterdiff?.tour ?? detail.tour
        guard !tour.isEmpty else { return base }
        let rank = Dictionary(tour.enumerated().map { ($0.element.path, $0.offset) }, uniquingKeysWith: { a, _ in a })
        return base.enumerated().sorted { a, b in
            let ra = rank[a.element.path] ?? Int.max
            let rb = rank[b.element.path] ?? Int.max
            if ra != rb { return ra < rb }
            return a.offset < b.offset
        }.map(\.element)
    }

    func tourStep(for path: String, detail: PullDetail) -> (Int, TourStep)? {
        let tour = pullInterdiff?.tour ?? detail.tour
        guard let idx = tour.firstIndex(where: { $0.path == path }) else { return nil }
        return (idx + 1, tour[idx])
    }

    /// Show a detail the server sent back, keeping the file shown when it's still there.
    private func show(_ d: PullDetail, keepFile: Bool = true) {
        pullDetail = d
        pullError = nil
        if let i = pullList.firstIndex(where: { $0.number == d.pull.number }) { pullList[i] = d.pull }
        let files = orderedFiles(for: d)
        let keep = keepFile ? pullFile.flatMap { p in files.first { $0.path == p } } : nil
        if let file = keep ?? files.first {
            Task { await select(pullFile: file.path) }
        } else {
            pullFile = nil
            pullDiff = nil
        }
    }

    /// Where the latest version's files diff from: the target as it is now for
    /// an open pull request, where it left the target for a finished one.
    var pullBase: String? {
        guard let d = pullDetail else { return nil }
        return d.pull.state == .open ? d.pull.target : d.latest?.baseSha
    }

    func select(pullFile path: String) async {
        guard let service = pullService, let pullRepo else { return }
        pullFile = path
        pullDiffGeneration += 1
        let generation = pullDiffGeneration
        do {
            let d: FileDiff
            if let from = pullInterdiffFrom, let to = pullInterdiffTo, let n = pullNumber {
                d = try await service.compareVersionDiff(repo: pullRepo, number: n, fromVersion: from, toVersion: to, path: path)
            } else if let base = pullBase, let head = pullDetail?.latest?.headSha {
                d = try await service.compareDiff(repo: pullRepo, base: base, head: head, path: path)
            } else {
                return
            }
            guard generation == pullDiffGeneration else { return }
            pullDiff = d
            pullDiffError = nil
        } catch {
            guard generation == pullDiffGeneration else { return }
            pullDiff = nil
            pullDiffError = error.localizedDescription
        }
    }

    func selectInterdiff(from: UInt32?, to: UInt32?) async {
        guard let service = pullService, let pullRepo, let n = pullNumber, let detail = pullDetail else { return }
        guard let from, let to, from < to else {
            pullInterdiffFrom = nil
            pullInterdiffTo = nil
            pullInterdiff = nil
            let files = orderedFiles(for: detail)
            if let keep = pullFile.flatMap({ p in files.first { $0.path == p } }) ?? files.first {
                await select(pullFile: keep.path)
            } else {
                pullFile = nil
                pullDiff = nil
            }
            return
        }
        pullInterdiffFrom = from
        pullInterdiffTo = to
        do {
            let inter = try await service.compareVersions(repo: pullRepo, number: n, fromVersion: from, toVersion: to)
            pullInterdiff = inter
            let files = orderedFiles(for: detail)
            if let keep = pullFile.flatMap({ p in files.first { $0.path == p } }) ?? files.first {
                await select(pullFile: keep.path)
            } else {
                pullFile = nil
                pullDiff = nil
            }
        } catch {
            notice = error.localizedDescription
        }
    }

    func syncStack() async {
        guard let service = pullService, let pullRepo, let n = pullNumber, pullBusy == nil else { return }
        pullBusy = "Syncing stack"
        defer { pullBusy = nil }
        do {
            let report = try await service.syncStack(repo: pullRepo, pull: n)
            await select(pull: n, keepFile: true)
            if let list = try? await service.pulls(repo: pullRepo) { pullList = list }
            let conflicts = report.updated.filter { $0.conflict != nil }.count
            notice = conflicts > 0
                ? "Stack synced with \(conflicts) conflict\(conflicts == 1 ? "" : "s")."
                : "Restacked \(report.updated.count) stacked PR\(report.updated.count == 1 ? "" : "s")."
        } catch {
            notice = error.localizedDescription
        }
    }

    /// Run a change to the selected pull request and show what comes back.
    private func change(_ what: String, _ body: (any PullService, String, UInt32) async throws -> PullDetail) async {
        guard let service = pullService, let pullRepo, let n = pullNumber, pullBusy == nil else { return }
        pullBusy = what
        defer { pullBusy = nil }
        do {
            show(try await body(service, pullRepo, n))
        } catch {
            notice = error.localizedDescription
        }
    }

    /// Notes on a diff line of the latest version: removed lines are on the
    /// old side, everything else on the new.
    static func side(of line: DiffLine) -> (Side, UInt32)? {
        switch line.kind {
        case .removed: line.oldLine.map { (.old, $0) }
        case .added, .context: line.newLine.map { (.new, $0) }
        }
    }

    func notes(on line: DiffLine, path: String) -> [Note] {
        guard let at = Self.side(of: line), let notes = pullDetail?.notes else { return [] }
        return notes.filter { $0.path == path && $0.side == at.0 && $0.currentLine == at.1 }
    }

    func addNote(on line: DiffLine, path: String, text: String) async {
        guard let at = Self.side(of: line) else { return }
        await change("Saving") { s, repo, number in
            try await s.addNote(repo: repo, number: number, path: path, side: at.0, line: at.1, text: text)
        }
    }

    func resolve(_ note: Note, _ resolved: Bool = true) async {
        await change(resolved ? "Resolving" : "Reopening") { s, repo, number in
            try await s.resolveNote(repo: repo, number: number, note: note.id, resolved: resolved)
        }
    }

    func setPullState(_ state: PullState) async {
        await change(state == .closed ? "Closing" : "Reopening") { s, repo, number in
            try await s.updatePull(repo: repo, number: number, title: nil, body: nil, state: state)
        }
    }

    func editPull(title: String, body: String) async {
        await change("Saving") { s, repo, number in
            try await s.updatePull(repo: repo, number: number, title: title, body: body, state: nil)
        }
    }

    func merge(strategy: MergeStrategy, deleteSource: Bool) async {
        guard let service = pullService, let pullRepo, let n = pullNumber, pullBusy == nil else { return }
        pullBusy = "Merging"
        defer { pullBusy = nil }
        do {
            _ = try await service.mergePull(repo: pullRepo, number: n, strategy: strategy, message: nil, deleteSource: deleteSource)
        } catch {
            notice = error.localizedDescription
            return
        }
        await select(pull: n, keepFile: true)
        if let list = try? await service.pulls(repo: pullRepo) { pullList = list }
        if source == .server { await loadBranches() }
    }

    /// Branches of the hosted repo, to open a pull request from.
    func hostedBranches() async -> [Branch] {
        guard let server = serverService, let pullRepo else { return [] }
        return ((try? await server.branches(repo: pullRepo)) ?? []).filter { $0.kind == .local }
    }

    var hostedDefaultBranch: String? {
        hostedRepos.first { $0.id == pullRepo }?.head?.label
    }

    func createPull(title: String, body: String, source: String, target: String, parentPull: UInt32? = nil) async throws {
        guard let service = pullService, let pullRepo else { throw ServiceError.daemon("Connect to the server first.") }
        let d = try await service.createPull(repo: pullRepo, title: title, body: body, source: source, target: target, stackId: nil, parentPull: parentPull)
        pullFilter = .open
        pullList = (try? await service.pulls(repo: pullRepo)) ?? pullList
        pullNumber = d.pull.number
        pullTab = .overview
        show(d, keepFile: false)
    }

    func loadRules() async -> [BranchRule] {
        guard let service = pullService, let pullRepo else { return [] }
        return (try? await service.rules(repo: pullRepo)) ?? []
    }

    func setRule(_ rule: BranchRule) async throws -> [BranchRule] {
        guard let service = pullService, let pullRepo else { return [] }
        let list = try await service.setRule(repo: pullRepo, rule: rule)
        if let n = pullNumber { await select(pull: n, keepFile: true) }
        return list
    }

    func deleteRule(_ pattern: String) async throws -> [BranchRule] {
        guard let service = pullService, let pullRepo else { return [] }
        let list = try await service.deleteRule(repo: pullRepo, pattern: pattern)
        if let n = pullNumber { await select(pull: n, keepFile: true) }
        return list
    }
}
