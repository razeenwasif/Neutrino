import Foundation

public enum ServiceError: Error, LocalizedError, Equatable {
    case unreachable(String)
    case pathTooLong
    case io(String)
    case daemon(String)
    case unexpectedResponse

    public var errorDescription: String? {
        switch self {
        case let .unreachable(why): "Can't reach neutrinod: \(why)"
        case .pathTooLong: "The neutrinod socket path is too long (Unix sockets allow 104 bytes)."
        case let .io(why): "Connection error: \(why)"
        case let .daemon(message): message
        case .unexpectedResponse: "neutrinod sent an unexpected response."
        }
    }
}

public enum Paths {
    /// `$NEUTRINO_SOCKET`, else `~/.neutrino/neutrinod.sock` (as neutrinod picks it).
    public static var socket: String {
        let env = ProcessInfo.processInfo.environment
        if let s = env["NEUTRINO_SOCKET"], !s.isEmpty { return s }
        let home = env["NEUTRINO_HOME"].flatMap { $0.isEmpty ? nil : $0 } ?? (NSHomeDirectory() + "/.neutrino")
        return home + "/neutrinod.sock"
    }
}

/// Everything the app asks of a daemon. `SocketService` talks to a real
/// neutrinod; `DemoService` serves the prototype's sample data in-process.
public protocol NeutrinoService: Sendable {
    func ping() async throws -> UInt32
    func engineInfo() async throws -> EngineInfo
    func repos() async throws -> [Repo]
    func addRepo(path: String) async throws -> Repo
    func removeRepo(id: String) async throws
    func branches(repo: String) async throws -> [Branch]
    func log(_ query: LogQuery) async throws -> LogPage
    func commit(repo: String, sha: String) async throws -> CommitDetail
    func fileDiff(repo: String, sha: String, path: String, context: UInt32?) async throws -> FileDiff

    // Working copy
    func status(repo: String) async throws -> WorkingStatus
    func workDiff(repo: String, path: String, area: Area) async throws -> FileDiff
    func stage(repo: String, paths: [String]) async throws
    func unstage(repo: String, paths: [String]) async throws
    func stageLines(repo: String, path: String, area: Area, lines: [LinePick]) async throws
    func discard(repo: String, paths: [String]) async throws
    func discardLines(repo: String, path: String, lines: [LinePick]) async throws
    /// Returns the new commit and what git and its hooks printed.
    func commit(repo: String, message: String, amend: Bool) async throws -> (sha: String, output: String)
    func createBranch(repo: String, name: String, start: String?, checkout: Bool) async throws
    func checkout(repo: String, branch: String) async throws
    func renameBranch(repo: String, from: String, to: String) async throws
    func deleteBranch(repo: String, name: String, force: Bool) async throws
    /// Changes to any registered repo, until the stream is dropped.
    func events() -> AsyncStream<RepoEvent>
}

// MARK: - Socket service

/// Talks to neutrinod over its Unix socket, one short-lived connection per call.
public final class SocketService: NeutrinoService {
    public let path: String

    public init(path: String = Paths.socket) {
        self.path = path
    }

    public func ping() async throws -> UInt32 {
        guard case let .pong(v) = try await call(.ping, timeout: 3) else { throw ServiceError.unexpectedResponse }
        return v
    }

    public func engineInfo() async throws -> EngineInfo {
        guard case let .engine(e) = try await call(.engineInfo) else { throw ServiceError.unexpectedResponse }
        return e
    }

    public func repos() async throws -> [Repo] {
        guard case let .repos(r) = try await call(.listRepos) else { throw ServiceError.unexpectedResponse }
        return r
    }

    public func addRepo(path: String) async throws -> Repo {
        guard case let .repo(r) = try await call(.addRepo(path: path)) else { throw ServiceError.unexpectedResponse }
        return r
    }

    public func removeRepo(id: String) async throws {
        guard case .removed = try await call(.removeRepo(repo: id)) else { throw ServiceError.unexpectedResponse }
    }

