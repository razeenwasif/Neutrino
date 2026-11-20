import NeutrinoKit
import Observation
import SwiftUI

/// The Code page: a repo's files at a branch or tag, like a forge's repo home.
/// Owned by AppModel, which supplies the service for the selected repo.
@MainActor @Observable
final class CodeModel {
    /// Branch, tag or sha being browsed; nil is HEAD (a hosted repo's default branch).
    var rev: String?
    /// Directory being listed, "" for the root.
    private(set) var path = ""
    /// File being shown instead of the listing.
    private(set) var file: String?
    var filter = ""

    private(set) var listing: TreeListing?
    private(set) var listingError: String?
    private(set) var loadingListing = false
    /// Newest commit touching each entry of the listed directory, by name.
    private(set) var entryCommits: [String: EntryCommit] = [:]
    private(set) var latest: LogEntry?
    private(set) var overview: RepoOverview?
    private(set) var readme: BlobContent?
    private(set) var blob: BlobContent?
    private(set) var blobError: String?
    private(set) var loadingBlob = false
    /// Markdown files show rendered unless the code is asked for.
    var showSource = false

    /// Which repo the state above belongs to.
    private var key = ""
    private var generation = 0

    var visibleEntries: [TreeEntry] {
        let all = listing?.entries ?? []
        let q = filter.trimmingCharacters(in: .whitespaces).lowercased()
        return q.isEmpty ? all : all.filter { $0.name.lowercased().contains(q) }
    }

    /// Path components with the path up to each, for the breadcrumb.
    var crumbs: [(name: String, path: String)] {
        let full = file ?? path
        var acc: [String] = []
        return full.split(separator: "/").map { part in
            acc.append(String(part))
            return (String(part), acc.joined(separator: "/"))
        }
    }

    // MARK: Loading

    /// Start over for another repo.
    func reset(for key: String) {
        guard key != self.key else { return }
        self.key = key
        rev = nil
        path = ""
        file = nil
        filter = ""
        listing = nil
        listingError = nil
        entryCommits = [:]
        latest = nil
        overview = nil
        readme = nil
        blob = nil
        blobError = nil
    }

    func load(_ service: any NeutrinoService, repo: String) async {
        guard let browse = service as? any BrowseService else {
            listingError = "This connection can't browse files."
            return
        }
        generation += 1
        let g = generation
        let rev = self.rev
        let path = self.path
        loadingListing = true
        defer { if g == generation { loadingListing = false } }

        async let listingResult = Result(catching: { try await browse.tree(repo: repo, rev: rev, path: path) })
        async let latestResult = Result(catching: { try await service.log(LogQuery(repo: repo, rev: rev, limit: 1)).commits.first })
        async let overviewResult = Result(catching: { try await browse.overview(repo: repo, rev: rev) })

        switch await listingResult {
        case let .success(l):
            guard g == generation else { return }
            listing = l
            listingError = nil
        case let .failure(e):
            guard g == generation else { return }
            listing = nil
            listingError = Self.explain(e)
        }
        latest = try? await latestResult.get()
        if g == generation { overview = try? await overviewResult.get() }

        // The README of the listed directory, shown under the files.
        if let entry = listing?.entries.first(where: { Self.isReadme($0) }) {
            let r = try? await browse.blob(repo: repo, rev: rev, path: entry.path)
            if g == generation { readme = r }
        } else {
            readme = nil
        }

        // Slowest last: which commit last touched each entry.
        let commits = (try? await browse.treeCommits(repo: repo, rev: rev, path: path)) ?? []
        guard g == generation else { return }
        entryCommits = Dictionary(commits.map { ($0.name, $0) }, uniquingKeysWith: { a, _ in a })

        if let file { await loadBlob(browse, repo: repo, path: file) }
    }

    private func loadBlob(_ browse: any BrowseService, repo: String, path: String) async {
        loadingBlob = true
        defer { loadingBlob = false }
        do {
            let b = try await browse.blob(repo: repo, rev: rev, path: path)
            guard file == path else { return }
            blob = b
            blobError = nil
        } catch {
            guard file == path else { return }
            blob = nil
            blobError = error.localizedDescription
        }
    }

    // MARK: Navigation

    /// Show a directory. The listing loads from the view's task.
    func open(dir: String) {
        file = nil
        blob = nil
        filter = ""
        if dir != path {
            path = dir
            listing = nil
            entryCommits = [:]
            readme = nil
        }
    }

    func open(file target: String, _ service: any NeutrinoService, repo: String) async {
        let parent = target.split(separator: "/").dropLast().joined(separator: "/")
        if parent != path { open(dir: parent) }
        file = target
        blob = nil
        blobError = nil
        showSource = false
        guard let browse = service as? any BrowseService else { return }
        await loadBlob(browse, repo: repo, path: target)
    }

    func open(_ entry: TreeEntry, _ service: any NeutrinoService, repo: String) async {
        switch entry.kind {
        case .dir: open(dir: entry.path)
        case .submodule: break
        default: await open(file: entry.path, service, repo: repo)
        }
    }

    /// Open a link from a README: a path in the repo opens here, anything else in the browser.
    func follow(_ url: URL, from readmePath: String, _ service: any NeutrinoService, repo: String) async -> Bool {
        guard url.scheme == nil, url.host() == nil else { return false }
        var target = url.path(percentEncoded: false)
        if target.isEmpty { return true }
        let base = readmePath.split(separator: "/").dropLast().map(String.init)
        var parts = target.hasPrefix("/") ? [] : base
        for p in target.split(separator: "/") {
            if p == ".." { _ = parts.popLast() } else if p != "." { parts.append(String(p)) }
        }
        target = parts.joined(separator: "/")
        if target.hasSuffix("/") || listing?.entries.first(where: { $0.path == target })?.kind == .dir {
            open(dir: target)
        } else {
            await open(file: target, service, repo: repo)
        }
        return true
    }

    func up() {
        if file != nil {
            open(dir: path)
        } else if !path.isEmpty {
            open(dir: path.split(separator: "/").dropLast().joined(separator: "/"))
        }
    }

    /// A daemon from before browsing rejects the request outright; say so plainly.
    nonisolated static func explain(_ error: any Error) -> String {
        let message = error.localizedDescription
        if message.contains("unknown variant `tree`") {
            return "This neutrinod is older than file browsing. Update it (protocol 8 or later) to see the files here."
        }
        return message
    }

    nonisolated static func isReadme(_ e: TreeEntry) -> Bool {
        e.kind == .file && e.name.lowercased().hasPrefix("readme")
    }

    nonisolated static func isMarkdown(_ path: String) -> Bool {
        let p = path.lowercased()
        return p.hasSuffix(".md") || p.hasSuffix(".markdown") || p.hasSuffix(".mdown")
    }
}

extension Result where Failure == any Error {
    init(catching body: () async throws -> Success) async {
        do { self = .success(try await body()) } catch { self = .failure(error) }
    }
}
