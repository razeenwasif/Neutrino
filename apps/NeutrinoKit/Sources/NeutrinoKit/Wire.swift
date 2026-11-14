import Foundation

// Mirrors crates/neutrino-proto. Keys are snake_case on the wire and
// camelCase here (the coders convert); enum values keep their wire spelling.

public let protocolVersion: UInt32 = 1

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

    public var letter: String {
        switch self {
        case .added: "A"
        case .modified: "M"
        case .deleted: "D"
        case .renamed: "R"
        case .copied: "C"
        case .typeChanged: "T"
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
        case "cancelled": .cancelled
        case "error": .error(message: try data(Message.self).message)
        default: throw WireError.unknownType(header.type)
        }
        return (header.id, response)
    }
}
