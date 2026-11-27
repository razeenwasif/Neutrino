import Foundation

// Pull requests and branch rules. Mirrors the milestone 4 types in
// crates/neutrino-proto. They live on the server, so only `ServerService`
// (and the demo) serve them.

public enum PullState: String, Codable, Hashable, Sendable {
    case open, merged, closed

    public var label: String {
        switch self {
        case .open: "Open"
        case .merged: "Merged"
        case .closed: "Closed"
        }
    }
}

public struct PullRequest: Codable, Hashable, Identifiable, Sendable {
    public var repo: String
    public var number: UInt32
    public var title: String
    public var body: String
    public var source: String
    public var target: String
    public var state: PullState
    public var headSha: String
    public var versions: UInt32
    public var openNotes: UInt32
    public var createdAt: Int64
    public var updatedAt: Int64
    public var mergedAt: Int64?
    public var mergeSha: String?
    public var stackId: String?
    public var parentPull: UInt32?
    public var stackConflict: String?

    public var id: String { "\(repo)#\(number)" }

    public init(repo: String, number: UInt32, title: String, body: String, source: String, target: String, state: PullState, headSha: String,
                versions: UInt32, openNotes: UInt32, createdAt: Int64, updatedAt: Int64, mergedAt: Int64? = nil, mergeSha: String? = nil,
                stackId: String? = nil, parentPull: UInt32? = nil, stackConflict: String? = nil) {
        self.repo = repo
        self.number = number
        self.title = title
        self.body = body
        self.source = source
        self.target = target
        self.state = state
        self.headSha = headSha
        self.versions = versions
        self.openNotes = openNotes
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.mergedAt = mergedAt
        self.mergeSha = mergeSha
        self.stackId = stackId
        self.parentPull = parentPull
        self.stackConflict = stackConflict
    }
}

public struct PullVersion: Codable, Hashable, Identifiable, Sendable {
    public var version: UInt32
    public var headSha: String
    public var baseSha: String
    public var createdAt: Int64

    public var id: UInt32 { version }

    public init(version: UInt32, headSha: String, baseSha: String, createdAt: Int64) {
        self.version = version
        self.headSha = headSha
        self.baseSha = baseSha
        self.createdAt = createdAt
    }
}

public enum Side: String, Codable, Hashable, Sendable {
    case old, new
}

public struct Note: Codable, Hashable, Identifiable, Sendable {
    public var id: Int64
    public var version: UInt32
    public var path: String
    public var side: Side
    public var line: UInt32
    public var text: String
    public var createdAt: Int64
    public var resolved: Bool
    /// Where the line is in the latest version; nil when it changed since.
    public var currentLine: UInt32?

    public var outdated: Bool { currentLine == nil }

    public init(id: Int64, version: UInt32, path: String, side: Side, line: UInt32, text: String, createdAt: Int64, resolved: Bool, currentLine: UInt32?) {
        self.id = id
        self.version = version
        self.path = path
        self.side = side
        self.line = line
        self.text = text
        self.createdAt = createdAt
        self.resolved = resolved
        self.currentLine = currentLine
    }
}

public enum MergeStrategy: String, Codable, Hashable, Sendable, CaseIterable, Identifiable {
    case merge, squash, rebase
    case fastForward = "fast_forward"

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .merge: "Merge commit"
        case .squash: "Squash"
        case .rebase: "Rebase"
        case .fastForward: "Fast-forward"
        }
    }

    public var detail: String {
        switch self {
        case .merge: "Keeps every commit and adds a merge commit."
        case .squash: "Combines the changes into one commit."
        case .rebase: "Replays each commit on top of the target."
        case .fastForward: "Moves the target to the source. Only when the target hasn't moved."
        }
    }
}

public struct Mergeability: Codable, Hashable, Sendable {
    public var clean: Bool
    public var conflicts: [String]
    public var fastForward: Bool
    public var behind: UInt32

    public init(clean: Bool, conflicts: [String] = [], fastForward: Bool, behind: UInt32) {
        self.clean = clean
        self.conflicts = conflicts
        self.fastForward = fastForward
        self.behind = behind
    }
}

