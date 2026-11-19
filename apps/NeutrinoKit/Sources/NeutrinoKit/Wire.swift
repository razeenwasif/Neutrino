import Foundation

// Mirrors crates/neutrino-proto. Keys are snake_case on the wire and
// camelCase here (the coders convert); enum values keep their wire spelling.

public let protocolVersion: UInt32 = 8

// MARK: - Repositories and refs

public enum Head: Hashable, Sendable, Codable {
    case branch(name: String, sha: String?)
    case detached(sha: String)

    public var sha: String? {
        switch self {
        case let .branch(_, sha): sha
        case let .detached(sha): sha
        }
    }

    /// The branch name, or a short sha for a detached HEAD.
    public var label: String {
        switch self {
        case let .branch(name, _): name
        case let .detached(sha): String(sha.prefix(7))
        }
    }

    private enum Keys: String, CodingKey { case kind, name, sha }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "branch": self = .branch(name: try c.decode(String.self, forKey: .name), sha: try c.decodeIfPresent(String.self, forKey: .sha))
        case "detached": self = .detached(sha: try c.decode(String.self, forKey: .sha))
        case let other: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown head kind \(other)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case let .branch(name, sha):
            try c.encode("branch", forKey: .kind)
            try c.encode(name, forKey: .name)
            try c.encode(sha, forKey: .sha)
        case let .detached(sha):
            try c.encode("detached", forKey: .kind)
            try c.encode(sha, forKey: .sha)
        }
    }
}

public struct Repo: Codable, Hashable, Identifiable, Sendable {
    public var id: String
    public var name: String
    public var path: String
    public var bare: Bool
    public var head: Head?
    public var error: String?

    public init(id: String, name: String, path: String, bare: Bool = false, head: Head?, error: String? = nil) {
        self.id = id
        self.name = name
        self.path = path
        self.bare = bare
        self.head = head
        self.error = error
    }
}

public enum RefKind: String, Codable, Sendable {
    case head, local, remote, tag
}

public struct RefLabel: Codable, Hashable, Sendable {
    public var name: String
    public var kind: RefKind
    public var isHead: Bool

    public init(name: String, kind: RefKind, isHead: Bool = false) {
        self.name = name
        self.kind = kind
        self.isHead = isHead
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        kind = try c.decode(RefKind.self, forKey: .kind)
        isHead = try c.decodeIfPresent(Bool.self, forKey: .isHead) ?? false
    }
}

public struct Branch: Codable, Hashable, Identifiable, Sendable {
    public var name: String
    public var kind: RefKind
    public var sha: String
    public var subject: String
    public var time: Int64
    public var isHead: Bool
    public var upstream: String?
    public var ahead: UInt32?
    public var behind: UInt32?

    public var id: String { "\(kind.rawValue):\(name)" }

    public init(name: String, kind: RefKind, sha: String, subject: String, time: Int64, isHead: Bool = false, upstream: String? = nil, ahead: UInt32? = nil, behind: UInt32? = nil) {
        self.name = name
        self.kind = kind
        self.sha = sha
        self.subject = subject
        self.time = time
        self.isHead = isHead
        self.upstream = upstream
        self.ahead = ahead
        self.behind = behind
    }
}

// MARK: - History

public struct Signature: Codable, Hashable, Sendable {
    public var name: String
    public var email: String
    public var time: Int64
    public var offset: Int32

    public init(name: String, email: String, time: Int64, offset: Int32 = 0) {
        self.name = name
        self.email = email
        self.time = time
        self.offset = offset
    }
}

public struct GraphSegment: Codable, Hashable, Sendable {
    public var from: UInt16
    public var to: UInt16
    public var color: UInt16

    public init(from: UInt16, to: UInt16, color: UInt16) {
        self.from = from
        self.to = to
        self.color = color
    }
}

/// One row of the commit graph; see `GraphRow` in neutrino-proto.
public struct GraphRow: Codable, Hashable, Sendable {
    public var lane: UInt16
    public var color: UInt16
    public var upper: [GraphSegment]
    public var lower: [GraphSegment]

    public init(lane: UInt16, color: UInt16, upper: [GraphSegment], lower: [GraphSegment]) {
        self.lane = lane
        self.color = color
        self.upper = upper
        self.lower = lower
    }
}

public struct LogEntry: Codable, Hashable, Identifiable, Sendable {
    public var sha: String
    public var parents: [String]
    public var subject: String
    public var author: Signature
    public var time: Int64
    public var refs: [RefLabel]
    public var graph: GraphRow?

