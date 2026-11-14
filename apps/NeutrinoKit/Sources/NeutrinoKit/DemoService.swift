import Foundation

/// The prototype's sample repository, served in-process when neutrinod isn't
/// running (and for previews and tests).
public final class DemoService: NeutrinoService {
    public init() {}

    static let now = Int64(Date().timeIntervalSince1970)

    static let repo = Repo(id: "nexus", name: "nexus", path: "~/Github/Applications/nexus", head: .branch(name: "reservations", sha: "a41f9c2e0b1c8d94f27a6305bd1e9c47f8a2b6d0"))

    private struct Sample {
        var sha: String
        var subject: String
        var body: String
        var ago: Int64
        var parents: [String]
        var refs: [RefLabel]
        var graph: GraphRow
        var files: [(FileChange, [String])]
    }

    private static func sha(_ short: String) -> String {
        (short + "e0b1c8d94f27a6305bd1e9c47f8a2b6d0").prefix(40).description
    }

    private static func seg(_ from: UInt16, _ to: UInt16, _ color: UInt16) -> GraphSegment {
        GraphSegment(from: from, to: to, color: color)
    }

    private static let samples: [Sample] = {
        [
            Sample(sha: sha("a41f9c2"), subject: "Coordinator: accept reservation requests", body: "Wires the endpoint to the store so a scheduler outside Nexus can hold memory on a node.", ago: 12 * 60,
                   parents: [sha("7be03d1")], refs: [RefLabel(name: "reservations", kind: .local, isHead: true)],
                   graph: GraphRow(lane: 0, color: 0, upper: [], lower: [seg(0, 0, 0)]),
                   files: [(FileChange(path: "cluster/nexus_cluster/server/app.py", status: .modified, additions: 6, deletions: 0), [
                       "@@ -112,3 +112,9 @@", " async def list_nodes():", "     return [n.public() for n in state.nodes]", "+", "+",
                       "+@app.post(\"/v1/reservations\")", "+async def reserve(req: ReservationRequest):",
                       "+    \"\"\"Hold memory on a node for a scheduler outside Nexus.\"\"\"", "+    return reservations.hold(req.owner, req.node, req.memory_gb)",
                   ])]),
            Sample(sha: sha("7be03d1"), subject: "API: validate reservation owner and node", body: "", ago: 40 * 60,
                   parents: [sha("c9d2e55")], refs: [],
                   graph: GraphRow(lane: 0, color: 0, upper: [seg(0, 0, 0)], lower: [seg(0, 0, 0)]),
                   files: [(FileChange(path: "cluster/nexus_cluster/server/models.py", status: .modified, additions: 5, deletions: 1), [
                       "@@ -40,4 +40,8 @@ class ReservationRequest(BaseModel):", "     owner: str", "     node: str",
                       "-    memory_gb: float", "+    memory_gb: float = Field(gt=0, le=64)", "+", "+    @field_validator(\"node\")",
                       "+    def known_node(cls, v):", "+        return require_node(v)",
                   ])]),
            Sample(sha: sha("c9d2e55"), subject: "Add /v1/reservations endpoint", body: "First step towards Neutrino and Nexus sharing atlas: reservations are owner-only, like everything that runs code.", ago: 2 * 3600,
                   parents: [sha("5f0a7b8")], refs: [RefLabel(name: "origin/reservations", kind: .remote)],
                   graph: GraphRow(lane: 0, color: 0, upper: [seg(0, 0, 0)], lower: [seg(0, 0, 0)]),
                   files: [(FileChange(path: "docs/CONTROL_PLANE.md", status: .modified, additions: 2, deletions: 0), [
                       "@@ -31,3 +31,5 @@", " - **Owner only.** Anything that runs code on the machines is allowed only for owners.",
                       "+- **Reservations** (`POST /v1/reservations`) hold memory on a node for a scheduler", "+  outside Nexus. They expire unless renewed.", " ",
                   ])]),
            Sample(sha: sha("5f0a7b8"), subject: "Merge fleet-ram into main", body: "", ago: 26 * 3600,
                   parents: [sha("2d8e6f3"), sha("e1c4a90")], refs: [RefLabel(name: "main", kind: .local), RefLabel(name: "origin/main", kind: .remote)],
                   graph: GraphRow(lane: 0, color: 0, upper: [seg(0, 0, 0)], lower: [seg(0, 0, 0), seg(0, 1, 1)]),
                   files: [(FileChange(path: "src/pages/Fleet.tsx", status: .modified, additions: 1, deletions: 1), [
                       "@@ -64,5 +64,5 @@ export function NodeCard({ node }: Props) {", "       <Gauge value={node.gpu.used} max={node.gpu.total} />",
                       "-      <Ram used={node.ram.used} total={node.ram.total} />", "+      <MemoryBar used={node.ram.used} reserved={node.ram.reserved} total={node.ram.total} />", "     </Card>",
                   ])]),
            Sample(sha: sha("e1c4a90"), subject: "Fleet: show reserved RAM per node", body: "", ago: 27 * 3600,
                   parents: [sha("93b7d06")], refs: [],
                   graph: GraphRow(lane: 1, color: 1, upper: [seg(0, 0, 0), seg(1, 1, 1)], lower: [seg(0, 0, 0), seg(1, 1, 1)]),
                   files: [(FileChange(path: "src/pages/Fleet.tsx", status: .modified, additions: 2, deletions: 1), [
                       "@@ -31,5 +31,6 @@ function useNode(id: string) {", "   const ram = pod.cluster.ram_gb", "+  const reserved = pod.cluster.reserved_gb ?? 0",
                       "-  return { ...pod, ram: { total: ram } }", "+  return { ...pod, ram: { total: ram, reserved } }",
                   ])]),
            Sample(sha: sha("93b7d06"), subject: "Fleet: memory bar component", body: "", ago: 50 * 3600,
                   parents: [sha("b60c1ae")], refs: [],
                   graph: GraphRow(lane: 1, color: 1, upper: [seg(0, 0, 0), seg(1, 1, 1)], lower: [seg(0, 0, 0), seg(1, 1, 1)]),
                   files: [(FileChange(path: "src/components/MemoryBar.tsx", status: .added, additions: 3, deletions: 0), [
                       "@@ -0,0 +1,3 @@", "+export function MemoryBar({ used, reserved, total }: Props) {", "+  const pct = (n: number) => `${(n / total) * 100}%`", "+  return <Track used={pct(used)} reserved={pct(reserved)} />",
                   ])]),
            Sample(sha: sha("2d8e6f3"), subject: "Fix sweep trial ordering", body: "Trials ran in set order, so a resumed sweep could repeat one and skip another.", ago: 52 * 3600,
                   parents: [sha("b60c1ae")], refs: [],
                   graph: GraphRow(lane: 0, color: 0, upper: [seg(0, 0, 0), seg(1, 1, 1)], lower: [seg(0, 0, 0), seg(1, 1, 1)]),
                   files: [(FileChange(path: "cluster/nexus_cluster/server/sweeps.py", status: .modified, additions: 1, deletions: 1), [
                       "@@ -77,4 +77,4 @@ def trials(spec: SweepSpec) -> list[Trial]:", "     combos = itertools.product(*spec.parameters.values())",
                       "-    return [Trial(i, dict(zip(spec.parameters, c))) for i, c in enumerate(set(combos))]", "+    return [Trial(i, dict(zip(spec.parameters, c))) for i, c in enumerate(combos)]",
                   ])]),
            Sample(sha: sha("b60c1ae"), subject: "Bump llama.cpp", body: "", ago: 75 * 3600,
                   parents: [sha("418fd27")], refs: [RefLabel(name: "v0.9.2", kind: .tag)],
                   graph: GraphRow(lane: 0, color: 0, upper: [seg(0, 0, 0), seg(1, 0, 1)], lower: [seg(0, 0, 0)]),
                   files: [(FileChange(path: "cluster/scripts/llama.lock", status: .modified, additions: 1, deletions: 1), [
                       "@@ -1,2 +1,2 @@", "-llama.cpp = \"b4410\"", "+llama.cpp = \"b4500\"", " whisper.cpp = \"v1.7.4\"",
                   ])]),
            Sample(sha: sha("418fd27"), subject: "Desktop: sign the Windows installer", body: "", ago: 5 * 86400,
                   parents: [], refs: [],
                   graph: GraphRow(lane: 0, color: 0, upper: [seg(0, 0, 0)], lower: []),
                   files: [(FileChange(path: ".neutrino/workflows/desktop-windows.yml", status: .added, additions: 2, deletions: 0), [
                       "@@ -0,0 +1,2 @@", "+      - run: bun run desktop:win -- --sign", "+    artifacts: [release/*.exe]",
                   ])]),
        ]
    }()

