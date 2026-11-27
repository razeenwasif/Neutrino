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

@Test func workingCopyRequests() throws {
    let lines = try json(Wire.line(id: 3, .stageLines(repo: "r", path: "a.rs", area: .unstaged, lines: [LinePick(kind: .added, line: 4)])))
    #expect(lines == ["id": 3, "method": "stage_lines", "params": ["repo": "r", "path": "a.rs", "area": "unstaged", "lines": [["kind": "added", "line": 4]]]] as NSDictionary)
    let discard = try json(Wire.line(id: 4, .discardLines(repo: "r", path: "a.rs", lines: [LinePick(kind: .removed, line: 2)])))
    #expect(discard == ["id": 4, "method": "discard_lines", "params": ["repo": "r", "path": "a.rs", "lines": [["kind": "removed", "line": 2]]]] as NSDictionary)
    let commit = try json(Wire.line(id: 5, .commitChanges(repo: "r", message: "Fix", amend: false)))
    #expect(commit == ["id": 5, "method": "commit_changes", "params": ["repo": "r", "message": "Fix", "amend": false]] as NSDictionary)
    #expect(try json(Wire.line(id: 6, .subscribe)) == ["id": 6, "method": "subscribe"] as NSDictionary)
    let branch = try json(Wire.line(id: 7, .createBranch(repo: "r", name: "x", start: nil, checkout: true)))
    #expect(branch == ["id": 7, "method": "create_branch", "params": ["repo": "r", "name": "x", "checkout": true]] as NSDictionary)
}

@Test func workingCopyResponses() throws {
    let status = #"{"id":2,"type":"status","data":{"head":{"kind":"branch","name":"main","sha":null},"upstream":"origin/main","ahead":1,"behind":0,"operation":"merge","staged":[{"path":"b","old_path":"a","status":"renamed","additions":0,"deletions":0,"binary":false}],"unstaged":[],"untracked":[{"path":"n.txt","old_path":null,"status":"untracked","additions":2,"deletions":0,"binary":false}],"conflicted":[{"path":"c","old_path":null,"status":"conflicted","additions":0,"deletions":0,"binary":false}]}}"#
    guard case let .status(st) = try Wire.decode(Data(status.utf8)).1 else { Issue.record("not a status"); return }
    #expect(st.operation == "merge" && st.ahead == 1)
    #expect(st.staged[0].oldPath == "a" && st.untracked[0].status == .untracked && st.conflicted[0].status == .conflicted)
    #expect(st.changeCount == 3 && !st.isClean)
    #expect(try Wire.decode(Data(#"{"id":3,"type":"done","data":{"output":""}}"#.utf8)) == (3, .done(output: "")))
    #expect(try Wire.decode(Data(#"{"id":4,"type":"committed","data":{"sha":"abc","output":"hook ran"}}"#.utf8)) == (4, .committed(sha: "abc", output: "hook ran")))
    #expect(try Wire.decode(Data(#"{"id":9,"type":"event","data":{"repo":"r","kind":"worktree"}}"#.utf8)) == (9, .event(RepoEvent(repo: "r", kind: "worktree"))))
    let added = DiffLine(kind: .added, oldLine: nil, newLine: 7, text: "x")
    #expect(LinePick(added) == LinePick(kind: .added, line: 7))
    #expect(LinePick(DiffLine(kind: .context, oldLine: 1, newLine: 1, text: "")) == nil)
}

@Test func demoWorkingCopy() async throws {
    let demo = DemoService()
    let st = try await demo.status(repo: "nexus")
    #expect(st.staged.count == 2 && st.unstaged.count == 2 && st.untracked.count == 1)
    let d = try await demo.workDiff(repo: "nexus", path: st.unstaged[0].path, area: .unstaged)
    #expect(!d.hunks.isEmpty)
}

