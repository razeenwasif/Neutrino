import LocalAuthentication
import NeutrinoKit
import SwiftUI

struct SecretRevealState: Identifiable, Equatable {
    var name: String
    var scopeLabel: String
    var value: String
    var expiresAt: Date
    var id: String { "\(scopeLabel)/\(name)" }
}

/// Secrets, variables and environments on the server. Secrets go in and never
/// come back out: the app only ever sees their names.
extension AppModel {
    var vaultService: (any VaultService)? { isServerOnline ? serverService : nil }

    /// The scope the screen shows.
    var vaultScope: VaultScope {
        // Without a hosted repo only every repo's scope means anything.
        guard pullRepo != nil else { return VaultScope() }
        return switch vaultView {
        case .repo: VaultScope(repo: pullRepo)
        case .everyRepo: VaultScope()
        case let .environment(e): VaultScope(repo: pullRepo, environment: e)
        }
    }

    func loadSecrets() async {
        guard let service = vaultService else {
            vaultInfo = nil
            pullRepoProblem = serverLink == nil ? "Secrets live on your server. Pair this Mac with it from the sidebar." : "\(serverLink?.name ?? "The server") is offline."
            return
        }
        let hosted = await hostedTarget()
        // Every repo's secrets need no hosted repo, so there's always something to show.
        if hosted == nil { pullRepoProblem = nil }
        do {
            vaultInfo = try await service.vaultInfo()
            environmentList = hosted == nil ? [] : try await service.environments(repo: hosted!)
            if case let .environment(e) = vaultView, !environmentList.contains(where: { $0.name == e }) { vaultView = .repo }
            secretList = try await service.secrets(scope: vaultScope)
            variableList = try await service.variables(scope: vaultScope)
            secretsError = nil
        } catch {
            secretsError = error.localizedDescription
        }
    }

    /// Only what was set at exactly this scope; wider ones are shown as inherited.
    func isOwn(_ scope: VaultScope) -> Bool { scope == vaultScope }

    private func vaultAction(_ body: (any VaultService) async throws -> Void) async {
        guard let service = vaultService else { return }
        do {
            try await body(service)
            secretsError = nil
        } catch {
            notice = error.localizedDescription
        }
        await loadSecrets()
    }

    func setSecret(name: String, value: String, kind: SecretKind = .standard, ttlSeconds: UInt64? = nil) async throws {
        guard let service = vaultService else { throw ServiceError.daemon("Connect to the server first.") }
        secretList = try await service.setSecret(scope: vaultScope, name: name, value: value, kind: kind, ttlSeconds: ttlSeconds)
    }

    func revealSecret(_ s: SecretInfo) async {
        guard let service = vaultService else {
            notice = "Connect to the server first."
            return
        }
        let ctx = LAContext()
        ctx.localizedCancelTitle = "Cancel"
        do {
            var authError: NSError?
            let ok: Bool
            if ctx.canEvaluatePolicy(.deviceOwnerAuthentication, error: &authError) {
                ok = try await ctx.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "Authenticate to reveal vault secret \(s.name)")
            } else {
                ok = true
            }
            guard ok else { return }
            let revealed = try await service.revealSecret(scope: s.scope, name: s.name, reason: "Touch ID break-glass reveal from NeutrinoMac")
            revealedSecret = SecretRevealState(
                name: revealed.name,
                scopeLabel: revealed.scope.label,
                value: revealed.value,
                expiresAt: Date.now.addingTimeInterval(TimeInterval(max(5, Int(revealed.ttlSeconds))))
            )
        } catch {
            notice = error.localizedDescription
        }
    }

    func deleteSecret(_ s: SecretInfo) async {
        await vaultAction { _ = try await $0.deleteSecret(scope: s.scope, name: s.name) }
    }

    func setVariable(name: String, value: String) async throws {
        guard let service = vaultService else { throw ServiceError.daemon("Connect to the server first.") }
        variableList = try await service.setVariable(scope: vaultScope, name: name, value: value)
    }

    func deleteVariable(_ v: Variable) async {
        await vaultAction { _ = try await $0.deleteVariable(scope: v.scope, name: v.name) }
    }

    func setEnvironment(_ e: DeployEnvironment) async throws {
        guard let service = vaultService else { throw ServiceError.daemon("Connect to the server first.") }
        environmentList = try await service.setEnvironment(e)
    }

    func deleteEnvironment(_ e: DeployEnvironment) async {
        if vaultView == .environment(e.name) { vaultView = .repo }
        await vaultAction { _ = try await $0.deleteEnvironment(repo: e.repo, name: e.name) }
    }

    // MARK: Runs: approvals and artifacts

    func decide(job: RunJob, approve: Bool) async {
        guard let service = vaultService, let hosted = pullRepo, runBusy == nil else { return }
        runBusy = approve ? "Approving" : "Turning down"
        defer { runBusy = nil }
        do {
            runDetail = try await service.approve(repo: hosted, job: job.id, approve: approve)
            await refreshRuns()
        } catch {
            notice = error.localizedDescription
        }
    }

    func loadArtifacts() async {
        guard let service = vaultService, let hosted = pullRepo, let run = runID else {
            artifactList = []
            return
        }
        artifactList = (try? await service.artifacts(repo: hosted, run: run)) ?? []
    }

    /// Ask where to save an artifact, then fetch it.
    func save(_ artifact: Artifact) async {
        guard let service = vaultService, let hosted = pullRepo else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = (artifact.path as NSString).lastPathComponent
        panel.message = "Save \(artifact.path) from run \(runDetail?.run.number ?? 0)"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        runBusy = "Downloading"
        defer { runBusy = nil }
        do {
            let data = try await service.download(repo: hosted, run: artifact.run, path: "\(artifact.job)/\(artifact.path)")
            try data.write(to: url)
        } catch {
            notice = error.localizedDescription
        }
    }
}
