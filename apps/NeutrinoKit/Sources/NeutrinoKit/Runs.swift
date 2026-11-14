import Foundation

// CI runs. Mirrors the milestone 5 types in crates/neutrino-proto; they live
// on the server, so `ServerService` serves them.

public enum RunState: String, Codable, Hashable, Sendable {
    case queued, running, success, failure, cancelled, skipped

    public var finished: Bool { self != .queued && self != .running }

    public var label: String {
        switch self {
        case .queued: "Queued"
        case .running: "Running"
        case .success: "Passed"
        case .failure: "Failed"
        case .cancelled: "Cancelled"
        case .skipped: "Skipped"
        }
    }
}

public struct Run: Codable, Hashable, Identifiable, Sendable {
    public var id: Int64
    public var repo: String
    public var number: UInt32
    public var workflow: String
    public var name: String
    public var event: String
    public var branch: String?
    public var sha: String
    public var pull: UInt32?
    public var state: RunState
    public var error: String?
    public var createdAt: Int64
    public var startedAt: Int64?
    public var finishedAt: Int64?

    public var shortSHA: String { String(sha.prefix(7)) }

    /// Seconds it ran, or has been running.
    public func duration(now: Date = Date()) -> Int64? {
        guard let start = startedAt else { return nil }
        return (finishedAt ?? Int64(now.timeIntervalSince1970)) - start
    }

    public var eventLabel: String {
        switch event {
        case "push": "Push"
        case "pull_request": "Pull request"
        case "workflow_dispatch": "Manual"
        default: event
        }
    }

    public init(id: Int64, repo: String, number: UInt32, workflow: String, name: String, event: String, branch: String?, sha: String, pull: UInt32?,
                state: RunState, error: String? = nil, createdAt: Int64, startedAt: Int64? = nil, finishedAt: Int64? = nil) {
        self.id = id
        self.repo = repo
        self.number = number
        self.workflow = workflow
        self.name = name
        self.event = event
        self.branch = branch
        self.sha = sha
        self.pull = pull
        self.state = state
        self.error = error
        self.createdAt = createdAt
        self.startedAt = startedAt
        self.finishedAt = finishedAt
    }
}

public struct RunStep: Codable, Hashable, Identifiable, Sendable {
    public var index: UInt32
    public var name: String
    public var state: RunState
    public var exitCode: Int32?
    public var logStart: UInt64
    public var logEnd: UInt64
    public var startedAt: Int64?
    public var finishedAt: Int64?

    public var id: UInt32 { index }
}

public struct RunJob: Codable, Hashable, Identifiable, Sendable {
    public var id: Int64
    public var run: Int64
    public var key: String
    public var name: String
    public var executor: String
    public var image: String?
    public var state: RunState
    public var error: String?
    public var needs: [String]
    public var attempt: UInt32
    public var startedAt: Int64?
    public var finishedAt: Int64?
    public var steps: [RunStep]
}

public struct RunDetail: Codable, Hashable, Sendable {
    public var run: Run
    public var jobs: [RunJob]
    public var warnings: [String]

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        run = try c.decode(Run.self, forKey: .run)
        jobs = try c.decode([RunJob].self, forKey: .jobs)
        warnings = try c.decodeIfPresent([String].self, forKey: .warnings) ?? []
    }

    public init(run: Run, jobs: [RunJob], warnings: [String] = []) {
        self.run = run
        self.jobs = jobs
        self.warnings = warnings
    }
}

public struct LogChunk: Codable, Hashable, Sendable {
    public var job: Int64
    public var offset: UInt64
    public var text: String
    public var next: UInt64
    public var done: Bool
}

public struct CiInfo: Codable, Hashable, Sendable {
    public var graviton: Bool
    public var gravitonError: String?
    public var budgetCpus: UInt32
    public var budgetMemoryMib: UInt64
    public var usedCpus: UInt32
    public var usedMemoryMib: UInt64
    public var running: UInt32
    public var queued: UInt32
}

public struct WorkflowFile: Codable, Hashable, Identifiable, Sendable {
    public var file: String
    public var name: String?
    public var triggers: [String]
    public var jobs: [String]
    public var error: String?
    public var warnings: [String]

    public var id: String { file }
}

/// Required checks that haven't passed, as the server works them out.
public func missingChecks(_ required: [String], in runs: [Run]) -> [String] {
    required.filter { name in
        !runs.contains { r in
            (r.name.caseInsensitiveCompare(name) == .orderedSame || r.workflow == name || r.workflow.replacingOccurrences(of: ".yml", with: "") == name) && r.state == .success
        }
    }
}

// MARK: - Service

public protocol RunService: Sendable {
    func runs(repo: String, pull: UInt32?) async throws -> [Run]
    func run(repo: String, id: Int64) async throws -> RunDetail
    func jobLog(repo: String, job: Int64, offset: UInt64) async throws -> LogChunk
    func cancelRun(repo: String, id: Int64) async throws -> RunDetail
    func rerun(repo: String, id: Int64) async throws -> RunDetail
    func dispatch(repo: String, workflow: String, branch: String) async throws -> RunDetail
    func workflows(repo: String, branch: String) async throws -> [WorkflowFile]
    func ciInfo() async throws -> CiInfo
}

extension ServerService: RunService {
    private func runDetail(_ r: Request) async throws -> RunDetail {
        guard case let .run(d) = try await call(r) else { throw ServiceError.unexpectedResponse }
        return d
    }

    public func runs(repo: String, pull: UInt32?) async throws -> [Run] {
        guard case let .runs(list) = try await call(.listRuns(repo: repo, pull: pull, limit: 100)) else { throw ServiceError.unexpectedResponse }
        return list
    }

    public func run(repo: String, id: Int64) async throws -> RunDetail { try await runDetail(.run(repo: repo, id: id)) }
    public func cancelRun(repo: String, id: Int64) async throws -> RunDetail { try await runDetail(.cancelRun(repo: repo, id: id)) }
    public func rerun(repo: String, id: Int64) async throws -> RunDetail { try await runDetail(.rerunRun(repo: repo, id: id)) }
    public func dispatch(repo: String, workflow: String, branch: String) async throws -> RunDetail {
        try await runDetail(.dispatchRun(repo: repo, workflow: workflow, branch: branch))
    }

    public func jobLog(repo: String, job: Int64, offset: UInt64) async throws -> LogChunk {
        guard case let .jobLog(chunk) = try await call(.jobLog(repo: repo, job: job, offset: offset)) else { throw ServiceError.unexpectedResponse }
        return chunk
    }

    public func workflows(repo: String, branch: String) async throws -> [WorkflowFile] {
        guard case let .workflows(list) = try await call(.workflows(repo: repo, branch: branch)) else { throw ServiceError.unexpectedResponse }
        return list
    }

    public func ciInfo() async throws -> CiInfo {
        guard case let .ci(info) = try await call(.ciInfo) else { throw ServiceError.unexpectedResponse }
        return info
    }
}
