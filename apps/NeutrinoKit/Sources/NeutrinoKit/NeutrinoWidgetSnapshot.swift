import Foundation
#if canImport(WidgetKit)
import WidgetKit
#endif

/// Summary of a repository for Neutrino's macOS/iOS Home & Desktop widgets.
public struct WidgetRepoItem: Codable, Sendable, Identifiable, Hashable {
    public let id: String
    public let name: String
    public let branch: String
    public let dirtyCount: Int
    public let ahead: Int
    public let behind: Int
    public let isHosted: Bool

    public init(
        id: String,
        name: String,
        branch: String,
        dirtyCount: Int = 0,
        ahead: Int = 0,
        behind: Int = 0,
        isHosted: Bool = false
    ) {
        self.id = id
        self.name = name
        self.branch = branch
        self.dirtyCount = dirtyCount
        self.ahead = ahead
        self.behind = behind
        self.isHosted = isHosted
    }
}

/// Waiting deploy approval metadata for Neutrino's iOS widgets.
public struct WidgetWaitingItem: Codable, Sendable, Identifiable, Hashable {
    public let id: Int64
    public let repo: String
    public let runID: Int64
    public let number: UInt32
    public let runName: String
    public let jobName: String
    public let environment: String?
    public let branch: String?

    public init(
        id: Int64,
        repo: String,
        runID: Int64,
        number: UInt32,
        runName: String,
        jobName: String,
        environment: String? = nil,
        branch: String? = nil
    ) {
        self.id = id
        self.repo = repo
        self.runID = runID
        self.number = number
        self.runName = runName
        self.jobName = jobName
        self.environment = environment
        self.branch = branch
    }
}

/// Open pull request metadata for Neutrino's widgets.
public struct WidgetPullItem: Codable, Sendable, Identifiable, Hashable {
    public let id: String
    public let number: UInt32
    public let title: String
    public let repoName: String
    public let source: String
    public let target: String

    public init(id: String, number: UInt32, title: String, repoName: String, source: String, target: String) {
        self.id = id
        self.number = number
        self.title = title
        self.repoName = repoName
        self.source = source
        self.target = target
    }
}

/// Recent CI workflow run metadata for Neutrino's widgets.
public struct WidgetRunItem: Codable, Sendable, Identifiable, Hashable {
    public let id: Int64
    public let workflow: String
    public let branch: String
    public let state: RunState
    public let number: UInt32
    public let repoName: String?

    public init(id: Int64, workflow: String, branch: String, state: RunState, number: UInt32, repoName: String? = nil) {
        self.id = id
        self.workflow = workflow
        self.branch = branch
        self.state = state
        self.number = number
        self.repoName = repoName
    }
}

/// Zero-secret workspace & forge summary snapshot shared with Neutrino's WidgetKit extensions.
public struct NeutrinoWidgetSnapshot: Codable, Sendable, Equatable {
    public let updatedMs: UInt64
    public let serverName: String?
    public let serverOnline: Bool
    public let daemonOnline: Bool
    public let localRepoCount: Int
    public let hostedRepoCount: Int
    public let dirtyRepoCount: Int
    public let openPullCount: Int
    public let activeRepoID: String?
    public let activeRepoName: String?
    public let activeBranch: String?
    public let activeDirtyFiles: Int
    public let waitingApprovalCount: Int
    public let readyPullCount: Int
    public let failingRunCount: Int
    public let isPhoneInbox: Bool
    public let repos: [WidgetRepoItem]
    public let waitingApprovals: [WidgetWaitingItem]
    public let openPulls: [WidgetPullItem]
    public let recentRuns: [WidgetRunItem]