    public var id: String { sha }
    public var shortSHA: String { String(sha.prefix(7)) }
    public var isMerge: Bool { parents.count > 1 }

    public init(sha: String, parents: [String], subject: String, author: Signature, time: Int64, refs: [RefLabel] = [], graph: GraphRow? = nil) {
        self.sha = sha
        self.parents = parents
        self.subject = subject
        self.author = author
        self.time = time
        self.refs = refs
        self.graph = graph
    }
}

public struct LogPage: Codable, Hashable, Sendable {
    public var commits: [LogEntry]
    public var hasMore: Bool
    public var lanes: UInt16

    public init(commits: [LogEntry], hasMore: Bool, lanes: UInt16) {
        self.commits = commits
        self.hasMore = hasMore
        self.lanes = lanes
    }
}

public struct LogQuery: Codable, Hashable, Sendable {
    public var repo: String
    public var rev: String?
    public var all: Bool
    public var skip: UInt32
    public var limit: UInt32?
    public var query: String?

    public init(repo: String, rev: String? = nil, all: Bool = false, skip: UInt32 = 0, limit: UInt32? = nil, query: String? = nil) {
        self.repo = repo
        self.rev = rev
        self.all = all
        self.skip = skip
        self.limit = limit
        self.query = query
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(repo, forKey: .repo)
        try c.encodeIfPresent(rev, forKey: .rev)
        try c.encode(all, forKey: .all)
        try c.encode(skip, forKey: .skip)
        try c.encodeIfPresent(limit, forKey: .limit)
        try c.encodeIfPresent(query, forKey: .query)
    }
}

// MARK: - Commits and diffs

public enum ChangeStatus: String, Codable, Sendable {
    case added, modified, deleted, renamed, copied
    case typeChanged = "type_changed"
    case untracked, conflicted

    public var letter: String {
        switch self {
        case .added: "A"
        case .modified: "M"
        case .deleted: "D"
        case .renamed: "R"
        case .copied: "C"
        case .typeChanged: "T"
        case .untracked: "?"
        case .conflicted: "U"
        }
    }

    public var label: String {
        switch self {
        case .added: "Added"
        case .modified: "Modified"
        case .deleted: "Deleted"
        case .renamed: "Renamed"
        case .copied: "Copied"
        case .typeChanged: "Type changed"
        case .untracked: "Untracked"
        case .conflicted: "Conflicted"
        }
    }
}

public struct FileChange: Codable, Hashable, Identifiable, Sendable {
    public var path: String
    public var oldPath: String?
    public var status: ChangeStatus
    public var additions: UInt32
    public var deletions: UInt32
    public var binary: Bool

    public var id: String { path }

    public init(path: String, oldPath: String? = nil, status: ChangeStatus, additions: UInt32, deletions: UInt32, binary: Bool = false) {
        self.path = path
        self.oldPath = oldPath
        self.status = status
        self.additions = additions
        self.deletions = deletions
        self.binary = binary
    }
}

public struct CommitDetail: Codable, Hashable, Sendable {
    public var sha: String
    public var parents: [String]
    public var author: Signature
    public var committer: Signature
    public var subject: String
    public var body: String
    public var refs: [RefLabel]
    public var files: [FileChange]
    public var truncated: Bool

    public init(sha: String, parents: [String], author: Signature, committer: Signature, subject: String, body: String, refs: [RefLabel], files: [FileChange], truncated: Bool = false) {
        self.sha = sha
        self.parents = parents
        self.author = author
        self.committer = committer
        self.subject = subject
        self.body = body
        self.refs = refs
        self.files = files
        self.truncated = truncated
    }
}

public enum LineKind: String, Codable, Sendable {
    case context, added, removed
}

public struct Segment: Codable, Hashable, Sendable {
    public var text: String
    public var changed: Bool

    public init(text: String, changed: Bool) {
        self.text = text
        self.changed = changed
    }
}

public struct DiffLine: Codable, Hashable, Sendable {
    public var kind: LineKind
    public var oldLine: UInt32?
    public var newLine: UInt32?
    public var text: String
    public var segments: [Segment]
    public var noNewline: Bool

    public init(kind: LineKind, oldLine: UInt32?, newLine: UInt32?, text: String, segments: [Segment] = [], noNewline: Bool = false) {
        self.kind = kind
        self.oldLine = oldLine
        self.newLine = newLine
        self.text = text
        self.segments = segments
        self.noNewline = noNewline
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(LineKind.self, forKey: .kind)
        oldLine = try c.decodeIfPresent(UInt32.self, forKey: .oldLine)
        newLine = try c.decodeIfPresent(UInt32.self, forKey: .newLine)
        text = try c.decode(String.self, forKey: .text)
        segments = try c.decodeIfPresent([Segment].self, forKey: .segments) ?? []
        noNewline = try c.decodeIfPresent(Bool.self, forKey: .noNewline) ?? false
    }
}