    private static let ada = Signature(name: "Razeen Wasif", email: "you@example.com", time: 0)

    private static func entry(_ s: Sample) -> LogEntry {
        var author = ada
        author.time = now - s.ago
        return LogEntry(sha: s.sha, parents: s.parents, subject: s.subject, author: author, time: now - s.ago, refs: s.refs, graph: s.graph)
    }

    /// Turn `"+x"`, `"-x"`, `" x"` and `"@@ -a,b +c,d @@"` lines into hunks.
    static func hunks(_ src: [String]) -> [Hunk] {
        var hunks: [Hunk] = []
        var o: UInt32 = 0, n: UInt32 = 0
        for raw in src {
            if raw.hasPrefix("@@") {
                let nums = raw.split(whereSeparator: { !$0.isNumber }).compactMap { UInt32($0) }
                o = nums.count > 0 ? nums[0] : 1
                n = nums.count > 2 ? nums[2] : 1
                hunks.append(Hunk(oldStart: o, oldLines: nums.count > 1 ? nums[1] : 0, newStart: n, newLines: nums.count > 3 ? nums[3] : 0, lines: []))
                if o == 0 { o = 1 }
                if n == 0 { n = 1 }
                continue
            }
            let text = String(raw.dropFirst())
            let line: DiffLine
            switch raw.first {
            case "+": line = DiffLine(kind: .added, oldLine: nil, newLine: n, text: text); n += 1
            case "-": line = DiffLine(kind: .removed, oldLine: o, newLine: nil, text: text); o += 1
            default: line = DiffLine(kind: .context, oldLine: o, newLine: n, text: text); o += 1; n += 1
            }
            hunks[hunks.count - 1].lines.append(line)
        }
        // Word highlights for a removed line followed by its replacement.
        for h in hunks.indices {
            let lines = hunks[h].lines
            for i in lines.indices.dropLast() where lines[i].kind == .removed && lines[i + 1].kind == .added {
                let (a, b) = (lines[i].text, lines[i + 1].text)
                let prefix = a.commonPrefix(with: b).count
                let suffix = zip(a.reversed(), b.reversed()).prefix(while: { $0 == $1 }).count
                guard prefix + suffix < min(a.count, b.count) + 1, prefix > 0 else { continue }
                func split(_ s: String) -> [Segment] {
                    let start = s.index(s.startIndex, offsetBy: prefix)
                    let end = s.index(s.endIndex, offsetBy: -min(suffix, s.count - prefix))
                    return [Segment(text: String(s[..<start]), changed: false), Segment(text: String(s[start ..< end]), changed: true), Segment(text: String(s[end...]), changed: false)].filter { !$0.text.isEmpty }
                }
                hunks[h].lines[i].segments = split(a)
                hunks[h].lines[i + 1].segments = split(b)
            }
        }
        return hunks
    }

