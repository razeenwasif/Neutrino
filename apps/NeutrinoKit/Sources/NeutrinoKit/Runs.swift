import Foundation

// CI runs. Mirrors the milestone 5 types in crates/neutrino-proto; they live
// on the server, so `ServerService` serves them.

public enum RunState: String, Codable, Hashable, Sendable {
    case queued, running, success, failure, cancelled, skipped, waiting

    public var finished: Bool { self != .queued && self != .running && self != .waiting }

    public var label: String {
        switch self {
        case .queued: "Queued"
        case .running: "Running"
        case .success: "Passed"
        case .failure: "Failed"
        case .cancelled: "Cancelled"
        case .skipped: "Skipped"
        case .waiting: "Waiting for approval"
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
    public var environment: String?
    public var approvedBy: String?
    public var attempt: UInt32
    public var startedAt: Int64?
    public var finishedAt: Int64?
    public var steps: [RunStep]
    public var frozenSnapshotPath: String?
    public var frozenSizeBytes: UInt64?
    public var frozenFailedStep: String?
    public var frozenExitCode: Int32?
}

public struct FrozenAttachment: Codable, Hashable, Sendable {
    public var job: Int64
    public var containerId: String
    public var containerName: String
    public var workdir: String
    public var failedStep: String?
    public var exitCode: Int32?
    public var cliCommand: String
    public var deepLink: String

    public init(job: Int64, containerId: String, containerName: String, workdir: String,
                failedStep: String? = nil, exitCode: Int32? = nil, cliCommand: String, deepLink: String) {
        self.job = job
        self.containerId = containerId
        self.containerName = containerName
        self.workdir = workdir
        self.failedStep = failedStep
        self.exitCode = exitCode
        self.cliCommand = cliCommand
        self.deepLink = deepLink
    }
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
    public var host: Bool?
    public var nexus: String?
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
    func attachFrozenJob(repo: String, job: Int64) async throws -> FrozenAttachment
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

    public func attachFrozenJob(repo: String, job: Int64) async throws -> FrozenAttachment {
        guard case let .frozenAttached(att) = try await call(.attachFrozenJob(repo: repo, job: job)) else {
            throw ServiceError.unexpectedResponse
        }
        return att
    }
}

// MARK: - Vault, environments, artifacts

public struct VaultScope: Codable, Hashable, Sendable {
    public var repo: String?
    public var environment: String?

    public init(repo: String? = nil, environment: String? = nil) {
        self.repo = repo
        self.environment = environment
    }

    public var label: String {
        switch (repo, environment) {
        case (nil, _): "Every repository"
        case let (r?, nil): r
        case let (r?, e?): "\(r) · \(e)"
        }
    }
}

public enum SecretKind: String, Codable, Sendable {
    case standard
    case jitToken = "jit_token"
}

public struct SecretInfo: Codable, Hashable, Identifiable, Sendable {
    public var scope: VaultScope
    public var name: String
    public var updatedAt: Int64
    public var kind: SecretKind
    public var ttlSeconds: UInt64?
    public var expiresAt: Int64?
    public var id: String { "\(scope.repo ?? "")/\(scope.environment ?? "")/\(name)" }

    public init(scope: VaultScope, name: String, updatedAt: Int64, kind: SecretKind = .standard, ttlSeconds: UInt64? = nil, expiresAt: Int64? = nil) {
        self.scope = scope
        self.name = name
        self.updatedAt = updatedAt
        self.kind = kind
        self.ttlSeconds = ttlSeconds
        self.expiresAt = expiresAt
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        scope = try c.decode(VaultScope.self, forKey: .scope)
        name = try c.decode(String.self, forKey: .name)
        updatedAt = try c.decode(Int64.self, forKey: .updatedAt)
        kind = try c.decodeIfPresent(SecretKind.self, forKey: .kind) ?? .standard
        ttlSeconds = try c.decodeIfPresent(UInt64.self, forKey: .ttlSeconds)
        expiresAt = try c.decodeIfPresent(Int64.self, forKey: .expiresAt)
    }
}

public struct SecretRevealed: Codable, Hashable, Sendable {
    public var scope: VaultScope
    public var name: String
    public var value: String
    public var ttlSeconds: UInt32
    public var expiresAt: Int64?
}

public enum ProvenanceKind: String, Codable, Sendable {
    case merge, ci, deploy
}

public struct ProvenanceRecord: Codable, Hashable, Identifiable, Sendable {
    public var id: Int64
    public var repo: String
    public var sha: String
    public var kind: ProvenanceKind
    public var statementJson: String
    public var serverKeyId: String
    public var serverPublicKey: String
    public var serverSignature: String
    public var deviceId: String?
    public var devicePublicKey: String?
    public var deviceSignature: String?
    public var deviceSignedAt: Int64?
    public var deviceNonce: String?
    public var devicePayload: String?
    public var gitRef: String?
    public var createdAt: Int64
}

public struct ProvenanceVerification: Codable, Hashable, Sendable {
    public var sha: String
    public var valid: Bool
    public var records: [ProvenanceRecord]
    public var verifiedCount: UInt32
    public var deviceSignedCount: UInt32
    public var issues: [String]
}

public enum ShadowState: String, Codable, Sendable {
    case disabled, idle, running, passed, failed
}

public struct ShadowStatus: Codable, Hashable, Sendable {
    public var repo: String
    public var enabled: Bool
    public var cmd: String
    public var useGraviton: Bool
    public var image: String?
    public var state: ShadowState
    public var exitCode: Int32?
    public var durationMs: UInt64?
    public var updatedAt: Int64?
    public var logTail: String

    public init(repo: String, enabled: Bool, cmd: String, useGraviton: Bool = false, image: String? = nil, state: ShadowState = .disabled, exitCode: Int32? = nil, durationMs: UInt64? = nil, updatedAt: Int64? = nil, logTail: String = "") {
        self.repo = repo
        self.enabled = enabled
        self.cmd = cmd
        self.useGraviton = useGraviton
        self.image = image
        self.state = state
        self.exitCode = exitCode
        self.durationMs = durationMs
        self.updatedAt = updatedAt
        self.logTail = logTail
    }
}

public struct Variable: Codable, Hashable, Identifiable, Sendable {
    public var scope: VaultScope
    public var name: String
    public var value: String
    public var updatedAt: Int64
    public var id: String { "\(scope.repo ?? "")/\(scope.environment ?? "")/\(name)" }
}

public struct DeployEnvironment: Codable, Hashable, Identifiable, Sendable {
    public var repo: String
    public var name: String
    public var requireApproval: Bool
    public var branches: [String]
    public var id: String { name }

    public init(repo: String, name: String, requireApproval: Bool, branches: [String] = []) {
        self.repo = repo
        self.name = name
        self.requireApproval = requireApproval
        self.branches = branches
    }
}

public struct Artifact: Codable, Hashable, Identifiable, Sendable {
    public var run: Int64
    public var job: String
    public var path: String
    public var size: UInt64
    public var id: String { "\(job)/\(path)" }
}

public struct ArtifactChunk: Codable, Hashable, Sendable {
    public var path: String
    public var offset: UInt64
    public var size: UInt64
    public var data: String
    public var done: Bool
}

public struct VaultInfo: Codable, Hashable, Sendable {
    public var setUp: Bool
    public var keyStore: String
}

public protocol VaultService: Sendable {
    func vaultInfo() async throws -> VaultInfo
    func secrets(scope: VaultScope) async throws -> [SecretInfo]
    func setSecret(scope: VaultScope, name: String, value: String, kind: SecretKind, ttlSeconds: UInt64?) async throws -> [SecretInfo]
    func revealSecret(scope: VaultScope, name: String, reason: String?) async throws -> SecretRevealed
    func provenance(repo: String, rev: String) async throws -> [ProvenanceRecord]
    func verifyProvenance(repo: String, rev: String) async throws -> ProvenanceVerification
    func deleteSecret(scope: VaultScope, name: String) async throws -> [SecretInfo]
    func variables(scope: VaultScope) async throws -> [Variable]
    func setVariable(scope: VaultScope, name: String, value: String) async throws -> [Variable]
    func deleteVariable(scope: VaultScope, name: String) async throws -> [Variable]
    func environments(repo: String) async throws -> [DeployEnvironment]
    func setEnvironment(_ env: DeployEnvironment) async throws -> [DeployEnvironment]
    func deleteEnvironment(repo: String, name: String) async throws -> [DeployEnvironment]
    func approve(repo: String, job: Int64, approve: Bool) async throws -> RunDetail
    func artifacts(repo: String, run: Int64) async throws -> [Artifact]
    /// The whole file, read in pieces.
    func download(repo: String, run: Int64, path: String) async throws -> Data
}

extension ServerService: VaultService {
    private func secretList(_ r: Request) async throws -> [SecretInfo] {
        guard case let .secrets(list) = try await call(r) else { throw ServiceError.unexpectedResponse }
        return list
    }

    private func variableList(_ r: Request) async throws -> [Variable] {
        guard case let .variables(list) = try await call(r) else { throw ServiceError.unexpectedResponse }
        return list
    }

    private func environmentList(_ r: Request) async throws -> [DeployEnvironment] {
        guard case let .environments(list) = try await call(r) else { throw ServiceError.unexpectedResponse }
        return list
    }

    public func vaultInfo() async throws -> VaultInfo {
        guard case let .vault(v) = try await call(.vaultInfo) else { throw ServiceError.unexpectedResponse }
        return v
    }

    public func secrets(scope: VaultScope) async throws -> [SecretInfo] { try await secretList(.listSecrets(scope: scope)) }
    public func setSecret(scope: VaultScope, name: String, value: String, kind: SecretKind = .standard, ttlSeconds: UInt64? = nil) async throws -> [SecretInfo] {
        try await secretList(.setSecret(scope: scope, name: name, value: value, kind: kind, ttlSeconds: ttlSeconds))
    }
    public func revealSecret(scope: VaultScope, name: String, reason: String? = nil) async throws -> SecretRevealed {
        guard case let .secretRevealed(r) = try await call(.revealSecret(scope: scope, name: name, reason: reason)) else {
            throw ServiceError.unexpectedResponse
        }
        return r
    }
    public func provenance(repo: String, rev: String) async throws -> [ProvenanceRecord] {
        guard case let .provenance(list) = try await call(.provenance(repo: repo, rev: rev)) else {
            throw ServiceError.unexpectedResponse
        }
        return list
    }
    public func verifyProvenance(repo: String, rev: String) async throws -> ProvenanceVerification {
        guard case let .provenanceVerified(v) = try await call(.verifyProvenance(repo: repo, rev: rev)) else {
            throw ServiceError.unexpectedResponse
        }
        return v
    }
    public func deleteSecret(scope: VaultScope, name: String) async throws -> [SecretInfo] { try await secretList(.deleteSecret(scope: scope, name: name)) }
    public func variables(scope: VaultScope) async throws -> [Variable] { try await variableList(.listVariables(scope: scope)) }
    public func setVariable(scope: VaultScope, name: String, value: String) async throws -> [Variable] {
        try await variableList(.setVariable(scope: scope, name: name, value: value))
    }
    public func deleteVariable(scope: VaultScope, name: String) async throws -> [Variable] { try await variableList(.deleteVariable(scope: scope, name: name)) }
    public func environments(repo: String) async throws -> [DeployEnvironment] { try await environmentList(.listEnvironments(repo: repo)) }
    public func setEnvironment(_ env: DeployEnvironment) async throws -> [DeployEnvironment] { try await environmentList(.setEnvironment(env)) }
    public func deleteEnvironment(repo: String, name: String) async throws -> [DeployEnvironment] {
        try await environmentList(.deleteEnvironment(repo: repo, name: name))
    }

    public func approve(repo: String, job: Int64, approve: Bool) async throws -> RunDetail {
        guard case let .run(d) = try await call(.approveJob(repo: repo, job: job, approve: approve)) else { throw ServiceError.unexpectedResponse }
        return d
    }

    public func artifacts(repo: String, run: Int64) async throws -> [Artifact] {
        guard case let .artifacts(list) = try await call(.listArtifacts(repo: repo, run: run)) else { throw ServiceError.unexpectedResponse }
        return list
    }

    public func download(repo: String, run: Int64, path: String) async throws -> Data {
        var out = Data()
        var offset: UInt64 = 0
        while true {
            guard case let .artifactChunk(chunk) = try await call(.artifactData(repo: repo, run: run, path: path, offset: offset)) else {
                throw ServiceError.unexpectedResponse
            }
            guard let bytes = Data(base64Encoded: chunk.data) else { throw ServiceError.io("the artifact came back unreadable") }
            out.append(bytes)
            offset += UInt64(bytes.count)
            if chunk.done || bytes.isEmpty { return out }
        }
    }
}