public struct Hunk: Codable, Hashable, Sendable {
    public var oldStart: UInt32
    public var oldLines: UInt32
    public var newStart: UInt32
    public var newLines: UInt32
    public var lines: [DiffLine]

    public var header: String { "@@ -\(oldStart),\(oldLines) +\(newStart),\(newLines) @@" }

    public init(oldStart: UInt32, oldLines: UInt32, newStart: UInt32, newLines: UInt32, lines: [DiffLine]) {
        self.oldStart = oldStart
        self.oldLines = oldLines
        self.newStart = newStart
        self.newLines = newLines
        self.lines = lines
    }
}

public struct FileDiff: Codable, Hashable, Sendable {
    public var path: String
    public var oldPath: String?
    public var status: ChangeStatus
    public var binary: Bool
    public var tooLarge: Bool
    public var additions: UInt32
    public var deletions: UInt32
    public var hunks: [Hunk]

    public init(path: String, oldPath: String? = nil, status: ChangeStatus, binary: Bool = false, tooLarge: Bool = false, additions: UInt32, deletions: UInt32, hunks: [Hunk]) {
        self.path = path
        self.oldPath = oldPath
        self.status = status
        self.binary = binary
        self.tooLarge = tooLarge
        self.additions = additions
        self.deletions = deletions
        self.hunks = hunks
    }
}

// MARK: - Working copy

public struct StatusFile: Codable, Hashable, Identifiable, Sendable {
    public var path: String
    public var oldPath: String?
    public var status: ChangeStatus
    public var additions: UInt32
    public var deletions: UInt32
    public var binary: Bool

    public var id: String { path }

    public init(path: String, oldPath: String? = nil, status: ChangeStatus, additions: UInt32 = 0, deletions: UInt32 = 0, binary: Bool = false) {
        self.path = path
        self.oldPath = oldPath
        self.status = status
        self.additions = additions
        self.deletions = deletions
        self.binary = binary
    }
}

public struct WorkingStatus: Codable, Hashable, Sendable {
    public var head: Head?
    public var upstream: String?
    public var ahead: UInt32?
    public var behind: UInt32?
    public var operation: String?
    public var staged: [StatusFile]
    public var unstaged: [StatusFile]
    public var untracked: [StatusFile]
    public var conflicted: [StatusFile]

    public var isClean: Bool { staged.isEmpty && unstaged.isEmpty && untracked.isEmpty && conflicted.isEmpty }
    public var changeCount: Int { Set((staged + unstaged + untracked + conflicted).map(\.path)).count }

    public init(head: Head?, upstream: String? = nil, ahead: UInt32? = nil, behind: UInt32? = nil, operation: String? = nil, staged: [StatusFile] = [], unstaged: [StatusFile] = [], untracked: [StatusFile] = [], conflicted: [StatusFile] = []) {
        self.head = head
        self.upstream = upstream
        self.ahead = ahead
        self.behind = behind
        self.operation = operation
        self.staged = staged
        self.unstaged = unstaged
        self.untracked = untracked
        self.conflicted = conflicted
    }
}

/// Which side of the working copy: the index against HEAD, or the working tree against the index.
public enum Area: String, Codable, Hashable, Sendable {
    case staged, unstaged
}

/// A changed line picked in a working-copy diff: added lines by new number, removed by old.
public struct LinePick: Codable, Hashable, Sendable {
    public var kind: LineKind
    public var line: UInt32

    public init(kind: LineKind, line: UInt32) {
        self.kind = kind
        self.line = line
    }

    /// The pick for a diff line, or nil for context.
    public init?(_ l: DiffLine) {
        switch l.kind {
        case .added: guard let n = l.newLine else { return nil }; self.init(kind: .added, line: n)
        case .removed: guard let n = l.oldLine else { return nil }; self.init(kind: .removed, line: n)
        case .context: return nil
        }
    }
}

public struct RemoteInfo: Codable, Hashable, Identifiable, Sendable {
    public var name: String
    public var url: String
    public var id: String { name }

    public init(name: String, url: String) {
        self.name = name
        self.url = url
    }
}

