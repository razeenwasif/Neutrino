import AppKit
import NeutrinoKit
import Observation
import SwiftUI

enum Section: String, CaseIterable {
    case code, changes, history, branches, pulls, runs, secrets
}

/// Which secrets and variables the Secrets screen shows.
enum VaultView: Hashable {
    case repo
    case everyRepo
    case environment(String)
}

enum PullFilter: String {
    case open, closed
}

enum PullTab: String {
    case overview, files, commits
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

    // Pull requests (methods in PullsModel.swift)
    /// The hosted repo the pull requests belong to: the selected hosted repo,
    /// or the one a local repo's remote points at.
    var pullRepo: String?
    var pullRepoProblem: String?
    var pullList: [PullRequest] = []
    var pullFilter: PullFilter = .open
    var pullNumber: UInt32?
    var pullDetail: PullDetail?
    var pullError: String?
    var pullTab: PullTab = .overview
    var pullFile: String?
    var pullDiff: FileDiff?
    var pullDiffError: String?
    var pullGeneration = 0
    var pullDiffGeneration = 0
    var pullBusy: String?
    var loadingPulls = false

    // CI runs (methods in RunsModel.swift); they belong to the same hosted repo as pull requests.
    var runList: [Run] = []
    var runID: Int64?
    var runDetail: RunDetail?
    var runError: String?
    var runJobID: Int64?
    /// The selected job's log so far, and where the next read starts.
    var jobLogText = ""
    var jobLogNext: UInt64 = 0
    var jobLogDone = false
    var runGeneration = 0
    var logGeneration = 0
    var runBusy: String?
    var ciInfo: CiInfo?
    var runsTask: Task<Void, Never>?
    var artifactList: [Artifact] = []

    // Secrets (methods in SecretsModel.swift)
    var vaultInfo: VaultInfo?
    var vaultView: VaultView = .repo
    var secretList: [SecretInfo] = []
    var variableList: [Variable] = []
    var environmentList: [DeployEnvironment] = []
    var secretsError: String?

    // Menu bar panel (methods in MenuBar.swift)
    /// Status of every working copy, refreshed when the panel opens.
    var glance: [String: WorkingStatus] = [:]
    /// Open pull requests of the selected repo, for the panel.
    var glancePulls: [PullRequest] = []
    var glancePullsNote: String?

    /// The Code page's state (CodeModel.swift).
    let code = CodeModel()

    /// A short message for the last thing that went wrong, shown in the header.
    var notice: String?

    // Answers that arrive after the user has moved on are dropped.
    private var historyGeneration = 0
    private var detailGeneration = 0
    private var diffGeneration = 0
    private var watcher: Task<Void, Never>?

    var repo: Repo? {
        source == .server ? hostedRepos.first { $0.id == repoID } : repos.first { $0.id == repoID }
    }

    // The server (methods in ServerModel.swift)
    enum Source: String { case local, server }
    enum ServerState: Equatable {
        case unpaired
        case connecting
        case online(EngineInfo)
        case offline(String)
    }

    /// Whether the selected repo is on this Mac or hosted on the server.
    var source: Source = Source(rawValue: UserDefaults.standard.string(forKey: "source") ?? "") ?? .local {
        didSet { UserDefaults.standard.set(source.rawValue, forKey: "source") }
    }
    var serverLink: ServerLink? = ServerStore.load()
    var serverService: ServerService?
    var serverState: ServerState = .unpaired
    var hostedRepos: [Repo] = []
    var serverEventsTask: Task<Void, Never>?
    var lastServerCheck = Date.distantPast
    /// Fetch, push or publish running.
    var syncing: String?

    /// Who answers for the selected repo: neutrinod here, or the server.
    var activeService: any NeutrinoService {
        if source == .server, let s = serverService { return s }
        return service
    }
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
        await connectServer()
        await loadRepos()
    }

    private func checkConnection() async {
        switch connection {
        case .live:
            if (try? await service.ping()) == nil { await connect() }
            await checkServer()
        case .demo, .connecting:
            if (try? await SocketService().ping()) != nil { await connect() }
            await checkServer()
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
        // A hosted repo picked last time comes back if the server still has it.
        if source == .server, repoID == nil, let saved, hostedRepos.contains(where: { $0.id == saved }) { repoID = saved }
        if source == .server, hostedRepos.isEmpty { source = .local }
        if repo == nil || repo?.error != nil {
            source = .local
            repoID = repos.first(where: { $0.id == saved && $0.error == nil })?.id ?? repos.first(where: { $0.error == nil })?.id
            rev = nil
        }
        // No working copy here (a Mac that only reads the server): show a hosted repo
        // rather than an empty window.
        if repoID == nil, let hosted = hostedRepos.first(where: { $0.id == saved }) ?? hostedRepos.first(where: { $0.head?.sha != nil }) ?? hostedRepos.first {
            source = .server
            repoID = hosted.id
            if section == .changes { section = .history }
        }
        await reload()
    }

    func select(repo id: String) async {
        guard id != repoID || source != .local else { return }
        source = .local
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
        async let p: Void = section == .pulls ? loadPulls() : ()
        async let r: Void = section == .runs ? loadRuns() : ()
        async let v: Void = section == .secrets ? loadSecrets() : ()
        _ = await (h, b, s, p, r, v)
        if source == .local, let repoID, let updated = try? await service.repos().first(where: { $0.id == repoID }), let i = repos.firstIndex(where: { $0.id == repoID }) {
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
            let page = try await activeService.log(q)
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
            let page = try await activeService.log(q)
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
            let d = try await activeService.commit(repo: repoID, sha: sha)
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
            let d = try await activeService.fileDiff(repo: repoID, sha: sha, path: path, context: 3)
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
            branches = try await activeService.branches(repo: repoID)
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

    /// The registered repos again, keeping the selection.
    func refreshRepoList() async {
        if let list = try? await service.repos() { repos = list }
    }

    func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    func reveal(_ repo: Repo) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: repo.path)])
    }
}