    public init(
        updatedMs: UInt64 = UInt64(Date().timeIntervalSince1970 * 1000),
        serverName: String? = nil,
        serverOnline: Bool = false,
        daemonOnline: Bool = true,
        localRepoCount: Int,
        hostedRepoCount: Int,
        dirtyRepoCount: Int,
        openPullCount: Int,
        activeRepoID: String? = nil,
        activeRepoName: String? = nil,
        activeBranch: String? = nil,
        activeDirtyFiles: Int = 0,
        waitingApprovalCount: Int = 0,
        readyPullCount: Int = 0,
        failingRunCount: Int = 0,
        isPhoneInbox: Bool = false,
        repos: [WidgetRepoItem] = [],
        waitingApprovals: [WidgetWaitingItem] = [],
        openPulls: [WidgetPullItem] = [],
        recentRuns: [WidgetRunItem] = []
    ) {
        self.updatedMs = updatedMs
        self.serverName = serverName
        self.serverOnline = serverOnline
        self.daemonOnline = daemonOnline
        self.localRepoCount = localRepoCount
        self.hostedRepoCount = hostedRepoCount
        self.dirtyRepoCount = dirtyRepoCount
        self.openPullCount = openPullCount
        self.activeRepoID = activeRepoID
        self.activeRepoName = activeRepoName
        self.activeBranch = activeBranch
        self.activeDirtyFiles = activeDirtyFiles
        self.waitingApprovalCount = waitingApprovalCount
        self.readyPullCount = readyPullCount
        self.failingRunCount = failingRunCount
        self.isPhoneInbox = isPhoneInbox
        self.repos = repos
        self.waitingApprovals = waitingApprovals
        self.openPulls = openPulls
        self.recentRuns = recentRuns
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        updatedMs = try c.decode(UInt64.self, forKey: .updatedMs)
        serverName = try c.decodeIfPresent(String.self, forKey: .serverName)
        serverOnline = try c.decode(Bool.self, forKey: .serverOnline)
        daemonOnline = try c.decode(Bool.self, forKey: .daemonOnline)
        localRepoCount = try c.decode(Int.self, forKey: .localRepoCount)
        hostedRepoCount = try c.decode(Int.self, forKey: .hostedRepoCount)
        dirtyRepoCount = try c.decode(Int.self, forKey: .dirtyRepoCount)
        openPullCount = try c.decode(Int.self, forKey: .openPullCount)
        activeRepoID = try c.decodeIfPresent(String.self, forKey: .activeRepoID)
        activeRepoName = try c.decodeIfPresent(String.self, forKey: .activeRepoName)
        activeBranch = try c.decodeIfPresent(String.self, forKey: .activeBranch)
        activeDirtyFiles = try c.decode(Int.self, forKey: .activeDirtyFiles)
        waitingApprovalCount = try c.decodeIfPresent(Int.self, forKey: .waitingApprovalCount) ?? 0
        readyPullCount = try c.decodeIfPresent(Int.self, forKey: .readyPullCount) ?? 0
        failingRunCount = try c.decodeIfPresent(Int.self, forKey: .failingRunCount) ?? 0
        isPhoneInbox = try c.decodeIfPresent(Bool.self, forKey: .isPhoneInbox) ?? false
        repos = try c.decodeIfPresent([WidgetRepoItem].self, forKey: .repos) ?? []
        waitingApprovals = try c.decodeIfPresent([WidgetWaitingItem].self, forKey: .waitingApprovals) ?? []
        openPulls = try c.decodeIfPresent([WidgetPullItem].self, forKey: .openPulls) ?? []
        recentRuns = try c.decodeIfPresent([WidgetRunItem].self, forKey: .recentRuns) ?? []
    }

    public static func fromInbox(_ inbox: Inbox, serverName: String?, online: Bool) -> NeutrinoWidgetSnapshot {
        var seenPulls = Set<String>()
        var mergedPulls: [PullRequest] = []
        for pr in inbox.ready + inbox.open where seenPulls.insert(pr.id).inserted {
            mergedPulls.append(pr)
        }

        let waitingItems = inbox.waiting.prefix(6).map { w in
            WidgetWaitingItem(
                id: w.job,
                repo: w.repo,
                runID: w.run,
                number: w.number,
                runName: w.runName,
                jobName: w.jobName,
                environment: w.environment,
                branch: w.branch
            )
        }
        let pullItems = mergedPulls.prefix(6).map { pr in
            WidgetPullItem(
                id: pr.id,
                number: pr.number,
                title: pr.title,
                repoName: pr.repo,
                source: pr.source,
                target: pr.target
            )
        }
        let runItems = inbox.runs.prefix(6).map { run in
            WidgetRunItem(
                id: run.id,
                workflow: run.name,
                branch: run.branch ?? run.shortSHA,
                state: run.state,
                number: run.number,
                repoName: run.repo
            )
        }
        let failingCount = inbox.runs.filter { $0.state == .failure }.count
        let activeRepo = inbox.waiting.first?.repo ?? mergedPulls.first?.repo ?? inbox.runs.first?.repo

        return NeutrinoWidgetSnapshot(
            serverName: serverName,
            serverOnline: online,
            daemonOnline: false,
            localRepoCount: 0,
            hostedRepoCount: 0,
            dirtyRepoCount: 0,
            openPullCount: mergedPulls.count,
            activeRepoID: activeRepo,
            activeRepoName: activeRepo,
            activeBranch: inbox.waiting.first?.branch ?? mergedPulls.first?.source,
            activeDirtyFiles: 0,
            waitingApprovalCount: inbox.waiting.count,
            readyPullCount: inbox.ready.count,
            failingRunCount: failingCount,
            isPhoneInbox: true,
            repos: [],
            waitingApprovals: waitingItems,
            openPulls: pullItems,
            recentRuns: runItems
        )
    }