public struct RepoEvent: Codable, Hashable, Sendable {
    public var repo: String
    /// `worktree` or `refs`.
    public var kind: String

    public init(repo: String, kind: String) {
        self.repo = repo
        self.kind = kind
    }
}

public struct EngineInfo: Codable, Hashable, Sendable {
    public var version: String
    public var `protocol`: UInt32
    public var role: String
    public var repos: UInt32

    public init(version: String, protocol: UInt32, role: String, repos: UInt32) {
        self.version = version
        self.protocol = `protocol`
        self.role = role
        self.repos = repos
    }
}

// MARK: - Control API

public enum Request: Sendable, Equatable {
    case ping
    case engineInfo
    case listRepos
    case addRepo(path: String)
    case removeRepo(repo: String)
    case branches(repo: String)
    case log(LogQuery)
    case commit(repo: String, sha: String)
    case fileDiff(repo: String, sha: String, path: String, context: UInt32?)
    case tree(repo: String, rev: String?, path: String)
    case treeCommits(repo: String, rev: String?, path: String)
    case blob(repo: String, rev: String?, path: String)
    case overview(repo: String, rev: String?)
    case status(repo: String)
    case workDiff(repo: String, path: String, area: Area, context: UInt32?)
    case stage(repo: String, paths: [String])
    case unstage(repo: String, paths: [String])
    case stageLines(repo: String, path: String, area: Area, lines: [LinePick])
    case discard(repo: String, paths: [String])
    case discardLines(repo: String, path: String, lines: [LinePick])
    case commitChanges(repo: String, message: String, amend: Bool)
    case createBranch(repo: String, name: String, start: String?, checkout: Bool)
    case checkout(repo: String, branch: String)
    case renameBranch(repo: String, from: String, to: String)
    case deleteBranch(repo: String, name: String, force: Bool)
    case subscribe
    case remotes(repo: String)
    case addRemote(repo: String, name: String, url: String)
    case fetch(repo: String, remote: String?)
    case push(repo: String, remote: String?, branch: String?, setUpstream: Bool)
    case createHosted(name: String, description: String?)
    case listPulls(repo: String)
    case createPull(repo: String, title: String, body: String, source: String, target: String)
    case pull(repo: String, number: UInt32)
    case updatePull(repo: String, number: UInt32, title: String?, body: String?, state: PullState?)
    case addNote(repo: String, number: UInt32, path: String, side: Side, line: UInt32, text: String)
    case resolveNote(repo: String, number: UInt32, note: Int64, resolved: Bool)
    case mergePull(repo: String, number: UInt32, strategy: MergeStrategy, message: String?, deleteSource: Bool)
    case compare(repo: String, base: String, head: String)
    case compareDiff(repo: String, base: String, head: String, path: String, context: UInt32?)
    case branchRules(repo: String)
    case setBranchRule(repo: String, rule: BranchRule)
    case deleteBranchRule(repo: String, pattern: String)
    case overrides(repo: String)
    case listRuns(repo: String, pull: UInt32?, limit: UInt32?)
    case run(repo: String, id: Int64)
    case jobLog(repo: String, job: Int64, offset: UInt64)
    case cancelRun(repo: String, id: Int64)
    case rerunRun(repo: String, id: Int64)
    case dispatchRun(repo: String, workflow: String, branch: String)
    case workflows(repo: String, branch: String)
    case ciInfo
    case vaultInfo
    case listSecrets(scope: VaultScope)
    case setSecret(scope: VaultScope, name: String, value: String)
    case deleteSecret(scope: VaultScope, name: String)
    case listVariables(scope: VaultScope)
    case setVariable(scope: VaultScope, name: String, value: String)
    case deleteVariable(scope: VaultScope, name: String)
    case listEnvironments(repo: String)
    case setEnvironment(DeployEnvironment)
    case deleteEnvironment(repo: String, name: String)
    case approveJob(repo: String, job: Int64, approve: Bool)
    case listArtifacts(repo: String, run: Int64)
    case artifactData(repo: String, run: Int64, path: String, offset: UInt64)
    case registerKey(publicKey: String)
    case registerPush(token: String?, sandbox: Bool, events: [String])
    case signed(request: String, at: Int64, nonce: String, signature: String)
    case inbox
    case notification(id: String)
    case pair(code: String, device: String)
    case auth(token: String)
    case cancel(id: UInt64)
}

