import Foundation

// Browsing a repo's files at a commit, for the Code page. Mirrors the tree,
// tree_commits, blob and overview requests in crates/neutrino-proto.

public enum EntryKind: String, Codable, Sendable {
    case dir, file, executable, symlink, submodule

    /// Directories and submodules list before files.
    public var isFolder: Bool { self == .dir || self == .submodule }
}

public struct TreeEntry: Codable, Hashable, Identifiable, Sendable {
    public var name: String
    /// Repo-relative path.
    public var path: String
    public var kind: EntryKind
    /// Bytes; nil for directories and submodules.
    public var size: UInt64?

    public var id: String { path }

    public init(name: String, path: String, kind: EntryKind, size: UInt64? = nil) {
        self.name = name
        self.path = path
        self.kind = kind
        self.size = size
    }
}

public struct TreeListing: Codable, Hashable, Sendable {
    /// The commit the listing was read from.
    public var commit: String
    public var path: String
    public var entries: [TreeEntry]

    public init(commit: String, path: String, entries: [TreeEntry]) {
        self.commit = commit
        self.path = path
        self.entries = entries
    }
}

/// The newest commit that changed one entry of a directory.
public struct EntryCommit: Codable, Hashable, Sendable {
    public var name: String
    public var sha: String
    public var subject: String
    public var time: Int64

    public init(name: String, sha: String, subject: String, time: Int64) {
        self.name = name
        self.sha = sha
        self.subject = subject
        self.time = time
    }
}

public struct BlobContent: Codable, Hashable, Sendable {
    public var commit: String
    public var path: String
    public var size: UInt64
    public var binary: Bool
    public var tooLarge: Bool
    public var text: String?
    public var lines: UInt32

    public init(commit: String, path: String, size: UInt64, binary: Bool = false, tooLarge: Bool = false, text: String?, lines: UInt32) {
        self.commit = commit
        self.path = path
        self.size = size
        self.binary = binary
        self.tooLarge = tooLarge
        self.text = text
        self.lines = lines
    }
}

public struct LanguageShare: Codable, Hashable, Sendable {
    public var name: String
    public var bytes: UInt64

    public init(name: String, bytes: UInt64) {
        self.name = name
        self.bytes = bytes
    }
}

public struct RepoOverview: Codable, Hashable, Sendable {
    public var commit: String
    public var commits: UInt32
    public var commitsCapped: Bool
    public var contributors: UInt32
    public var files: UInt32
    public var languages: [LanguageShare]
    public var readme: String?
    public var license: String?

    public init(commit: String, commits: UInt32, commitsCapped: Bool = false, contributors: UInt32, files: UInt32, languages: [LanguageShare], readme: String?, license: String?) {
        self.commit = commit
        self.commits = commits
        self.commitsCapped = commitsCapped
        self.contributors = contributors
        self.files = files
        self.languages = languages
        self.readme = readme
        self.license = license
    }
}

/// Reading files at a commit. neutrinod answers for this Mac's repos, the
/// server for hosted ones; `rev` nil means HEAD.
public protocol BrowseService: Sendable {
    func tree(repo: String, rev: String?, path: String) async throws -> TreeListing
    func treeCommits(repo: String, rev: String?, path: String) async throws -> [EntryCommit]
    func blob(repo: String, rev: String?, path: String) async throws -> BlobContent
    func overview(repo: String, rev: String?) async throws -> RepoOverview
}

/// The four calls over any connection that speaks the control API.
protocol BrowseCalls: BrowseService {
    func send(_ request: Request) async throws -> Response
}

extension BrowseCalls {
    public func tree(repo: String, rev: String?, path: String) async throws -> TreeListing {
        guard case let .tree(t) = try await send(.tree(repo: repo, rev: rev, path: path)) else { throw ServiceError.unexpectedResponse }
        return t
    }

    public func treeCommits(repo: String, rev: String?, path: String) async throws -> [EntryCommit] {
        guard case let .treeCommits(c) = try await send(.treeCommits(repo: repo, rev: rev, path: path)) else { throw ServiceError.unexpectedResponse }
        return c
    }

    public func blob(repo: String, rev: String?, path: String) async throws -> BlobContent {
        guard case let .blob(b) = try await send(.blob(repo: repo, rev: rev, path: path)) else { throw ServiceError.unexpectedResponse }
        return b
    }

    public func overview(repo: String, rev: String?) async throws -> RepoOverview {
        guard case let .overview(o) = try await send(.overview(repo: repo, rev: rev)) else { throw ServiceError.unexpectedResponse }
        return o
    }
}

extension SocketService: BrowseCalls {
    func send(_ request: Request) async throws -> Response { try await call(request) }
}

extension ServerService: BrowseCalls {
    func send(_ request: Request) async throws -> Response { try await call(request) }
}

// MARK: - Demo

extension DemoService: BrowseService {
    private static let readme = """
    # nexus

    Run work on the machines at home from anywhere: a **coordinator** that knows every node, an agent on each, and a desktop app to watch it all.

    ## Layout

    - `cluster/` — the coordinator and node agent (Python, FastAPI)
    - `src/` — the desktop app (React, Tauri)
    - `docs/` — how the control plane works

    ## Running it

    ```sh
    uv run nexus-coordinator
    npm run tauri dev
    ```

    > Reservations are owner-only, like everything that runs code on the machines.
    """

