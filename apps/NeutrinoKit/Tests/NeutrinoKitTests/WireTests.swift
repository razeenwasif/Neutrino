import Foundation
@testable import NeutrinoKit
import Testing

// The JSON here is what neutrinod sends; crates/neutrino-proto pins the same shapes.

private func json(_ data: Data) throws -> NSDictionary {
    try JSONSerialization.jsonObject(with: data.dropLast()) as! NSDictionary
}

@Test func requestLines() throws {
    let ping = try Wire.line(id: 1, .ping)
    #expect(ping.last == 0x0A)
    #expect(try json(ping) == ["id": 1, "method": "ping"] as NSDictionary)

    let log = try json(Wire.line(id: 7, .log(LogQuery(repo: "nexus", limit: 50))))
    #expect(log == ["id": 7, "method": "log", "params": ["repo": "nexus", "all": false, "skip": 0, "limit": 50]] as NSDictionary)

    let diff = try json(Wire.line(id: 3, .fileDiff(repo: "x", sha: "abc", path: "a/b.rs", context: nil)))
    #expect(diff == ["id": 3, "method": "file_diff", "params": ["repo": "x", "sha": "abc", "path": "a/b.rs"]] as NSDictionary)

    let add = try json(Wire.line(id: 4, .addRepo(path: "/Users/me/repo")))
    #expect(add == ["id": 4, "method": "add_repo", "params": ["path": "/Users/me/repo"]] as NSDictionary)
}

@Test func responses() throws {
    #expect(try Wire.decode(Data(#"{"id":2,"type":"pong","data":{"protocol":1}}"#.utf8)) == (2, .pong(protocol: 1)))
    #expect(try Wire.decode(Data(#"{"id":5,"type":"cancelled"}"#.utf8)) == (5, .cancelled))
    #expect(try Wire.decode(Data(#"{"id":6,"type":"error","data":{"message":"no repo"}}"#.utf8)) == (6, .error(message: "no repo")))
    #expect(throws: WireError.unknownType("what")) { try Wire.decode(Data(#"{"id":1,"type":"what"}"#.utf8)) }
}

@Test func repos() throws {
    let line = #"{"id":1,"type":"repos","data":[{"id":"nexus","name":"nexus","path":"/p","bare":false,"head":{"kind":"branch","name":"main","sha":"abc"},"error":null},{"id":"gone","name":"gone","path":"/g","bare":false,"head":null,"error":"/g no longer exists"},{"id":"d","name":"d","path":"/d","bare":true,"head":{"kind":"detached","sha":"def1234567"}}]}"#
    guard case let .repos(list) = try Wire.decode(Data(line.utf8)).1 else { Issue.record("not repos"); return }
    #expect(list[0].head == .branch(name: "main", sha: "abc"))
    #expect(list[1].head == nil && list[1].error == "/g no longer exists")
    #expect(list[2].head?.label == "def1234")
    #expect(list[2].bare)
}