public enum Response: Sendable, Equatable {
    case pong(protocol: UInt32)
    case engine(EngineInfo)
    case repos([Repo])
    case repo(Repo)
    case removed(repo: String)
    case branches([Branch])
    case log(LogPage)
    case commit(CommitDetail)
    case fileDiff(FileDiff)
    case tree(TreeListing)
    case treeCommits([EntryCommit])
    case blob(BlobContent)
    case overview(RepoOverview)
    case status(WorkingStatus)
    case done(output: String)
    case committed(sha: String, output: String)
    case event(RepoEvent)
    case remotes([RemoteInfo])
    case pulls([PullRequest])
    case pull(PullDetail)
    case merged(sha: String)
    case compare([FileChange])
    case rules([BranchRule])
    case overrides([RuleOverride])
    case runs([Run])
    case run(RunDetail)
    case jobLog(LogChunk)
    case ci(CiInfo)
    case workflows([WorkflowFile])
    case vault(VaultInfo)
    case secrets([SecretInfo])
    case variables([Variable])
    case environments([DeployEnvironment])
    case artifacts([Artifact])
    case artifactChunk(ArtifactChunk)
    case inbox(Inbox)
    case notification(NotificationText)
    case paired(token: String, deviceID: String, host: String)
    case authenticated(host: String, role: String)
    case cancelled
    case error(message: String)
}

public enum WireError: Error, Equatable {
    case unknownType(String)
}

public enum Wire {
    static func encoder() -> JSONEncoder {
        let e = JSONEncoder()
        e.keyEncodingStrategy = .convertToSnakeCase
        e.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return e
    }

    static func decoder() -> JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    private struct Envelope<P: Encodable>: Encodable {
        var id: UInt64
        var method: String
        var params: P?
    }

    private struct NoParams: Encodable {}
    private struct RepoParam: Encodable { var repo: String }
    private struct PathParam: Encodable { var path: String }
    private struct IDParam: Encodable { var id: UInt64 }
    private struct CommitParam: Encodable { var repo: String; var sha: String }
    private struct TreeParam: Encodable { var repo: String; var rev: String?; var path: String }
    private struct OverviewParam: Encodable { var repo: String; var rev: String? }
    private struct WorkDiffParam: Encodable { var repo: String; var path: String; var area: Area; var context: UInt32? }
    private struct PathsParam: Encodable { var repo: String; var paths: [String] }
    private struct LinesParam: Encodable { var repo: String; var path: String; var area: Area?; var lines: [LinePick] }
    private struct CommitChangesParam: Encodable { var repo: String; var message: String; var amend: Bool }
    private struct CreateBranchParam: Encodable { var repo: String; var name: String; var start: String?; var checkout: Bool }
    private struct CheckoutParam: Encodable { var repo: String; var branch: String }
    private struct RenameParam: Encodable { var repo: String; var from: String; var to: String }
    private struct DeleteParam: Encodable { var repo: String; var name: String; var force: Bool }
    private struct AddRemoteParam: Encodable { var repo: String; var name: String; var url: String }
    private struct FetchParam: Encodable { var repo: String; var remote: String? }
    private struct PushParam: Encodable { var repo: String; var remote: String?; var branch: String?; var setUpstream: Bool }
    private struct HostParam: Encodable { var name: String; var description: String? }
    private struct PairParam: Encodable { var code: String; var device: String }
    private struct AuthParam: Encodable { var token: String }
    private struct CreatePullParam: Encodable { var repo: String; var title: String; var body: String; var source: String; var target: String }
    private struct PullParam: Encodable { var repo: String; var number: UInt32 }
    private struct UpdatePullParam: Encodable { var repo: String; var number: UInt32; var title: String?; var body: String?; var state: PullState? }
    private struct NoteParam: Encodable { var repo: String; var number: UInt32; var path: String; var side: Side; var line: UInt32; var text: String }
    private struct ResolveParam: Encodable { var repo: String; var number: UInt32; var note: Int64; var resolved: Bool }
    private struct MergeParam: Encodable { var repo: String; var number: UInt32; var strategy: MergeStrategy; var message: String?; var deleteSource: Bool }
    private struct CompareParam: Encodable { var repo: String; var base: String; var head: String }
    private struct CompareDiffParam: Encodable { var repo: String; var base: String; var head: String; var path: String; var context: UInt32? }
    private struct RuleParam: Encodable { var repo: String; var rule: BranchRule }
    private struct PatternParam: Encodable { var repo: String; var pattern: String }
    private struct ListRunsParam: Encodable { var repo: String; var pull: UInt32?; var limit: UInt32? }
    private struct RunParam: Encodable { var repo: String; var id: Int64 }
    private struct JobLogParam: Encodable { var repo: String; var job: Int64; var offset: UInt64 }
    private struct DispatchParam: Encodable { var repo: String; var workflow: String; var branch: String }
    private struct BranchParam: Encodable { var repo: String; var branch: String }
    private struct ScopeParam: Encodable { var scope: VaultScope }
    private struct NamedValueParam: Encodable { var scope: VaultScope; var name: String; var value: String }
    private struct NamedParam: Encodable { var scope: VaultScope; var name: String }
    private struct EnvironmentParam: Encodable { var environment: DeployEnvironment }
    private struct RepoNameParam: Encodable { var repo: String; var name: String }
    private struct ApproveParam: Encodable { var repo: String; var job: Int64; var approve: Bool }
    private struct ArtifactsParam: Encodable { var repo: String; var run: Int64 }
    private struct ArtifactDataParam: Encodable { var repo: String; var run: Int64; var path: String; var offset: UInt64 }
    private struct KeyParam: Encodable { var publicKey: String }
    private struct PushParam2: Encodable { var token: String?; var sandbox: Bool; var events: [String] }
    private struct SignedParam: Encodable { var request: String; var at: Int64; var nonce: String; var signature: String }
    private struct NotificationParam: Encodable { var id: String }
    private struct DiffParam: Encodable {
        var repo: String
        var sha: String
        var path: String
        var context: UInt32?
    }