/// Stages a line, commits and watches events against a running neutrinod on a
/// scratch repo, when NEUTRINO_LIVE_REPO names a registered repo you don't mind changing.
@Test(.enabled(if: ProcessInfo.processInfo.environment["NEUTRINO_LIVE_REPO"] != nil))
func liveWorkingCopy() async throws {
    let repo = ProcessInfo.processInfo.environment["NEUTRINO_LIVE_REPO"]!
    let s = SocketService()
    let st = try await s.status(repo: repo)
    let file = try #require(st.unstaged.first)
    let diff = try await s.workDiff(repo: repo, path: file.path, area: .unstaged)
    let pick = try #require(diff.hunks.first?.lines.compactMap(LinePick.init).first)
    let events = s.events()
    try await s.stageLines(repo: repo, path: file.path, area: .unstaged, lines: [pick])
    var it = events.makeAsyncIterator()
    let event = await it.next()
    #expect(event?.repo == repo)
    let after = try await s.status(repo: repo)
    #expect(after.staged.contains(where: { $0.path == file.path }))
}

@Test func serverWire() throws {
    let push = try json(Wire.line(id: 1, .push(repo: "r", remote: "server", branch: nil, setUpstream: true)))
    #expect(push == ["id": 1, "method": "push", "params": ["repo": "r", "remote": "server", "set_upstream": true]] as NSDictionary)
    #expect(try json(Wire.line(id: 0, .auth(token: "t"))) == ["id": 0, "method": "auth", "params": ["token": "t"]] as NSDictionary)
    let host = try json(Wire.line(id: 2, .createHosted(name: "app", description: nil)))
    #expect(host == ["id": 2, "method": "create_hosted", "params": ["name": "app"]] as NSDictionary)
    #expect(try Wire.decode(Data(#"{"id":0,"type":"paired","data":{"token":"tok","device_id":"d1","host":"server"}}"#.utf8)) == (0, .paired(token: "tok", deviceID: "d1", host: "server")))
    #expect(try Wire.decode(Data(#"{"id":0,"type":"authenticated","data":{"host":"server","role":"server"}}"#.utf8)) == (0, .authenticated(host: "server", role: "server")))
    #expect(try Wire.decode(Data(#"{"id":3,"type":"remotes","data":[{"name":"server","url":"ssh://build-server/x.git"}]}"#.utf8)) == (3, .remotes([RemoteInfo(name: "server", url: "ssh://build-server/x.git")])))
}

@Test func pushURLs() {
    let link = ServerLink(deviceID: "d", name: "server", serviceName: nil, address: "10.0.0.2", port: 7844, fingerprint: String(repeating: "ab", count: 32), sshHost: "build-server")
    #expect(link.pushURL(for: "/Users/me/.neutrino/repos/app.git") == "ssh://build-server/Users/me/.neutrino/repos/app.git")
    var local = link
    local.sshHost = " "
    #expect(local.pushURL(for: "/srv/app.git") == "/srv/app.git")
    #expect(link.shortFingerprint == "ABAB ABAB ABAB ABAB")
}

/// Pairs with a server on this Mac when NEUTRINO_LIVE_SERVER_PORT and
/// NEUTRINO_LIVE_PAIR_CODE are set (a code from `ntr pair` on that server).
@Test(.enabled(if: ProcessInfo.processInfo.environment["NEUTRINO_LIVE_PAIR_CODE"] != nil))
func liveServer() async throws {
    let env = ProcessInfo.processInfo.environment
    let target = ServerTarget(name: "test", address: "127.0.0.1", port: UInt16(env["NEUTRINO_LIVE_SERVER_PORT"]!)!)
    let fp = try await Pairing.fingerprint(of: target)
    #expect(fp.count == 64)
    // A wrong pin is refused before anything is sent.
    await #expect(throws: ServiceError.self) {
        _ = try await Pairing.pair(target, fingerprint: String(repeating: "0", count: 64), code: env["NEUTRINO_LIVE_PAIR_CODE"]!, device: "test", sshHost: "")
    }
    let link = try await Pairing.pair(target, fingerprint: fp, code: env["NEUTRINO_LIVE_PAIR_CODE"]!, device: "NeutrinoKit test", sshHost: "")
    defer { ServerStore.forget() }
    let server = ServerService(link: link)
    #expect(try await server.engineInfo().role == "server")
    let repos = try await server.repos()
    #expect(repos.allSatisfy { $0.bare })
    await #expect(throws: ServiceError.self) { _ = try await server.status(repo: "x") }
    if let repo = repos.first, repo.head?.sha != nil {
        let page = try await server.log(LogQuery(repo: repo.id, limit: 5))
        #expect(!page.commits.isEmpty)
    }
}

