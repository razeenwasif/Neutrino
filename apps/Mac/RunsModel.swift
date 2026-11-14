import NeutrinoKit
import SwiftUI

/// CI runs live on the server, in the same hosted repo as the pull requests.
extension AppModel {
    var runService: (any RunService)? { isServerOnline ? serverService : nil }

    var selectedJob: RunJob? { runDetail?.jobs.first { $0.id == runJobID } }

    /// Find the hosted repo for the selection; says why not when there isn't one.
    func hostedTarget() async -> String? {
        guard pullService != nil else {
            pullRepo = nil
            pullRepoProblem = serverLink == nil
                ? "CI runs on your server. Pair this Mac with it from the sidebar."
                : "\(serverLink?.name ?? "The server") is offline."
            return nil
        }
        guard let hosted = await resolvePullRepo() else {
            pullRepo = nil
            pullRepoProblem = "\(repo?.name ?? "This repository") isn't on \(serverLink?.name ?? "the server") yet. Publish it from the sidebar to use its pull requests, runs and secrets."
            return nil
        }
        if pullRepo != hosted {
            runID = nil
            runDetail = nil
            runJobID = nil
            pullNumber = nil
            pullDetail = nil
        }
        pullRepo = hosted
        pullRepoProblem = nil
        return hosted
    }

    func loadRuns() async {
        guard let hosted = await hostedTarget(), let service = runService else {
            runList = []
            runDetail = nil
            return
        }
        do {
            runList = try await service.runs(repo: hosted, pull: nil)
            runError = nil
        } catch {
            runList = []
            runError = error.localizedDescription
            return
        }
        ciInfo = try? await service.ciInfo()
        if let id = runID, runList.contains(where: { $0.id == id }) {
            await select(run: id, keepJob: true)
        } else if let first = runList.first {
            await select(run: first.id)
        } else {
            runID = nil
            runDetail = nil
            runJobID = nil
        }
    }

    /// A run changed on the server: refresh the list and what's shown, keeping the log.
    func refreshRuns() async {
        guard let hosted = pullRepo, let service = runService else { return }
        if let list = try? await service.runs(repo: hosted, pull: nil) { runList = list }
        if let id = runID, let d = try? await service.run(repo: hosted, id: id) {
            runDetail = d
            if let i = runList.firstIndex(where: { $0.id == id }) { runList[i] = d.run }
        }
        ciInfo = try? await service.ciInfo()
    }

    func select(run id: Int64, keepJob: Bool = false) async {
        guard let service = runService, let hosted = pullRepo else { return }
        let changed = runID != id
        runID = id
        runGeneration += 1
        let generation = runGeneration
        do {
            let d = try await service.run(repo: hosted, id: id)
            guard generation == runGeneration else { return }
            runDetail = d
            runError = nil
            if changed || !keepJob || !d.jobs.contains(where: { $0.id == runJobID }) {
                // The first job that failed, else the first that ran.
                let pick = d.jobs.first { $0.state == .failure } ?? d.jobs.first { $0.state == .running } ?? d.jobs.first
                select(job: pick?.id)
            }
        } catch {
            guard generation == runGeneration else { return }
            runDetail = nil
            runError = error.localizedDescription
        }
    }

    /// Show a job's log, and keep reading it while the job runs.
    func select(job id: Int64?) {
        runsTask?.cancel()
        runJobID = id
        jobLogText = ""
        jobLogNext = 0
        jobLogDone = false
        logGeneration += 1
        let generation = logGeneration
        guard let id, let hosted = pullRepo, let service = runService else { return }
        runsTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, generation == self.logGeneration else { return }
                do {
                    let chunk = try await service.jobLog(repo: hosted, job: id, offset: self.jobLogNext)
                    guard generation == self.logGeneration else { return }
                    if !chunk.text.isEmpty { self.jobLogText += chunk.text }
                    let grew = chunk.next > self.jobLogNext
                    self.jobLogNext = chunk.next
                    if chunk.done {
                        self.jobLogDone = true
                        // The job finished: pick up its final state.
                        if self.runDetail?.jobs.first(where: { $0.id == id })?.state.finished == false { await self.refreshRuns() }
                        return
                    }
                    if grew { continue }
                } catch {
                    return
                }
                try? await Task.sleep(for: .seconds(1))
                // Steps move on without a log line; keep their states fresh while it runs.
                await self.refreshRuns()
            }
        }
    }

    /// The part of the log that belongs to a step.
    func logText(of step: RunStep) -> String {
        let bytes = Array(jobLogText.utf8)
        let start = Int(min(step.logStart, UInt64(bytes.count)))
        let end = Int(min(max(step.logEnd, step.logStart), UInt64(bytes.count)))
        return String(decoding: bytes[start ..< end], as: UTF8.self)
    }

    private func runAction(_ what: String, _ body: (any RunService, String) async throws -> RunDetail) async {
        guard let service = runService, let hosted = pullRepo, runBusy == nil else { return }
        runBusy = what
        defer { runBusy = nil }
        do {
            let d = try await body(service, hosted)
            if let list = try? await service.runs(repo: hosted, pull: nil) { runList = list }
            await select(run: d.run.id)
        } catch {
            notice = error.localizedDescription
        }
    }

    func cancelRun() async {
        guard let id = runID else { return }
        await runAction("Cancelling") { s, repo in try await s.cancelRun(repo: repo, id: id) }
    }

    func rerun() async {
        guard let id = runID else { return }
        await runAction("Starting") { s, repo in try await s.rerun(repo: repo, id: id) }
    }

    func dispatch(workflow: String, branch: String) async throws {
        guard let service = runService, let hosted = pullRepo else { throw ServiceError.daemon("Connect to the server first.") }
        let d = try await service.dispatch(repo: hosted, workflow: workflow, branch: branch)
        if let list = try? await service.runs(repo: hosted, pull: nil) { runList = list }
        await select(run: d.run.id)
    }

    func workflowFiles(branch: String) async -> [WorkflowFile] {
        guard let service = runService, let hosted = pullRepo else { return [] }
        return (try? await service.workflows(repo: hosted, branch: branch)) ?? []
    }

    /// Open a check from a pull request.
    func show(check run: Run) async {
        section = .runs
        await loadRuns()
        await select(run: run.id)
    }
}