    private static func line<P: Encodable>(_ id: UInt64, _ method: String, _ params: P?) throws -> Data {
        var data = try encoder().encode(Envelope(id: id, method: method, params: params))
        data.append(0x0A)
        return data
    }

    /// One request line, newline included.
    public static func line(id: UInt64, _ request: Request) throws -> Data {
        switch request {
        case .ping: try line(id, "ping", NoParams?.none)
        case .engineInfo: try line(id, "engine_info", NoParams?.none)
        case .listRepos: try line(id, "list_repos", NoParams?.none)
        case let .addRepo(path): try line(id, "add_repo", PathParam(path: path))
        case let .removeRepo(repo): try line(id, "remove_repo", RepoParam(repo: repo))
        case let .branches(repo): try line(id, "branches", RepoParam(repo: repo))
        case let .log(q): try line(id, "log", q)
        case let .commit(repo, sha): try line(id, "commit", CommitParam(repo: repo, sha: sha))
        case let .fileDiff(repo, sha, path, context): try line(id, "file_diff", DiffParam(repo: repo, sha: sha, path: path, context: context))
        case let .tree(repo, rev, path): try line(id, "tree", TreeParam(repo: repo, rev: rev, path: path))
        case let .treeCommits(repo, rev, path): try line(id, "tree_commits", TreeParam(repo: repo, rev: rev, path: path))
        case let .blob(repo, rev, path): try line(id, "blob", TreeParam(repo: repo, rev: rev, path: path))
        case let .overview(repo, rev): try line(id, "overview", OverviewParam(repo: repo, rev: rev))
        case let .status(repo): try line(id, "status", RepoParam(repo: repo))
        case let .workDiff(repo, path, area, context): try line(id, "work_diff", WorkDiffParam(repo: repo, path: path, area: area, context: context))
        case let .stage(repo, paths): try line(id, "stage", PathsParam(repo: repo, paths: paths))
        case let .unstage(repo, paths): try line(id, "unstage", PathsParam(repo: repo, paths: paths))
        case let .stageLines(repo, path, area, lines): try line(id, "stage_lines", LinesParam(repo: repo, path: path, area: area, lines: lines))
        case let .discard(repo, paths): try line(id, "discard", PathsParam(repo: repo, paths: paths))
        case let .discardLines(repo, path, lines): try line(id, "discard_lines", LinesParam(repo: repo, path: path, area: nil, lines: lines))
        case let .commitChanges(repo, message, amend): try line(id, "commit_changes", CommitChangesParam(repo: repo, message: message, amend: amend))
        case let .createBranch(repo, name, start, checkout): try line(id, "create_branch", CreateBranchParam(repo: repo, name: name, start: start, checkout: checkout))
        case let .checkout(repo, branch): try line(id, "checkout", CheckoutParam(repo: repo, branch: branch))
        case let .renameBranch(repo, from, to): try line(id, "rename_branch", RenameParam(repo: repo, from: from, to: to))
        case let .deleteBranch(repo, name, force): try line(id, "delete_branch", DeleteParam(repo: repo, name: name, force: force))
        case .subscribe: try line(id, "subscribe", NoParams?.none)
        case let .remotes(repo): try line(id, "remotes", RepoParam(repo: repo))
        case let .addRemote(repo, name, url): try line(id, "add_remote", AddRemoteParam(repo: repo, name: name, url: url))
        case let .fetch(repo, remote): try line(id, "fetch", FetchParam(repo: repo, remote: remote))
        case let .push(repo, remote, branch, up): try line(id, "push", PushParam(repo: repo, remote: remote, branch: branch, setUpstream: up))
        case let .createHosted(name, description): try line(id, "create_hosted", HostParam(name: name, description: description))
        case let .listPulls(repo): try line(id, "list_pulls", RepoParam(repo: repo))
        case let .createPull(repo, title, body, source, target):
            try line(id, "create_pull", CreatePullParam(repo: repo, title: title, body: body, source: source, target: target))
        case let .pull(repo, number): try line(id, "pull", PullParam(repo: repo, number: number))
        case let .updatePull(repo, number, title, body, state):
            try line(id, "update_pull", UpdatePullParam(repo: repo, number: number, title: title, body: body, state: state))
        case let .addNote(repo, number, path, side, l, text):
            try line(id, "add_note", NoteParam(repo: repo, number: number, path: path, side: side, line: l, text: text))
        case let .resolveNote(repo, number, note, resolved): try line(id, "resolve_note", ResolveParam(repo: repo, number: number, note: note, resolved: resolved))
        case let .mergePull(repo, number, strategy, message, deleteSource):
            try line(id, "merge_pull", MergeParam(repo: repo, number: number, strategy: strategy, message: message, deleteSource: deleteSource))
        case let .compare(repo, base, head): try line(id, "compare", CompareParam(repo: repo, base: base, head: head))
        case let .compareDiff(repo, base, head, path, context):
            try line(id, "compare_diff", CompareDiffParam(repo: repo, base: base, head: head, path: path, context: context))
        case let .branchRules(repo): try line(id, "branch_rules", RepoParam(repo: repo))
        case let .setBranchRule(repo, rule): try line(id, "set_branch_rule", RuleParam(repo: repo, rule: rule))
        case let .deleteBranchRule(repo, pattern): try line(id, "delete_branch_rule", PatternParam(repo: repo, pattern: pattern))
        case let .overrides(repo): try line(id, "overrides", RepoParam(repo: repo))
        case let .listRuns(repo, pull, limit): try line(id, "list_runs", ListRunsParam(repo: repo, pull: pull, limit: limit))
        case let .run(repo, run): try line(id, "run", RunParam(repo: repo, id: run))
        case let .jobLog(repo, job, offset): try line(id, "job_log", JobLogParam(repo: repo, job: job, offset: offset))
        case let .cancelRun(repo, run): try line(id, "cancel_run", RunParam(repo: repo, id: run))
        case let .rerunRun(repo, run): try line(id, "rerun_run", RunParam(repo: repo, id: run))
        case let .dispatchRun(repo, workflow, branch): try line(id, "dispatch_run", DispatchParam(repo: repo, workflow: workflow, branch: branch))
        case let .workflows(repo, branch): try line(id, "workflows", BranchParam(repo: repo, branch: branch))
        case .ciInfo: try line(id, "ci_info", NoParams?.none)
        case .vaultInfo: try line(id, "vault_info", NoParams?.none)
        case let .listSecrets(scope): try line(id, "list_secrets", ScopeParam(scope: scope))
        case let .setSecret(scope, name, value): try line(id, "set_secret", NamedValueParam(scope: scope, name: name, value: value))
        case let .deleteSecret(scope, name): try line(id, "delete_secret", NamedParam(scope: scope, name: name))
        case let .listVariables(scope): try line(id, "list_variables", ScopeParam(scope: scope))
        case let .setVariable(scope, name, value): try line(id, "set_variable", NamedValueParam(scope: scope, name: name, value: value))
        case let .deleteVariable(scope, name): try line(id, "delete_variable", NamedParam(scope: scope, name: name))
        case let .listEnvironments(repo): try line(id, "list_environments", RepoParam(repo: repo))
        case let .setEnvironment(env): try line(id, "set_environment", EnvironmentParam(environment: env))
        case let .deleteEnvironment(repo, name): try line(id, "delete_environment", RepoNameParam(repo: repo, name: name))
        case let .approveJob(repo, job, approve): try line(id, "approve_job", ApproveParam(repo: repo, job: job, approve: approve))
        case let .listArtifacts(repo, run): try line(id, "list_artifacts", ArtifactsParam(repo: repo, run: run))
        case let .artifactData(repo, run, path, offset): try line(id, "artifact_data", ArtifactDataParam(repo: repo, run: run, path: path, offset: offset))
        case let .registerKey(key): try line(id, "register_key", KeyParam(publicKey: key))
        case let .registerPush(token, sandbox, events): try line(id, "register_push", PushParam2(token: token, sandbox: sandbox, events: events))
        case let .signed(request, at, nonce, signature): try line(id, "signed", SignedParam(request: request, at: at, nonce: nonce, signature: signature))
        case .inbox: try line(id, "inbox", NoParams?.none)
        case let .notification(nid): try line(id, "notification", NotificationParam(id: nid))
        case let .pair(code, device): try line(id, "pair", PairParam(code: code, device: device))
        case let .auth(token): try line(id, "auth", AuthParam(token: token))
        case let .cancel(target): try line(id, "cancel", IDParam(id: target))
        }
    }