@Test func logPage() throws {
    let line = #"""
    {"id":9,"type":"log","data":{"commits":[{"sha":"aaa","parents":["bbb","ccc"],"subject":"Merge","author":{"name":"Ada","email":"a@x","time":1700000000,"offset":3600},"time":1700000060,"refs":[{"name":"main","kind":"local","is_head":true},{"name":"v1","kind":"tag","is_head":false}],"graph":{"lane":0,"color":0,"upper":[],"lower":[{"from":0,"to":0,"color":0},{"from":0,"to":1,"color":1}]}}],"has_more":true,"lanes":2}}
    """#
    guard case let .log(page) = try Wire.decode(Data(line.utf8)).1 else { Issue.record("not a log"); return }
    #expect(page.hasMore && page.lanes == 2)
    let c = page.commits[0]
    #expect(c.isMerge && c.shortSHA == "aaa")
    #expect(c.refs[0] == RefLabel(name: "main", kind: .local, isHead: true))
    #expect(c.graph?.lower.last == GraphSegment(from: 0, to: 1, color: 1))
    #expect(c.author.offset == 3600)
}

@Test func commitAndDiff() throws {
    let commit = #"{"id":1,"type":"commit","data":{"sha":"s","parents":[],"author":{"name":"A","email":"e","time":1,"offset":0},"committer":{"name":"A","email":"e","time":1,"offset":0},"subject":"S","body":"","refs":[],"files":[{"path":"b","old_path":"a","status":"renamed","additions":1,"deletions":2,"binary":false},{"path":"l","old_path":null,"status":"type_changed","additions":0,"deletions":0,"binary":false}],"truncated":false}}"#
    guard case let .commit(d) = try Wire.decode(Data(commit.utf8)).1 else { Issue.record("not a commit"); return }
    #expect(d.files[0].oldPath == "a" && d.files[0].status == .renamed)
    #expect(d.files[1].status == .typeChanged)

    let diff = #"{"id":1,"type":"file_diff","data":{"path":"a","old_path":null,"status":"modified","binary":false,"too_large":false,"additions":1,"deletions":1,"hunks":[{"old_start":1,"old_lines":2,"new_start":1,"new_lines":2,"lines":[{"kind":"context","old_line":1,"new_line":1,"text":"x"},{"kind":"removed","old_line":2,"new_line":null,"text":"old","segments":[{"text":"old","changed":true}],"no_newline":true},{"kind":"added","old_line":null,"new_line":2,"text":"new"}]}]}}"#
    guard case let .fileDiff(f) = try Wire.decode(Data(diff.utf8)).1 else { Issue.record("not a diff"); return }
    let lines = f.hunks[0].lines
    #expect(f.hunks[0].header == "@@ -1,2 +1,2 @@")
    #expect(lines[0].segments.isEmpty && !lines[0].noNewline)
    #expect(lines[1].noNewline && lines[1].segments == [Segment(text: "old", changed: true)])
    #expect(lines[2].kind == .added && lines[2].newLine == 2)
}

@Test func demoServiceAnswers() async throws {
    let demo = DemoService()
    let page = try await demo.log(LogQuery(repo: "nexus"))
    #expect(page.commits.count == 9)
    let merge = try #require(page.commits.first(where: { $0.isMerge }))
    let detail = try await demo.commit(repo: "nexus", sha: merge.shortSHA)
    let diff = try await demo.fileDiff(repo: "nexus", sha: merge.sha, path: detail.files[0].path, context: nil)
    let removed = try #require(diff.hunks[0].lines.first(where: { $0.kind == .removed }))
    #expect(removed.segments.contains(where: { $0.changed }))
    #expect(try await demo.log(LogQuery(repo: "nexus", query: "sweep")).commits.count == 1)
}

@Test func relativeTimes() {
    let now = Date(timeIntervalSince1970: 1_700_000_000)
    #expect(Format.ago(1_700_000_000 - 30, now: now) == "just now")
    #expect(Format.ago(1_700_000_000 - 720, now: now) == "12 min ago")
    #expect(Format.ago(1_700_000_000 - 3 * 3600, now: now) == "3 h ago")
    #expect(Format.ago(1_700_000_000 - 30 * 3600, now: now) == "yesterday")
    #expect(Format.ago(1_700_000_000 - 5 * 86400, now: now) == "5 d ago")
}

/// Drives a running neutrinod when NEUTRINO_LIVE=1 (with NEUTRINO_SOCKET pointing at it).
@Test(.enabled(if: ProcessInfo.processInfo.environment["NEUTRINO_LIVE"] == "1"))
func liveDaemon() async throws {
    let s = SocketService()
    #expect(try await s.ping() == protocolVersion)
    let repos = try await s.repos()
    let repo = try #require(repos.first(where: { $0.error == nil }))
    let page = try await s.log(LogQuery(repo: repo.id, limit: 5))
    let first = try #require(page.commits.first)
    let detail = try await s.commit(repo: repo.id, sha: first.sha)
    if let file = detail.files.first(where: { !$0.binary }) {
        let diff = try await s.fileDiff(repo: repo.id, sha: first.sha, path: file.path, context: 3)
        #expect(diff.additions == file.additions && diff.deletions == file.deletions)
    }
    await #expect(throws: ServiceError.self) { try await s.branches(repo: "no-such-repo") }
}
