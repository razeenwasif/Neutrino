import AppKit
import NeutrinoKit
import Observation
import SwiftUI

enum Section: String, CaseIterable {
    case changes, history, branches
}

/// A file in the Changes list, and which side of it is shown.
struct ChangeRef: Hashable {
    var path: String
    var area: Area
}

enum HistoryScope: String {
    /// HEAD, or the branch picked in Branches.
    case current
    /// Every branch and tag.
    case all
}

enum DiffLayout: String {
    case unified, split
}

/// Everything the window shows, loaded from neutrinod (or demo data when it isn't running).
@MainActor @Observable
final class AppModel {
    enum Connection: Equatable {
        case connecting
        case live(EngineInfo)
        case demo(reason: String)
    }

    private(set) var connection: Connection = .connecting
    private(set) var service: any NeutrinoService = DemoService()

    private(set) var repos: [Repo] = []
    var repoID: String? {
        didSet { UserDefaults.standard.set(repoID, forKey: "repo") }
    }

    var section: Section = Section(rawValue: UserDefaults.standard.string(forKey: "section") ?? "") ?? .changes {
        didSet { UserDefaults.standard.set(section.rawValue, forKey: "section") }
    }

    // Changes (the methods are in ChangesModel.swift)
    var status: WorkingStatus?
    var statusError: String?
    var change: ChangeRef?
    var workDiff: FileDiff?
    var workDiffError: String?
    /// Lines picked in the shown diff.
    var picks: Set<LinePick> = []
    var commitSummary = ""
    var commitBody = ""
    var amend = false
    var committing = false
    var lastCommit: (sha: String, output: String)?
    /// A write is running; buttons that write are disabled meanwhile.
    var writing = false
    var statusGeneration = 0
    var workDiffGeneration = 0
    var eventsTask: Task<Void, Never>?
    var refreshTask: Task<Void, Never>?

    // History
    var scope: HistoryScope = .current
    /// A branch to show instead of HEAD; set from Branches.
    var rev: String?
    var query = ""
    private(set) var commits: [LogEntry] = []
    private(set) var lanes: UInt16 = 1
    private(set) var hasMore = false
    private(set) var loadingHistory = false
    private(set) var historyError: String?
    private(set) var historyMillis: Int?

    var selectedSHA: String?
    private(set) var detail: CommitDetail?
    private(set) var detailError: String?
    var selectedPath: String?
    private(set) var diff: FileDiff?
    private(set) var diffError: String?
    private(set) var loadingDiff = false
    var layout: DiffLayout = DiffLayout(rawValue: UserDefaults.standard.string(forKey: "layout") ?? "") ?? .unified {
        didSet { UserDefaults.standard.set(layout.rawValue, forKey: "layout") }
    }
    /// The diff takes the whole width, hiding the commit list.
    var wideDiff = false

    // Branches
    private(set) var branches: [Branch] = []
    private(set) var branchesError: String?
    var selectedBranch: String?

    /// A short message for the last thing that went wrong, shown in the header.
    var notice: String?

    // Answers that arrive after the user has moved on are dropped.
    private var historyGeneration = 0
    private var detailGeneration = 0
    private var diffGeneration = 0
    private var watcher: Task<Void, Never>?

    var repo: Repo? { repos.first { $0.id == repoID } }
    var isLive: Bool { if case .live = connection { true } else { false } }

    var historyTitle: String {
        if scope == .all { return "All branches" }
        return rev ?? repo?.head?.label ?? "HEAD"
    }

    // MARK: Connection