// The same line is decoded by crates/neutrino-proto's `pull_detail_wire` test.
let pullLine = #"{"id":9,"type":"pull","data":{"pull":{"repo":"app","number":3,"title":"Shout","body":"Louder.","source":"feature","target":"main","state":"open","head_sha":"bbb","versions":2,"open_notes":1,"created_at":10,"updated_at":20,"merged_at":null,"merge_sha":null},"versions":[{"version":1,"head_sha":"aaa","base_sha":"000","created_at":10},{"version":2,"head_sha":"bbb","base_sha":"000","created_at":20}],"notes":[{"id":5,"version":1,"path":"app.txt","side":"new","line":3,"text":"Keep three","created_at":11,"resolved":false,"current_line":4},{"id":6,"version":1,"path":"app.txt","side":"old","line":1,"text":"Gone","created_at":12,"resolved":true,"current_line":null}],"mergeability":{"clean":true,"conflicts":[],"fast_forward":false,"behind":1},"files":[{"path":"app.txt","old_path":null,"status":"modified","additions":2,"deletions":1,"binary":false}],"commits":[{"sha":"bbb","parents":["aaa"],"subject":"start at zero","author":{"name":"Ada","email":"ada@example.com","time":20,"offset":0},"time":20,"refs":[],"graph":null}],"rule":{"pattern":"main","require_pull":true,"block_force":true,"block_delete":false}}}"#