public struct BranchRule: Codable, Hashable, Identifiable, Sendable {
    public var pattern: String
    public var requirePull: Bool
    public var blockForce: Bool
    public var blockDelete: Bool
    public var requiredChecks: [String]

    public var id: String { pattern }

    public init(pattern: String, requirePull: Bool = true, blockForce: Bool = true, blockDelete: Bool = true, requiredChecks: [String] = []) {
        self.pattern = pattern
        self.requirePull = requirePull
        self.blockForce = blockForce
        self.blockDelete = blockDelete
        self.requiredChecks = requiredChecks
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        pattern = try c.decode(String.self, forKey: .pattern)
        requirePull = try c.decode(Bool.self, forKey: .requirePull)
        blockForce = try c.decode(Bool.self, forKey: .blockForce)
        blockDelete = try c.decode(Bool.self, forKey: .blockDelete)
        requiredChecks = try c.decodeIfPresent([String].self, forKey: .requiredChecks) ?? []
    }

    /// What the rule guards against, in words.
    public var summary: String {
        var parts: [String] = []
        if requirePull { parts.append("pull requests only") }
        if blockForce { parts.append("no force-push") }
        if blockDelete { parts.append("no delete") }
        return parts.isEmpty ? "nothing" : parts.joined(separator: ", ")
    }
}

public struct RuleOverride: Codable, Hashable, Sendable {
    public var refName: String
    public var rule: String
    public var reason: String
    public var at: Int64

    public init(refName: String, rule: String, reason: String, at: Int64) {
        self.refName = refName
        self.rule = rule
        self.reason = reason
        self.at = at
    }
}

public enum SymbolDeltaKind: String, Codable, Sendable {
    case added, removed
    case signatureChanged = "signature_changed"
}

public struct SymbolDelta: Codable, Hashable, Identifiable, Sendable {
    public var path: String
    public var symbol: String
    public var kind: String
    public var change: SymbolDeltaKind

    public var id: String { "\(path):\(kind):\(symbol):\(change.rawValue)" }

    public init(path: String, symbol: String, kind: String, change: SymbolDeltaKind) {
        self.path = path
        self.symbol = symbol
        self.kind = kind
        self.change = change
    }
}

public struct VerificationReceipt: Codable, Hashable, Sendable {
    public var publicSymbolDeltas: [SymbolDelta]
    public var dependencyFiles: [String]
    public var securityPaths: [String]
    public var verifiedChecks: [String]
    public var provenanceCount: UInt32
    public var deviceSignedCount: UInt32
    public var riskLevel: String

    public init(
        publicSymbolDeltas: [SymbolDelta] = [],
        dependencyFiles: [String] = [],
        securityPaths: [String] = [],
        verifiedChecks: [String] = [],
        provenanceCount: UInt32 = 0,
        deviceSignedCount: UInt32 = 0,
        riskLevel: String = "low"
    ) {
        self.publicSymbolDeltas = publicSymbolDeltas
        self.dependencyFiles = dependencyFiles
        self.securityPaths = securityPaths
        self.verifiedChecks = verifiedChecks
        self.provenanceCount = provenanceCount
        self.deviceSignedCount = deviceSignedCount
        self.riskLevel = riskLevel
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        publicSymbolDeltas = try c.decodeIfPresent([SymbolDelta].self, forKey: .publicSymbolDeltas) ?? []
        dependencyFiles = try c.decodeIfPresent([String].self, forKey: .dependencyFiles) ?? []
        securityPaths = try c.decodeIfPresent([String].self, forKey: .securityPaths) ?? []
        verifiedChecks = try c.decodeIfPresent([String].self, forKey: .verifiedChecks) ?? []
        provenanceCount = try c.decodeIfPresent(UInt32.self, forKey: .provenanceCount) ?? 0
        deviceSignedCount = try c.decodeIfPresent(UInt32.self, forKey: .deviceSignedCount) ?? 0
        riskLevel = try c.decodeIfPresent(String.self, forKey: .riskLevel) ?? "low"
    }
}