    func start() {
        guard watcher == nil else { return }
        watcher = Task { [weak self] in
            await self?.connect()
            // Notice the daemon starting or stopping without a restart of the app.
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(4))
                await self?.checkConnection()
            }
        }
    }

    private func connect() async {
        let socket = SocketService()
        do {
            let info = try await socket.engineInfo()
            service = socket
            connection = .live(info)
        } catch {
            service = DemoService()
            connection = .demo(reason: error.localizedDescription)
        }
        listen()
        await loadRepos()
    }

    private func checkConnection() async {
        switch connection {
        case .live:
            if (try? await service.ping()) == nil { await connect() }
        case .demo, .connecting:
            if (try? await SocketService().ping()) != nil { await connect() }
        }
    }

    // MARK: Repositories

    func loadRepos() async {
        do {
            repos = try await service.repos()
        } catch {
            repos = []
            notice = error.localizedDescription
        }
        let saved = UserDefaults.standard.string(forKey: "repo")
        if repo == nil || repo?.error != nil {
            repoID = repos.first(where: { $0.id == saved && $0.error == nil })?.id ?? repos.first(where: { $0.error == nil })?.id
            rev = nil
        }
        await reload()
    }

    func select(repo id: String) async {
        guard id != repoID else { return }
        repoID = id
        rev = nil
        selectedSHA = nil
        selectedBranch = nil
        change = nil
        picks = []
        lastCommit = nil
        await reload()
    }

    func addRepository() async {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Add"
        panel.message = "Choose a folder inside a git repository."
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let added = try await service.addRepo(path: url.path)
            repos = try await service.repos()
            await select(repo: added.id)
        } catch {
            notice = error.localizedDescription
        }
    }

    func remove(repo id: String) async {
        do {
            try await service.removeRepo(id: id)
            if repoID == id { repoID = nil }
            await loadRepos()
        } catch {
            notice = error.localizedDescription
        }
    }

    /// Reload what's on screen, keeping the selection where it still exists.
    func reload() async {
        async let h: Void = reloadHistory()
        async let b: Void = loadBranches()
        async let s: Void = loadStatus()
        _ = await (h, b, s)
        if let repoID, let updated = try? await service.repos().first(where: { $0.id == repoID }), let i = repos.firstIndex(where: { $0.id == repoID }) {
            repos[i] = updated
        }
    }

    // MARK: History

    private func query(skip: Int) -> LogQuery? {
        guard let repoID else { return nil }
        let q = query.trimmingCharacters(in: .whitespaces)
        return LogQuery(repo: repoID, rev: scope == .current ? rev : nil, all: scope == .all, skip: UInt32(skip), limit: 200, query: q.isEmpty ? nil : q)
    }

    func reloadHistory() async {
        historyGeneration += 1
        let generation = historyGeneration
        guard let q = query(skip: 0) else {
            commits = []
            hasMore = false
            clearDetail()
            return
        }
        loadingHistory = true
        let started = ContinuousClock.now
        do {
            let page = try await service.log(q)
            guard generation == historyGeneration else { return }
            commits = page.commits
            lanes = page.lanes
            hasMore = page.hasMore
            historyError = nil
            historyMillis = Int((ContinuousClock.now - started) / .milliseconds(1))
        } catch {
            guard generation == historyGeneration else { return }
            commits = []
            hasMore = false
            historyError = error.localizedDescription
        }
        loadingHistory = false
        if let sha = selectedSHA, commits.contains(where: { $0.sha == sha }) {
            if detail?.sha != sha { await select(commit: sha) }
        } else if let first = commits.first {
            await select(commit: first.sha)
        } else {
            clearDetail()
        }
    }

    func loadMore() async {
        guard hasMore, !loadingHistory, let q = query(skip: commits.count) else { return }
        let generation = historyGeneration
        loadingHistory = true
        defer { loadingHistory = false }
        do {
            let page = try await service.log(q)
            guard generation == historyGeneration else { return }
            commits += page.commits
            lanes = max(lanes, page.lanes)
            hasMore = page.hasMore
        } catch {
            notice = error.localizedDescription
        }
    }

    func moveSelection(by step: Int) async {
        guard !commits.isEmpty else { return }
        let i = commits.firstIndex(where: { $0.sha == selectedSHA }) ?? -1
        let next = min(max(i + step, 0), commits.count - 1)
        await select(commit: commits[next].sha)
        if next >= commits.count - 20 { await loadMore() }
    }

    private func clearDetail() {
        selectedSHA = nil
        detail = nil
        detailError = nil
        selectedPath = nil
        diff = nil
        diffError = nil
    }

    func select(commit sha: String) async {
        guard let repoID else { return }
        selectedSHA = sha
        detailGeneration += 1
        let generation = detailGeneration
        do {
            let d = try await service.commit(repo: repoID, sha: sha)
            guard generation == detailGeneration else { return }
            detail = d
            detailError = nil
            let keep = selectedPath.flatMap { p in d.files.first(where: { $0.path == p }) }
            if let file = keep ?? d.files.first {
                await select(file: file.path)
            } else {
                selectedPath = nil
                diff = nil
            }
        } catch {
            guard generation == detailGeneration else { return }
            detail = nil
            detailError = error.localizedDescription
            diff = nil
        }
    }

    func select(file path: String) async {
        guard let repoID, let sha = detail?.sha else { return }
        selectedPath = path
        diffGeneration += 1
        let generation = diffGeneration
        loadingDiff = true
        do {
            let d = try await service.fileDiff(repo: repoID, sha: sha, path: path, context: 3)
            guard generation == diffGeneration else { return }
            diff = d
            diffError = nil
        } catch {
            guard generation == diffGeneration else { return }
            diff = nil
            diffError = error.localizedDescription
        }
        loadingDiff = false
    }

    // MARK: Branches

    func loadBranches() async {
        guard let repoID else {
            branches = []
            return
        }
        do {
            branches = try await service.branches(repo: repoID)
            branchesError = nil
            if selectedBranch == nil || !branches.contains(where: { $0.id == selectedBranch }) {
                selectedBranch = branches.first?.id
            }
        } catch {
            branches = []
            branchesError = error.localizedDescription
        }
    }

    func showHistory(of branch: Branch) async {
        rev = branch.isHead ? nil : branch.name
        scope = .current
        section = .history
        selectedSHA = nil
        await reloadHistory()
    }

    /// Swap in a fresh copy of one repo (its HEAD may have moved).
    func loadReposKeepingSelection(_ updated: Repo) async {
        if let i = repos.firstIndex(where: { $0.id == updated.id }) { repos[i] = updated }
    }

    func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    func reveal(_ repo: Repo) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: repo.path)])
    }
}