    public func branches(repo: String) async throws -> [Branch] {
        guard case let .branches(b) = try await call(.branches(repo: repo)) else { throw ServiceError.unexpectedResponse }
        return b
    }

    public func log(_ query: LogQuery) async throws -> LogPage {
        guard case let .log(p) = try await call(.log(query)) else { throw ServiceError.unexpectedResponse }
        return p
    }

    public func commit(repo: String, sha: String) async throws -> CommitDetail {
        guard case let .commit(d) = try await call(.commit(repo: repo, sha: sha)) else { throw ServiceError.unexpectedResponse }
        return d
    }

    public func fileDiff(repo: String, sha: String, path: String, context: UInt32?) async throws -> FileDiff {
        guard case let .fileDiff(d) = try await call(.fileDiff(repo: repo, sha: sha, path: path, context: context)) else { throw ServiceError.unexpectedResponse }
        return d
    }

    public func status(repo: String) async throws -> WorkingStatus {
        guard case let .status(s) = try await call(.status(repo: repo)) else { throw ServiceError.unexpectedResponse }
        return s
    }

    public func workDiff(repo: String, path: String, area: Area) async throws -> FileDiff {
        guard case let .fileDiff(d) = try await call(.workDiff(repo: repo, path: path, area: area, context: 3)) else { throw ServiceError.unexpectedResponse }
        return d
    }

    private func perform(_ r: Request) async throws {
        guard case .done = try await call(r, timeout: 300) else { throw ServiceError.unexpectedResponse }
    }

    public func stage(repo: String, paths: [String]) async throws { try await perform(.stage(repo: repo, paths: paths)) }
    public func unstage(repo: String, paths: [String]) async throws { try await perform(.unstage(repo: repo, paths: paths)) }
    public func stageLines(repo: String, path: String, area: Area, lines: [LinePick]) async throws {
        try await perform(.stageLines(repo: repo, path: path, area: area, lines: lines))
    }
    public func discard(repo: String, paths: [String]) async throws { try await perform(.discard(repo: repo, paths: paths)) }
    public func discardLines(repo: String, path: String, lines: [LinePick]) async throws {
        try await perform(.discardLines(repo: repo, path: path, lines: lines))
    }

    public func commit(repo: String, message: String, amend: Bool) async throws -> (sha: String, output: String) {
        // Hooks may run tests, so allow a long wait.
        guard case let .committed(sha, output) = try await call(.commitChanges(repo: repo, message: message, amend: amend), timeout: 1800) else {
            throw ServiceError.unexpectedResponse
        }
        return (sha, output)
    }

    public func createBranch(repo: String, name: String, start: String?, checkout: Bool) async throws {
        try await perform(.createBranch(repo: repo, name: name, start: start, checkout: checkout))
    }
    public func checkout(repo: String, branch: String) async throws { try await perform(.checkout(repo: repo, branch: branch)) }
    public func renameBranch(repo: String, from: String, to: String) async throws { try await perform(.renameBranch(repo: repo, from: from, to: to)) }
    public func deleteBranch(repo: String, name: String, force: Bool) async throws { try await perform(.deleteBranch(repo: repo, name: name, force: force)) }

    /// A connection held open on `subscribe`; ends when the stream is dropped or the daemon goes away.
    public func events() -> AsyncStream<RepoEvent> {
        let path = self.path
        return AsyncStream { continuation in
            let box = FDBox()
            continuation.onTermination = { _ in box.shutdown() }
            let thread = Thread {
                defer { continuation.finish() }
                guard let fd = try? Self.connect(path: path, timeout: 0) else { return }
                box.set(fd)
                defer { box.close() }
                guard (try? Self.writeAll(fd, Wire.line(id: 1, .subscribe))) != nil else { return }
                var pending = Data()
                var chunk = [UInt8](repeating: 0, count: 64 * 1024)
                while true {
                    let n = read(fd, &chunk, chunk.count)
                    if n <= 0 {
                        if n < 0, errno == EINTR { continue }
                        return
                    }
                    pending.append(contentsOf: chunk[0 ..< n])
                    while let nl = pending.firstIndex(of: 0x0A) {
                        let line = pending[pending.startIndex ..< nl]
                        pending.removeSubrange(pending.startIndex ... nl)
                        if case let .event(e)? = try? Wire.decode(Data(line)).1 { continuation.yield(e) }
                    }
                }
            }
            thread.name = "neutrino events"
            thread.start()
        }
    }