public struct PullDetail: Codable, Hashable, Sendable {
    public var pull: PullRequest
    public var versions: [PullVersion]
    public var notes: [Note]
    public var mergeability: Mergeability?
    public var files: [FileChange]
    /// Oldest first.
    public var commits: [LogEntry]
    public var rule: BranchRule?
    /// The latest run of each workflow on the latest version.
    public var checks: [Run]
    public var stack: [StackMember]
    public var tour: [TourStep]
    public var moves: [MoveBlock]
    public var receipt: VerificationReceipt?

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        pull = try c.decode(PullRequest.self, forKey: .pull)
        versions = try c.decode([PullVersion].self, forKey: .versions)
        notes = try c.decode([Note].self, forKey: .notes)
        mergeability = try c.decodeIfPresent(Mergeability.self, forKey: .mergeability)
        files = try c.decode([FileChange].self, forKey: .files)
        commits = try c.decode([LogEntry].self, forKey: .commits)
        rule = try c.decodeIfPresent(BranchRule.self, forKey: .rule)
        checks = try c.decodeIfPresent([Run].self, forKey: .checks) ?? []
        stack = try c.decodeIfPresent([StackMember].self, forKey: .stack) ?? []
        tour = try c.decodeIfPresent([TourStep].self, forKey: .tour) ?? []
        moves = try c.decodeIfPresent([MoveBlock].self, forKey: .moves) ?? []
        receipt = try c.decodeIfPresent(VerificationReceipt.self, forKey: .receipt)
    }

    public init(pull: PullRequest, versions: [PullVersion], notes: [Note], mergeability: Mergeability?, files: [FileChange], commits: [LogEntry], rule: BranchRule?, checks: [Run] = [], stack: [StackMember] = [], tour: [TourStep] = [], moves: [MoveBlock] = [], receipt: VerificationReceipt? = nil) {
        self.checks = checks
        self.stack = stack
        self.tour = tour
        self.moves = moves
        self.receipt = receipt
        self.pull = pull
        self.versions = versions
        self.notes = notes
        self.mergeability = mergeability
        self.files = files
        self.commits = commits
        self.rule = rule
    }

    /// The base the latest version's files diff against: the target now for
    /// an open pull request, where it left the target otherwise.
    public var latest: PullVersion? { versions.last }
}

// MARK: - Service

/// Pull requests and rules, on the server.
public protocol PullService: Sendable {
    func pulls(repo: String) async throws -> [PullRequest]
    func createPull(repo: String, title: String, body: String, source: String, target: String, stackId: String?, parentPull: UInt32?) async throws -> PullDetail
    func pull(repo: String, number: UInt32) async throws -> PullDetail
    func updatePull(repo: String, number: UInt32, title: String?, body: String?, state: PullState?) async throws -> PullDetail
    func addNote(repo: String, number: UInt32, path: String, side: Side, line: UInt32, text: String) async throws -> PullDetail
    func resolveNote(repo: String, number: UInt32, note: Int64, resolved: Bool) async throws -> PullDetail
    func mergePull(repo: String, number: UInt32, strategy: MergeStrategy, message: String?, deleteSource: Bool) async throws -> String
    /// One file's diff from `base` (via the merge base) to `head`.
    func compareDiff(repo: String, base: String, head: String, path: String) async throws -> FileDiff
    func compareVersions(repo: String, number: UInt32, fromVersion: UInt32, toVersion: UInt32) async throws -> VersionInterdiff
    func compareVersionDiff(repo: String, number: UInt32, fromVersion: UInt32, toVersion: UInt32, path: String) async throws -> FileDiff
    func stacks(repo: String) async throws -> [StackInfo]
    func syncStack(repo: String, pull: UInt32) async throws -> StackSyncReport
    func rules(repo: String) async throws -> [BranchRule]
    func setRule(repo: String, rule: BranchRule) async throws -> [BranchRule]
    func deleteRule(repo: String, pattern: String) async throws -> [BranchRule]
    func overrides(repo: String) async throws -> [RuleOverride]
}

public extension PullService {
    func createPull(repo: String, title: String, body: String, source: String, target: String) async throws -> PullDetail {
        try await createPull(repo: repo, title: title, body: body, source: source, target: target, stackId: nil, parentPull: nil)
    }
}

extension ServerService: PullService {
    private func detail(_ r: Request) async throws -> PullDetail {
        guard case let .pull(d) = try await call(r) else { throw ServiceError.unexpectedResponse }
        return d
    }