    private struct Header: Decodable {
        var id: UInt64
        var type: String
    }

    private struct Payload<T: Decodable>: Decodable {
        var data: T
    }

    private struct Pong: Decodable { var `protocol`: UInt32 }
    private struct Removed: Decodable { var repo: String }
    private struct Message: Decodable { var message: String }
    private struct Output: Decodable { var output: String }
    private struct Committed: Decodable { var sha: String; var output: String }
    private struct Paired: Decodable { var token: String; var deviceId: String; var host: String }
    private struct Authenticated: Decodable { var host: String; var role: String }
    private struct MergedSha: Decodable { var sha: String }

    /// Parse one response line into its id and response.
    public static func decode(_ line: Data) throws -> (UInt64, Response) {
        let d = decoder()
        let header = try d.decode(Header.self, from: line)
        func data<T: Decodable>(_: T.Type) throws -> T { try d.decode(Payload<T>.self, from: line).data }
        let response: Response = switch header.type {
        case "pong": .pong(protocol: try data(Pong.self).protocol)
        case "engine": .engine(try data(EngineInfo.self))
        case "repos": .repos(try data([Repo].self))
        case "repo": .repo(try data(Repo.self))
        case "removed": .removed(repo: try data(Removed.self).repo)
        case "branches": .branches(try data([Branch].self))
        case "log": .log(try data(LogPage.self))
        case "commit": .commit(try data(CommitDetail.self))
        case "file_diff": .fileDiff(try data(FileDiff.self))
        case "tree": .tree(try data(TreeListing.self))
        case "tree_commits": .treeCommits(try data([EntryCommit].self))
        case "blob": .blob(try data(BlobContent.self))
        case "overview": .overview(try data(RepoOverview.self))
        case "status": .status(try data(WorkingStatus.self))
        case "done": .done(output: try data(Output.self).output)
        case "committed": try { let c = try data(Committed.self); return .committed(sha: c.sha, output: c.output) }()
        case "event": .event(try data(RepoEvent.self))
        case "remotes": .remotes(try data([RemoteInfo].self))
        case "pulls": .pulls(try data([PullRequest].self))
        case "pull": .pull(try data(PullDetail.self))
        case "merged": .merged(sha: try data(MergedSha.self).sha)
        case "compare": .compare(try data([FileChange].self))
        case "rules": .rules(try data([BranchRule].self))
        case "overrides": .overrides(try data([RuleOverride].self))
        case "runs": .runs(try data([Run].self))
        case "run": .run(try data(RunDetail.self))
        case "job_log": .jobLog(try data(LogChunk.self))
        case "ci": .ci(try data(CiInfo.self))
        case "workflows": .workflows(try data([WorkflowFile].self))
        case "vault": .vault(try data(VaultInfo.self))
        case "secrets": .secrets(try data([SecretInfo].self))
        case "variables": .variables(try data([Variable].self))
        case "environments": .environments(try data([DeployEnvironment].self))
        case "artifacts": .artifacts(try data([Artifact].self))
        case "artifact_chunk": .artifactChunk(try data(ArtifactChunk.self))
        case "inbox": .inbox(try data(Inbox.self))
        case "notification": .notification(try data(NotificationText.self))
        case "paired": try { let p = try data(Paired.self); return .paired(token: p.token, deviceID: p.deviceId, host: p.host) }()
        case "authenticated": try { let a = try data(Authenticated.self); return .authenticated(host: a.host, role: a.role) }()
        case "cancelled": .cancelled
        case "error": .error(message: try data(Message.self).message)
        default: throw WireError.unknownType(header.type)
        }
        return (header.id, response)
    }
}