    /// Send one request and wait for its answer, off the main thread.
    public func call(_ request: Request, timeout: TimeInterval = 60) async throws -> Response {
        let path = self.path
        let line = try Wire.line(id: 1, request)
        return try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    let fd = try Self.connect(path: path, timeout: timeout)
                    defer { close(fd) }
                    try Self.writeAll(fd, line)
                    let reply = try Self.readLine(fd)
                    let (_, response) = try Wire.decode(reply)
                    if case let .error(message) = response {
                        continuation.resume(throwing: ServiceError.daemon(message))
                    } else {
                        continuation.resume(returning: response)
                    }
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    static func connect(path: String, timeout: TimeInterval) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw ServiceError.io(String(cString: strerror(errno))) }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        let capacity = MemoryLayout.size(ofValue: addr.sun_path)
        guard bytes.count < capacity else {
            close(fd)
            throw ServiceError.pathTooLong
        }
        withUnsafeMutableBytes(of: &addr.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        if timeout > 0 {
            var tv = timeval(tv_sec: Int(timeout), tv_usec: 0)
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
        }
        var noSigpipe: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigpipe, socklen_t(MemoryLayout<Int32>.size))
        let ok = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard ok == 0 else {
            let why = String(cString: strerror(errno))
            close(fd)
            throw ServiceError.unreachable(why)
        }
        return fd
    }

    static func writeAll(_ fd: Int32, _ data: Data) throws {
        try data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
            var offset = 0
            while offset < raw.count {
                let n = write(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset)
                if n < 0 {
                    if errno == EINTR { continue }
                    throw ServiceError.io(String(cString: strerror(errno)))
                }
                offset += n
            }
        }
    }

    static func readLine(_ fd: Int32) throws -> Data {
        var out = Data()
        var chunk = [UInt8](repeating: 0, count: 256 * 1024)
        while true {
            let n = read(fd, &chunk, chunk.count)
            if n < 0 {
                if errno == EINTR { continue }
                if errno == EAGAIN { throw ServiceError.io("timed out waiting for neutrinod") }
                throw ServiceError.io(String(cString: strerror(errno)))
            }
            if n == 0 { throw ServiceError.io("neutrinod closed the connection") }
            if let nl = chunk[0 ..< n].firstIndex(of: 0x0A) {
                out.append(contentsOf: chunk[0 ..< nl])
                return out
            }
            out.append(contentsOf: chunk[0 ..< n])
        }
    }
}

/// A socket shared between the reading thread and the stream's cancellation.
final class FDBox: @unchecked Sendable {
    private let lock = NSLock()
    private var fd: Int32 = -1
    private var stopped = false

    func set(_ fd: Int32) {
        lock.lock()
        self.fd = fd
        let stop = stopped
        lock.unlock()
        if stop { Darwin.shutdown(fd, SHUT_RDWR) }
    }

    /// Wake the reader so it returns.
    func shutdown() {
        lock.lock()
        stopped = true
        let fd = self.fd
        lock.unlock()
        if fd >= 0 { Darwin.shutdown(fd, SHUT_RDWR) }
    }

    func close() {
        lock.lock()
        let fd = self.fd
        self.fd = -1
        lock.unlock()
        if fd >= 0 { Darwin.close(fd) }
    }
}