    private func ruleList(_ r: Request) async throws -> [BranchRule] {
        guard case let .rules(list) = try await call(r) else { throw ServiceError.unexpectedResponse }
        return list
    }

    public func pulls(repo: String) async throws -> [PullRequest] {
        guard case let .pulls(list) = try await call(.listPulls(repo: repo)) else { throw ServiceError.unexpectedResponse }
        return list
    }

    public func createPull(repo: String, title: String, body: String, source: String, target: String, stackId: String? = nil, parentPull: UInt32? = nil) async throws -> PullDetail {
        try await detail(.createPull(repo: repo, title: title, body: body, source: source, target: target, stackId: stackId, parentPull: parentPull))
    }

    public func pull(repo: String, number: UInt32) async throws -> PullDetail {
        try await detail(.pull(repo: repo, number: number))
    }

    public func updatePull(repo: String, number: UInt32, title: String?, body: String?, state: PullState?) async throws -> PullDetail {
        try await detail(.updatePull(repo: repo, number: number, title: title, body: body, state: state))
    }

    public func addNote(repo: String, number: UInt32, path: String, side: Side, line: UInt32, text: String) async throws -> PullDetail {
        try await detail(.addNote(repo: repo, number: number, path: path, side: side, line: line, text: text))
    }

    public func resolveNote(repo: String, number: UInt32, note: Int64, resolved: Bool) async throws -> PullDetail {
        try await detail(.resolveNote(repo: repo, number: number, note: note, resolved: resolved))
    }

    public func mergePull(repo: String, number: UInt32, strategy: MergeStrategy, message: String?, deleteSource: Bool) async throws -> String {
        guard case let .merged(sha) = try await call(.mergePull(repo: repo, number: number, strategy: strategy, message: message, deleteSource: deleteSource)) else {
            throw ServiceError.unexpectedResponse
        }
        return sha
    }

    public func compareDiff(repo: String, base: String, head: String, path: String) async throws -> FileDiff {
        guard case let .fileDiff(d) = try await call(.compareDiff(repo: repo, base: base, head: head, path: path, context: 3)) else {
            throw ServiceError.unexpectedResponse
        }
        return d
    }

    public func compareVersions(repo: String, number: UInt32, fromVersion: UInt32, toVersion: UInt32) async throws -> VersionInterdiff {
        guard case let .versionDiff(v) = try await call(.compareVersions(repo: repo, number: number, fromVersion: fromVersion, toVersion: toVersion)) else {
            throw ServiceError.unexpectedResponse
        }
        return v
    }

    public func compareVersionDiff(repo: String, number: UInt32, fromVersion: UInt32, toVersion: UInt32, path: String) async throws -> FileDiff {
        guard case let .fileDiff(d) = try await call(.compareVersionDiff(repo: repo, number: number, fromVersion: fromVersion, toVersion: toVersion, path: path, context: 3)) else {
            throw ServiceError.unexpectedResponse
        }
        return d
    }

    public func stacks(repo: String) async throws -> [StackInfo] {
        guard case let .stacks(s) = try await call(.listStacks(repo: repo)) else {
            throw ServiceError.unexpectedResponse
        }
        return s
    }

    public func syncStack(repo: String, pull: UInt32) async throws -> StackSyncReport {
        guard case let .stackSynced(r) = try await call(.syncStack(repo: repo, pull: pull)) else {
            throw ServiceError.unexpectedResponse
        }
        return r
    }

    public func rules(repo: String) async throws -> [BranchRule] { try await ruleList(.branchRules(repo: repo)) }
    public func setRule(repo: String, rule: BranchRule) async throws -> [BranchRule] { try await ruleList(.setBranchRule(repo: repo, rule: rule)) }
    public func deleteRule(repo: String, pattern: String) async throws -> [BranchRule] { try await ruleList(.deleteBranchRule(repo: repo, pattern: pattern)) }

    public func overrides(repo: String) async throws -> [RuleOverride] {
        guard case let .overrides(list) = try await call(.overrides(repo: repo)) else { throw ServiceError.unexpectedResponse }
        return list
    }
}