    private static let files: [String: String] = [
        "README.md": readme,
        "LICENSE": "MIT License\n\nCopyright (c) 2026\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software.\n",
        "pyproject.toml": "[project]\nname = \"nexus-cluster\"\nversion = \"0.4.0\"\nrequires-python = \">=3.12\"\ndependencies = [\"fastapi\", \"uvicorn\", \"pydantic>=2\"]\n",
        "package.json": "{\n  \"name\": \"nexus\",\n  \"private\": true,\n  \"scripts\": { \"tauri\": \"tauri\" }\n}\n",
        "cluster/nexus_cluster/server/app.py": "from fastapi import FastAPI\n\nfrom .state import state\nfrom . import reservations\n\napp = FastAPI(title=\"Nexus coordinator\")\n\n\n@app.get(\"/v1/nodes\")\nasync def list_nodes():\n    return [n.public() for n in state.nodes]\n\n\n@app.post(\"/v1/reservations\")\nasync def reserve(req: ReservationRequest):\n    \"\"\"Hold memory on a node for a scheduler outside Nexus.\"\"\"\n    return reservations.hold(req.owner, req.node, req.memory_gb)\n",
        "docs/CONTROL_PLANE.md": "# Control plane\n\n- **Owner only.** Anything that runs code on the machines is allowed only for owners.\n- **Reservations** (`POST /v1/reservations`) hold memory on a node for a scheduler\n  outside Nexus. They expire unless renewed.\n",
        "src/pages/Fleet.tsx": "export function NodeCard({ node }: Props) {\n  return (\n    <Card>\n      <Gauge value={node.gpu.used} max={node.gpu.total} />\n    </Card>\n  )\n}\n",
    ]

    private static let dirs: [String: [TreeEntry]] = [
        "": [
            TreeEntry(name: "cluster", path: "cluster", kind: .dir),
            TreeEntry(name: "docs", path: "docs", kind: .dir),
            TreeEntry(name: "src", path: "src", kind: .dir),
            TreeEntry(name: "LICENSE", path: "LICENSE", kind: .file, size: 1068),
            TreeEntry(name: "package.json", path: "package.json", kind: .file, size: 812),
            TreeEntry(name: "pyproject.toml", path: "pyproject.toml", kind: .file, size: 1240),
            TreeEntry(name: "README.md", path: "README.md", kind: .file, size: 2381),
        ],
        "cluster": [TreeEntry(name: "nexus_cluster", path: "cluster/nexus_cluster", kind: .dir)],
        "cluster/nexus_cluster": [TreeEntry(name: "server", path: "cluster/nexus_cluster/server", kind: .dir)],
        "cluster/nexus_cluster/server": [TreeEntry(name: "app.py", path: "cluster/nexus_cluster/server/app.py", kind: .file, size: 4410)],
        "docs": [TreeEntry(name: "CONTROL_PLANE.md", path: "docs/CONTROL_PLANE.md", kind: .file, size: 5120)],
        "src": [TreeEntry(name: "pages", path: "src/pages", kind: .dir)],
        "src/pages": [TreeEntry(name: "Fleet.tsx", path: "src/pages/Fleet.tsx", kind: .file, size: 3020)],
    ]

    private static var head: String { repo.head?.sha ?? "" }

    public func tree(repo _: String, rev _: String?, path: String) async throws -> TreeListing {
        guard let entries = Self.dirs[path] else { throw ServiceError.daemon("\(path) isn't a directory") }
        return TreeListing(commit: Self.head, path: path, entries: entries)
    }

    public func treeCommits(repo _: String, rev _: String?, path: String) async throws -> [EntryCommit] {
        let picks: [(String, String, Int64)] = [
            ("a41f9c2e0b1c8d94f27a6305bd1e9c47f8a2b6d0", "Coordinator: accept reservation requests", 12 * 60),
            ("7be03d1e0b1c8d94f27a6305bd1e9c47f8a2b6d0", "API: validate reservation owner and node", 40 * 60),
            ("c9d2e55e0b1c8d94f27a6305bd1e9c47f8a2b6d0", "Add /v1/reservations endpoint", 2 * 3600),
            ("5f0a7b8e0b1c8d94f27a6305bd1e9c47f8a2b6d0", "Merge fleet-ram into main", 26 * 3600),
        ]
        return (Self.dirs[path] ?? []).enumerated().map { i, e in
            let p = picks[(i * 3 + e.name.count) % picks.count]
            return EntryCommit(name: e.name, sha: p.0, subject: p.1, time: Self.now - p.2 - Int64(i) * 86400)
        }
    }

    public func blob(repo _: String, rev _: String?, path: String) async throws -> BlobContent {
        guard let text = Self.files[path] else { throw ServiceError.daemon("\(path) isn't a file") }
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false).count - (text.hasSuffix("\n") ? 1 : 0)
        return BlobContent(commit: Self.head, path: path, size: UInt64(text.utf8.count), text: text, lines: UInt32(lines))
    }

    public func overview(repo _: String, rev _: String?) async throws -> RepoOverview {
        RepoOverview(commit: Self.head, commits: 412, contributors: 3, files: 186,
                     languages: [LanguageShare(name: "Python", bytes: 182_000), LanguageShare(name: "TypeScript", bytes: 141_000),
                                 LanguageShare(name: "Rust", bytes: 38_000), LanguageShare(name: "CSS", bytes: 9_400), LanguageShare(name: "Shell", bytes: 2_100)],
                     readme: "README.md", license: "LICENSE")
    }
}