    public func ping() async throws -> UInt32 { protocolVersion }

    public func engineInfo() async throws -> EngineInfo {
        EngineInfo(version: "demo", protocol: protocolVersion, role: "demo", repos: 1)
    }

    public func repos() async throws -> [Repo] { [Self.repo] }

    public func addRepo(path _: String) async throws -> Repo {
        throw ServiceError.daemon("Start neutrinod to add your own repositories.")
    }

    public func removeRepo(id _: String) async throws {
        throw ServiceError.daemon("The demo repository can't be removed.")
    }

    public func branches(repo _: String) async throws -> [Branch] {
        let t = Self.now
        return [
            Branch(name: "reservations", kind: .local, sha: Self.sha("a41f9c2"), subject: "Coordinator: accept reservation requests", time: t - 720, isHead: true, upstream: "origin/reservations", ahead: 2, behind: 0),
            Branch(name: "main", kind: .local, sha: Self.sha("5f0a7b8"), subject: "Merge fleet-ram into main", time: t - 26 * 3600, upstream: "origin/main", ahead: 0, behind: 0),
            Branch(name: "fleet-ram", kind: .local, sha: Self.sha("e1c4a90"), subject: "Fleet: show reserved RAM per node", time: t - 27 * 3600, upstream: "origin/fleet-ram", ahead: 0, behind: 0),
            Branch(name: "win-installer-arm64", kind: .local, sha: Self.sha("6ab31f0"), subject: "Desktop: try an arm64 Windows build", time: t - 21 * 86400),
            Branch(name: "origin/reservations", kind: .remote, sha: Self.sha("c9d2e55"), subject: "Add /v1/reservations endpoint", time: t - 7200),
            Branch(name: "origin/main", kind: .remote, sha: Self.sha("5f0a7b8"), subject: "Merge fleet-ram into main", time: t - 26 * 3600),
        ]
    }

    public func log(_ q: LogQuery) async throws -> LogPage {
        var list = Self.samples.map(Self.entry)
        if let needle = q.query?.lowercased(), !needle.isEmpty {
            list = list.filter { $0.subject.lowercased().contains(needle) || $0.sha.hasPrefix(needle) }.map { var e = $0; e.graph = nil; return e }
        }
        let page = Array(list.dropFirst(Int(q.skip)).prefix(Int(q.limit ?? 100)))
        return LogPage(commits: page, hasMore: false, lanes: 2)
    }

    public func commit(repo _: String, sha: String) async throws -> CommitDetail {
        guard let s = Self.samples.first(where: { $0.sha.hasPrefix(sha) }) else { throw ServiceError.daemon("no commit named \(sha)") }
        var author = Self.ada
        author.time = Self.now - s.ago
        return CommitDetail(sha: s.sha, parents: s.parents, author: author, committer: author, subject: s.subject, body: s.body, refs: s.refs, files: s.files.map(\.0))
    }

    public func fileDiff(repo _: String, sha: String, path: String, context _: UInt32?) async throws -> FileDiff {
        guard let s = Self.samples.first(where: { $0.sha.hasPrefix(sha) }), let f = s.files.first(where: { $0.0.path == path }) else {
            throw ServiceError.daemon("\(path) did not change in \(sha.prefix(7))")
        }
        return FileDiff(path: path, status: f.0.status, additions: f.0.additions, deletions: f.0.deletions, hunks: Self.hunks(f.1))
    }
}
