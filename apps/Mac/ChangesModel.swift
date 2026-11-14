import NeutrinoKit
import SwiftUI

/// The working copy: status, picking lines, writes, and refreshing when files change.
extension AppModel {
    // MARK: Live updates

    /// Follow change events for as long as this connection lasts.
    func listen() {
        eventsTask?.cancel()
        let stream = service.events()
        eventsTask = Task { [weak self] in
            for await event in stream {
                guard let self, event.repo == self.repoID else { continue }
                self.scheduleRefresh(refs: event.kind == "refs")
            }
        }
    }

    /// Coalesce bursts of events into one refresh.
    func scheduleRefresh(refs: Bool) {
        let alsoRefs = refs
        refreshTask?.cancel()
        refreshTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(120))
            guard let self, !Task.isCancelled else { return }
            await self.loadStatus()
            if alsoRefs {
                await self.loadBranches()
                await self.refreshRepoHead()
                if self.section == .history { await self.reloadHistory() }
            }
        }
    }

    func refreshRepoHead() async {
        guard source == .local, let repoID, let updated = try? await service.repos().first(where: { $0.id == repoID }) else { return }
        // Replace the one entry so the header shows the new branch.
        await loadReposKeepingSelection(updated)
    }

    // MARK: Status

    var stagedPaths: Set<String> { Set(status?.staged.map(\.path) ?? []) }

    /// Unstaged changes, conflicts and untracked files: what "Not staged" lists.
    var notStaged: [StatusFile] {
        guard let s = status else { return [] }
        return s.unstaged + s.untracked
    }

    func loadStatus() async {
        guard let repoID, source == .local, repo?.bare == false else {
            status = nil
            return
        }
        statusGeneration += 1
        let generation = statusGeneration
        do {
            let s = try await service.status(repo: repoID)
            guard generation == statusGeneration else { return }
            status = s
            statusError = nil
        } catch {
            guard generation == statusGeneration else { return }
            status = nil
            statusError = error.localizedDescription
            return
        }
        // Keep the shown file if it's still there, else show the first.
        if let c = change, exists(c) {
            await loadWorkDiff(keepPicks: true)
        } else {
            let s = status!
            let first = (s.conflicted + s.unstaged + s.untracked).first.map { ChangeRef(path: $0.path, area: .unstaged) }
                ?? s.staged.first.map { ChangeRef(path: $0.path, area: .staged) }
            await select(change: first)
        }
    }

    private func exists(_ c: ChangeRef) -> Bool {
        guard let s = status else { return false }
        switch c.area {
        case .staged: return s.staged.contains { $0.path == c.path }
        case .unstaged: return (s.unstaged + s.untracked + s.conflicted).contains { $0.path == c.path }
        }
    }

    func select(change c: ChangeRef?) async {
        if c != change { picks = [] }
        change = c
        if c == nil {
            workDiff = nil
            workDiffError = nil
            return
        }
        await loadWorkDiff(keepPicks: false)
    }

    func loadWorkDiff(keepPicks: Bool) async {
        guard let repoID, let c = change else { return }
        workDiffGeneration += 1
        let generation = workDiffGeneration
        do {
            let d = try await service.workDiff(repo: repoID, path: c.path, area: c.area)
            guard generation == workDiffGeneration else { return }
            workDiff = d
            workDiffError = nil
            // Picks that no longer point at a changed line are dropped.
            let present = Set(d.hunks.flatMap(\.lines).compactMap(LinePick.init))
            picks = keepPicks ? picks.intersection(present) : []
        } catch {
            guard generation == workDiffGeneration else { return }
            workDiff = nil
            workDiffError = error.localizedDescription
        }
    }

    // MARK: Writes

    /// Run a write, then refresh. Errors show in the header.
    private func perform(_ action: @escaping () async throws -> Void) async {
        guard !writing else { return }
        writing = true
        defer { writing = false }
        do {
            try await action()
        } catch {
            notice = error.localizedDescription
        }
        await loadStatus()
    }

    func toggle(_ file: StatusFile, staged: Bool) async {
        guard let repoID else { return }
        let service = self.service
        await perform {
            if staged {
                try await service.unstage(repo: repoID, paths: [file.path])
            } else {
                try await service.stage(repo: repoID, paths: [file.path])
            }
        }
        // Follow the file to the side it moved to.
        if change?.path == file.path {
            await select(change: ChangeRef(path: file.path, area: staged ? .unstaged : .staged))
        }
    }

    func stageAll() async {
        guard let repoID else { return }
        let paths = notStaged.map(\.path) + (status?.conflicted.map(\.path) ?? [])
        guard !paths.isEmpty else { return }
        let service = self.service
        await perform { try await service.stage(repo: repoID, paths: paths) }
    }

    func unstageAll() async {
        guard let repoID, let paths = status?.staged.map(\.path), !paths.isEmpty else { return }
        let service = self.service
        await perform { try await service.unstage(repo: repoID, paths: paths) }
    }

    func togglePick(_ line: DiffLine) {
        guard let pick = LinePick(line) else { return }
        if picks.contains(pick) { picks.remove(pick) } else { picks.insert(pick) }
    }

    /// Stage or unstage every changed line of one hunk.
    func applyHunk(_ hunk: Hunk) async {
        guard let repoID, let c = change else { return }
        let lines = hunk.lines.compactMap(LinePick.init)
        let service = self.service
        await perform { try await service.stageLines(repo: repoID, path: c.path, area: c.area, lines: lines) }
    }

    /// Stage the picked lines (or unstage them, in the staged view).
    func applyPicks() async {
        guard let repoID, let c = change, !picks.isEmpty else { return }
        let lines = Array(picks)
        let service = self.service
        await perform { try await service.stageLines(repo: repoID, path: c.path, area: c.area, lines: lines) }
        picks = []
    }

    func discardPicks() async {
        guard let repoID, let c = change, c.area == .unstaged, !picks.isEmpty else { return }
        let lines = Array(picks)
        let service = self.service
        await perform { try await service.discardLines(repo: repoID, path: c.path, lines: lines) }
        picks = []
    }

    func discard(paths: [String]) async {
        guard let repoID, !paths.isEmpty else { return }
        let service = self.service
        await perform { try await service.discard(repo: repoID, paths: paths) }
    }

    var canCommit: Bool {
        let hasMessage = !commitSummary.trimmingCharacters(in: .whitespaces).isEmpty
        return !committing && (amend || (hasMessage && !(status?.staged.isEmpty ?? true)))
    }

    var commitMessage: String {
        let body = commitBody.trimmingCharacters(in: .whitespacesAndNewlines)
        let summary = commitSummary.trimmingCharacters(in: .whitespacesAndNewlines)
        return body.isEmpty ? summary : "\(summary)\n\n\(body)"
    }

    func commit() async {
        guard let repoID, canCommit else { return }
        committing = true
        defer { committing = false }
        do {
            let result = try await service.commit(repo: repoID, message: commitMessage, amend: amend)
            lastCommit = result
            commitSummary = ""
            commitBody = ""
            amend = false
        } catch {
            notice = error.localizedDescription
        }
        await loadStatus()
        await loadBranches()
        await refreshRepoHead()
        if section == .history { await reloadHistory() }
    }

    /// Fill the message from the last commit, to amend it.
    func startAmend() {
        amend = true
        if commitSummary.isEmpty, let c = commits.first(where: { $0.sha == repo?.head?.sha }) ?? commits.first {
            commitSummary = c.subject
        }
    }

    // MARK: Branches

    private func branchWrite(_ action: @escaping () async throws -> Void) async -> Bool {
        var ok = true
        writing = true
        do {
            try await action()
        } catch {
            notice = error.localizedDescription
            ok = false
        }
        writing = false
        await loadBranches()
        await refreshRepoHead()
        await loadStatus()
        if section == .history { await reloadHistory() }
        return ok
    }

    func checkout(_ branch: Branch) async {
        guard let repoID else { return }
        let service = self.service
        _ = await branchWrite { try await service.checkout(repo: repoID, branch: branch.name) }
    }

    func createBranch(name: String, start: String?, checkout: Bool) async -> Bool {
        guard let repoID else { return false }
        let service = self.service
        return await branchWrite { try await service.createBranch(repo: repoID, name: name, start: start, checkout: checkout) }
    }

    func renameBranch(from: String, to: String) async -> Bool {
        guard let repoID else { return false }
        let service = self.service
        let ok = await branchWrite { try await service.renameBranch(repo: repoID, from: from, to: to) }
        if ok { selectedBranch = "local:\(to)" }
        return ok
    }

    /// Returns git's refusal instead of showing it, so an unmerged branch can be offered a forced delete.
    func deleteBranch(_ name: String, force: Bool) async -> String? {
        guard let repoID else { return nil }
        writing = true
        defer { writing = false }
        do {
            try await service.deleteBranch(repo: repoID, name: name, force: force)
        } catch {
            return error.localizedDescription
        }
        await loadBranches()
        return nil
    }
}
