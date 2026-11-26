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

    public init(id: Int64, workflow: String, branch: String, state: RunState, number: UInt32) {
        self.id = id
        self.workflow = workflow
        self.branch = branch
        self.state = state
        self.number = number
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
    public let repos: [WidgetRepoItem]
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
        repos: [WidgetRepoItem] = [],
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
        self.repos = repos
        self.openPulls = openPulls
        self.recentRuns = recentRuns
    }

    public static let placeholder = NeutrinoWidgetSnapshot(
        serverName: "mini",
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
}

public enum NeutrinoWidgetStore {
    #if os(macOS)
    public static let widgetGroupID = "YOUR_TEAM_ID.group.dev.neutrino"
    #else
    public static let widgetGroupID = "group.dev.neutrino"
    #endif
    private static let filename = "widget-snapshot.json"

    public static func save(_ snapshot: NeutrinoWidgetSnapshot) {
        guard let url = snapshotURL() else { return }
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            let data = try JSONEncoder().encode(snapshot)
            try data.write(to: url, options: .atomic)
            #if canImport(WidgetKit)
            WidgetCenter.shared.reloadAllTimelines()
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