    public static let placeholder = NeutrinoWidgetSnapshot(
        serverName: "server",
        serverOnline: true,
        daemonOnline: true,
        localRepoCount: 6,
        hostedRepoCount: 8,
        dirtyRepoCount: 2,
        openPullCount: 3,
        activeRepoID: "neutrino",
        activeRepoName: "Neutrino",
        activeBranch: "main",
        activeDirtyFiles: 4,
        repos: [
            WidgetRepoItem(id: "neutrino", name: "Neutrino", branch: "main", dirtyCount: 4, ahead: 1, behind: 0),
            WidgetRepoItem(id: "graviton", name: "Graviton", branch: "master", dirtyCount: 0, ahead: 0, behind: 0),
            WidgetRepoItem(id: "stratus", name: "Stratus", branch: "main", dirtyCount: 2, ahead: 2, behind: 0),
            WidgetRepoItem(id: "axion", name: "Axion", branch: "main", dirtyCount: 0, ahead: 0, behind: 0),
        ],
        openPulls: [
            WidgetPullItem(id: "Neutrino#42", number: 42, title: "Stratus UI overhaul & desktop widgets", repoName: "Neutrino", source: "feat/ui", target: "main"),
            WidgetPullItem(id: "Graviton#19", number: 19, title: "Snapshot-backed WidgetKit telemetry", repoName: "Graviton", source: "feat/widgets", target: "master"),
        ],
        recentRuns: [
            WidgetRunItem(id: 1, workflow: "ci-macos", branch: "main", state: .success, number: 128),
            WidgetRunItem(id: 2, workflow: "rust-tests", branch: "feat/ui", state: .running, number: 129),
        ]
    )

    public static let phonePlaceholder = NeutrinoWidgetSnapshot(
        serverName: "server",
        serverOnline: true,
        daemonOnline: false,
        localRepoCount: 0,
        hostedRepoCount: 0,
        dirtyRepoCount: 0,
        openPullCount: 2,
        activeRepoID: "Neutrino",
        activeRepoName: "Neutrino",
        activeBranch: "main",
        activeDirtyFiles: 0,
        waitingApprovalCount: 1,
        readyPullCount: 1,
        failingRunCount: 0,
        isPhoneInbox: true,
        repos: [],
        waitingApprovals: [
            WidgetWaitingItem(id: 101, repo: "Neutrino", runID: 44, number: 128, runName: "Release", jobName: "deploy-prod", environment: "production", branch: "main"),
        ],
        openPulls: [
            WidgetPullItem(id: "Neutrino#42", number: 42, title: "Stratus iOS widgets & deep links", repoName: "Neutrino", source: "feat/ios-widgets", target: "main"),
            WidgetPullItem(id: "Graviton#19", number: 19, title: "Live App Group snapshots", repoName: "Graviton", source: "feat/widgets", target: "master"),
        ],
        recentRuns: [
            WidgetRunItem(id: 1, workflow: "ci-ios", branch: "main", state: .running, number: 128, repoName: "Neutrino"),
            WidgetRunItem(id: 2, workflow: "rust-tests", branch: "master", state: .success, number: 94, repoName: "Graviton"),
        ]
    )
}

public enum NeutrinoWidgetStore {
    #if os(macOS)
    public static let widgetGroupID: String = {
        if let configured = Bundle.main.object(forInfoDictionaryKey: "NeutrinoWidgetGroupID") as? String,
           !configured.isEmpty,
           !configured.contains("$(") {
            return configured
        }
        return "group.dev.neutrino"
    }()
    #else
    public static let widgetGroupID = "group.dev.neutrino"
    #endif
    private static let filename = "widget-snapshot.json"

    public static func save(_ snapshot: NeutrinoWidgetSnapshot, reload: Bool = true) {
        guard let url = snapshotURL() else { return }
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let data = try JSONEncoder().encode(snapshot)
            try data.write(to: url, options: .atomic)
            #if canImport(WidgetKit)
            if reload {
                WidgetCenter.shared.reloadAllTimelines()
            }
            #endif
        } catch {
            // Non-fatal: widget refreshes on next app cycle.
        }
    }

    public static func load() -> NeutrinoWidgetSnapshot? {
        guard let url = snapshotURL(),
              let data = try? Data(contentsOf: url) else {
            return nil
        }
        return try? JSONDecoder().decode(NeutrinoWidgetSnapshot.self, from: data)
    }

    private static func snapshotURL() -> URL? {
        if let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: widgetGroupID) {
            return container.appendingPathComponent(filename)
        }
        #if os(macOS)
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Group Containers", isDirectory: true)
            .appendingPathComponent(widgetGroupID, isDirectory: true)
            .appendingPathComponent(filename)
        #else
        return nil
        #endif
    }
}