@Test func pullWire() throws {
    guard case let .pull(d) = try Wire.decode(Data(pullLine.utf8)).1 else { Issue.record("not a pull"); return }
    #expect(d.pull.number == 3 && d.pull.state == .open && d.pull.openNotes == 1 && d.pull.mergeSha == nil)
    #expect(d.versions.map(\.version) == [1, 2])
    #expect(d.notes[0].currentLine == 4 && d.notes[0].side == .new && !d.notes[0].outdated)
    #expect(d.notes[1].outdated && d.notes[1].side == .old && d.notes[1].resolved)
    #expect(d.mergeability == Mergeability(clean: true, fastForward: false, behind: 1))
    #expect(d.rule?.requiredChecks == [] && d.rule?.blockDelete == false)
    #expect(d.rule?.summary == "pull requests only, no force-push")
    #expect(d.commits.first?.subject == "start at zero")

    let merge = try json(Wire.line(id: 2, .mergePull(repo: "app", number: 3, strategy: .fastForward, message: nil, deleteSource: true)))
    #expect(merge == ["id": 2, "method": "merge_pull", "params": ["repo": "app", "number": 3, "strategy": "fast_forward", "delete_source": true]] as NSDictionary)
    let note = try json(Wire.line(id: 3, .addNote(repo: "app", number: 3, path: "a.rs", side: .old, line: 7, text: "Why?")))
    #expect(note == ["id": 3, "method": "add_note", "params": ["repo": "app", "number": 3, "path": "a.rs", "side": "old", "line": 7, "text": "Why?"]] as NSDictionary)
    let rule = try json(Wire.line(id: 4, .setBranchRule(repo: "app", rule: BranchRule(pattern: "release/*", blockDelete: false))))
    #expect(rule == ["id": 4, "method": "set_branch_rule", "params": ["repo": "app", "rule": ["pattern": "release/*", "require_pull": true, "block_force": true, "block_delete": false, "required_checks": []]]] as NSDictionary)
    let close = try json(Wire.line(id: 5, .updatePull(repo: "app", number: 3, title: nil, body: nil, state: .closed)))
    #expect(close == ["id": 5, "method": "update_pull", "params": ["repo": "app", "number": 3, "state": "closed"]] as NSDictionary)

    #expect(try Wire.decode(Data(#"{"id":1,"type":"merged","data":{"sha":"ccc"}}"#.utf8)) == (1, .merged(sha: "ccc")))
    guard case let .overrides(o) = try Wire.decode(Data(#"{"id":1,"type":"overrides","data":[{"ref_name":"refs/heads/main","rule":"require_pull","reason":"hotfix","at":5}]}"#.utf8)).1 else {
        Issue.record("not overrides"); return
    }
    #expect(o.first?.reason == "hotfix")
}

// The same line is decoded by crates/neutrino-proto's `run_detail_wire` test.
let runLine = #"{"id":4,"type":"run","data":{"run":{"id":7,"repo":"app","number":3,"workflow":"ci.yml","name":"ci","event":"pull_request","branch":"feature","sha":"abcdef1234","pull":2,"state":"failure","error":null,"created_at":100,"started_at":101,"finished_at":130},"jobs":[{"id":11,"run":7,"key":"test-2","name":"test (2)","executor":"graviton","image":"alpine:3.20","state":"failure","error":null,"needs":["build"],"attempt":1,"started_at":110,"finished_at":125,"steps":[{"index":0,"name":"echo test","state":"failure","exit_code":1,"log_start":40,"log_end":52,"started_at":111,"finished_at":112}]}],"warnings":["jobs.deploy.services is ignored for now"]}}"#

@Test func runWire() throws {
    guard case let .run(d) = try Wire.decode(Data(runLine.utf8)).1 else { Issue.record("not a run"); return }
    #expect(d.run.number == 3 && d.run.state == .failure && d.run.pull == 2 && d.run.duration() == 29)
    #expect(d.run.eventLabel == "Pull request")
    #expect(d.jobs[0].steps[0].exitCode == 1 && d.jobs[0].steps[0].logStart == 40)
    #expect(d.warnings.count == 1)
    let list = try json(Wire.line(id: 2, .listRuns(repo: "app", pull: 2, limit: nil)))
    #expect(list == ["id": 2, "method": "list_runs", "params": ["repo": "app", "pull": 2]] as NSDictionary)
    let log = try json(Wire.line(id: 3, .jobLog(repo: "app", job: 11, offset: 40)))
    #expect(log == ["id": 3, "method": "job_log", "params": ["repo": "app", "job": 11, "offset": 40]] as NSDictionary)
    #expect(try json(Wire.line(id: 1, .ciInfo)) == ["id": 1, "method": "ci_info"] as NSDictionary)
    guard case let .jobLog(chunk) = try Wire.decode(Data(#"{"id":3,"type":"job_log","data":{"job":11,"offset":40,"text":"hi\n","next":43,"done":true}}"#.utf8)).1 else {
        Issue.record("not a log"); return
    }
    #expect(chunk.next == 43 && chunk.done)
    let ok = Run(id: 1, repo: "app", number: 1, workflow: "ci.yml", name: "ci", event: "push", branch: "main", sha: "a", pull: nil, state: .success, createdAt: 0)
    #expect(missingChecks(["ci", "lint"], in: [ok]) == ["lint"])
    #expect(missingChecks(["ci.yml"], in: [ok]).isEmpty)
}

@Test func vaultWire() throws {
    let set = try json(Wire.line(id: 1, .setSecret(scope: VaultScope(repo: "app", environment: "prod"), name: "TOKEN", value: "v")))
    #expect(set == ["id": 1, "method": "set_secret", "params": ["scope": ["repo": "app", "environment": "prod"], "name": "TOKEN", "value": "v"]] as NSDictionary)
    let global = try json(Wire.line(id: 2, .listSecrets(scope: VaultScope())))
    #expect(global == ["id": 2, "method": "list_secrets", "params": ["scope": [:]]] as NSDictionary)
    let env = try json(Wire.line(id: 3, .setEnvironment(DeployEnvironment(repo: "app", name: "prod", requireApproval: true, branches: ["main"]))))
    #expect(env == ["id": 3, "method": "set_environment", "params": ["environment": ["repo": "app", "name": "prod", "require_approval": true, "branches": ["main"]]]] as NSDictionary)
    let approve = try json(Wire.line(id: 4, .approveJob(repo: "app", job: 9, approve: false)))
    #expect(approve == ["id": 4, "method": "approve_job", "params": ["repo": "app", "job": 9, "approve": false]] as NSDictionary)
    guard case let .secrets(list) = try Wire.decode(Data(#"{"id":1,"type":"secrets","data":[{"scope":{"repo":"app","environment":null},"name":"TOKEN","updated_at":5}]}"#.utf8)).1 else {
        Issue.record("not secrets"); return
    }
    #expect(list[0].scope == VaultScope(repo: "app") && list[0].scope.label == "app")
    guard case let .run(d) = try Wire.decode(Data(#"{"id":1,"type":"run","data":{"run":{"id":1,"repo":"app","number":1,"workflow":"ci.yml","name":"ci","event":"push","branch":"main","sha":"a","pull":null,"state":"waiting","error":null,"created_at":0,"started_at":null,"finished_at":null},"jobs":[{"id":2,"run":1,"key":"deploy","name":"deploy","executor":"graviton","image":"x","state":"waiting","error":null,"needs":[],"environment":"prod","approved_by":null,"attempt":1,"started_at":null,"finished_at":null,"steps":[]}],"warnings":[]}}"#.utf8)).1 else {
        Issue.record("not a run"); return
    }
    #expect(d.run.state == .waiting && !d.run.state.finished && d.jobs[0].environment == "prod")
}

@Test func browseWire() throws {
    let tree = try json(Wire.line(id: 1, .tree(repo: "x", rev: nil, path: "src")))
    #expect(tree == ["id": 1, "method": "tree", "params": ["repo": "x", "path": "src"]] as NSDictionary)
    let commits = try json(Wire.line(id: 2, .treeCommits(repo: "x", rev: "main", path: "")))
    #expect(commits == ["id": 2, "method": "tree_commits", "params": ["repo": "x", "rev": "main", "path": ""]] as NSDictionary)
    let blob = try json(Wire.line(id: 3, .blob(repo: "x", rev: "v1", path: "README.md")))
    #expect(blob == ["id": 3, "method": "blob", "params": ["repo": "x", "rev": "v1", "path": "README.md"]] as NSDictionary)
    let overview = try json(Wire.line(id: 4, .overview(repo: "x", rev: nil)))
    #expect(overview == ["id": 4, "method": "overview", "params": ["repo": "x"]] as NSDictionary)

    let listing = #"{"id":1,"type":"tree","data":{"commit":"abc","path":"","entries":[{"name":"src","path":"src","kind":"dir","size":null},{"name":"run.sh","path":"run.sh","kind":"executable","size":42},{"name":"vendor","path":"vendor","kind":"submodule"}]}}"#
    guard case let .tree(t) = try Wire.decode(Data(listing.utf8)).1 else { Issue.record("not a tree"); return }
    #expect(t.entries.map(\.kind) == [.dir, .executable, .submodule])
    #expect(t.entries[1].size == 42 && t.entries[0].size == nil)
    #expect(t.entries[2].kind.isFolder)

    let changed = #"{"id":2,"type":"tree_commits","data":[{"name":"src","sha":"def","subject":"Fix it","time":1700000000}]}"#
    #expect(try Wire.decode(Data(changed.utf8)).1 == .treeCommits([EntryCommit(name: "src", sha: "def", subject: "Fix it", time: 1_700_000_000)]))

    let file = #"{"id":3,"type":"blob","data":{"commit":"abc","path":"a.png","size":2048,"binary":true,"too_large":false,"text":null,"lines":0}}"#
    guard case let .blob(b) = try Wire.decode(Data(file.utf8)).1 else { Issue.record("not a blob"); return }
    #expect(b.binary && !b.tooLarge && b.text == nil && b.size == 2048)

    let summary = #"{"id":4,"type":"overview","data":{"commit":"abc","commits":120,"commits_capped":false,"contributors":2,"files":40,"languages":[{"name":"Rust","bytes":9000},{"name":"Swift","bytes":3000}],"readme":"README.md","license":null}}"#
    guard case let .overview(o) = try Wire.decode(Data(summary.utf8)).1 else { Issue.record("not an overview"); return }
    #expect(o.commits == 120 && !o.commitsCapped && o.languages.first?.name == "Rust" && o.readme == "README.md" && o.license == nil)
}

@Test func demoBrowse() async throws {
    let demo = DemoService()
    let root = try await demo.tree(repo: "nexus", rev: nil, path: "")
    #expect(root.entries.first?.kind == .dir)
    let readme = try await demo.blob(repo: "nexus", rev: nil, path: "README.md")
    #expect(readme.text?.hasPrefix("# nexus") == true && readme.lines > 5)
    let overview = try await demo.overview(repo: "nexus", rev: nil)
    #expect(overview.readme == "README.md")
}
