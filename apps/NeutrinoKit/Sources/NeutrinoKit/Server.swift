import CryptoKit
import Foundation
import Network
import Security

// The server is neutrinod --server. Devices reach it over TLS with
// its certificate pinned (adapted from Graviton's remote client). Pairing is
// two steps: connect once to see the certificate's fingerprint, and only when
// the user has checked it against `ntr pair`, connect again pinned to it and
// send the code.

// MARK: - The saved link

/// A paired server, as this Mac knows it. The token lives in the Keychain.
public struct ServerLink: Codable, Hashable, Sendable {
    public var deviceID: String
    /// The server's name.
    public var name: String
    /// Bonjour service name when it was found on the network (survives address changes).
    public var serviceName: String?
    public var address: String?
    public var port: UInt16
    /// SHA-256 of the server's certificate, lowercase hex. Every connection checks it.
    public var fingerprint: String
    /// What to put in front of a hosted repo's path to push to it: an SSH host (`build-server`) or a user@host.
    public var sshHost: String

    public init(deviceID: String, name: String, serviceName: String?, address: String?, port: UInt16, fingerprint: String, sshHost: String) {
        self.deviceID = deviceID
        self.name = name
        self.serviceName = serviceName
        self.address = address
        self.port = port
        self.fingerprint = fingerprint
        self.sshHost = sshHost
    }

    public var shortFingerprint: String { Self.short(fingerprint) }

    /// "3FA2 C91B 00D4 E5F6", as `ntr pair` prints it.
    public static func short(_ fingerprint: String) -> String {
        stride(from: 0, to: min(16, fingerprint.count), by: 4).map { i -> String in
            let start = fingerprint.index(fingerprint.startIndex, offsetBy: i)
            return String(fingerprint[start ..< fingerprint.index(start, offsetBy: 4)]).uppercased()
        }.joined(separator: " ")
    }

    /// The URL to push a hosted repo to: `ssh://build-server/Users/…/app.git`, or the bare path when no SSH host is set.
    public func pushURL(for path: String) -> String {
        let host = sshHost.trimmingCharacters(in: .whitespaces)
        return host.isEmpty ? path : "ssh://\(host)\(path)"
    }

    var endpoints: [NWEndpoint] {
        // The address seen at pairing first: it answers at once, while a
        // Bonjour name has to be resolved first. Bonjour finds it again if the address moves.
        var list: [NWEndpoint] = []
        if let address, let port = NWEndpoint.Port(rawValue: port) {
            list.append(.hostPort(host: NWEndpoint.Host(address), port: port))
        }
        if let serviceName {
            list.append(.service(name: serviceName, type: bonjourType, domain: "local.", interface: nil))
        }
        return list
    }

    public static let defaultPort: UInt16 = 7844
}

let bonjourType = "_neutrino._tcp"

public enum ServerStore {
    private static let key = "server"
    private static let service = "dev.neutrino.server"

    /// Set at launch by an app that shares its pairing with an extension (the
    /// iPhone app and its notification extension): the app group for the link
    /// and the optional Keychain access group for the token.
    nonisolated(unsafe) public static var shared: (appGroup: String, keychainGroup: String?)?

    private static var defaults: UserDefaults {
        shared.flatMap { UserDefaults(suiteName: $0.appGroup) } ?? .standard
    }

    /// The paired server, if any. Doesn't touch the Keychain: reading it can
    /// put up a prompt, which mustn't hold up launch. The token is read when
    /// `ServerService` connects, and a missing one reads as unpaired then.
    public static func load() -> ServerLink? {
        guard let data = defaults.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(ServerLink.self, from: data)
    }

    public static func save(_ link: ServerLink, token: String? = nil) throws {
        if let token { try setToken(token, for: link.deviceID) }
        defaults.set(try JSONEncoder().encode(link), forKey: key)
    }

    public static func forget() {
        if let link = load() { deleteToken(for: link.deviceID) }
        defaults.removeObject(forKey: key)
        DeviceKey.forget()
    }

    private static func query(_ id: String) -> [String: Any] {
        var q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: id]
        if let group = shared?.keychainGroup, !group.isEmpty { q[kSecAttrAccessGroup as String] = group }
        return q
    }

    static func setToken(_ token: String, for id: String) throws {
        SecItemDelete(query(id) as CFDictionary)
        var add = query(id)
        add[kSecValueData as String] = Data(token.utf8)
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        guard status == errSecSuccess else { throw ServiceError.io("couldn't save the server's token in the Keychain (\(status))") }
    }

    static func token(for id: String) -> String? {
        var q = query(id)
        q[kSecReturnData as String] = true
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func deleteToken(for id: String) {
        SecItemDelete(query(id) as CFDictionary)
    }
}

// MARK: - Connection

/// One TLS connection carrying JSON lines, with the server's certificate pinned.
final class LineConnection: @unchecked Sendable {
    private let connection: NWConnection
    private let queue = DispatchQueue(label: "dev.neutrino.server")
    private var buffer = Data()
    private let observed = Observed()
    private final class Observed: @unchecked Sendable { var value: String? }

    /// The certificate fingerprint seen during the handshake.
    var fingerprint: String? { observed.value }

    /// - Parameter pin: the required fingerprint; `nil` accepts any certificate and records it.
    init(endpoint: NWEndpoint, pin: String?) {
        let tls = NWProtocolTLS.Options()
        let observed = self.observed
        sec_protocol_options_set_verify_block(tls.securityProtocolOptions, { _, trust, complete in
            let secTrust = sec_trust_copy_ref(trust).takeRetainedValue()
            guard let chain = SecTrustCopyCertificateChain(secTrust) as? [SecCertificate], let leaf = chain.first else {
                complete(false)
                return
            }
            let fingerprint = SHA256.hash(data: SecCertificateCopyData(leaf) as Data).map { String(format: "%02x", $0) }.joined()
            observed.value = fingerprint
            complete(pin == nil || pin == fingerprint)
        }, queue)
        let tcp = NWProtocolTCP.Options()
        tcp.connectionTimeout = 6
        connection = NWConnection(to: endpoint, using: NWParameters(tls: tls, tcp: tcp))
    }

    func start() async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let once = Once()
            connection.stateUpdateHandler = { [observed] state in
                switch state {
                case .ready:
                    once.run { continuation.resume() }
                case let .failed(error):
                    let mismatch = observed.value != nil
                    once.run {
                        continuation.resume(throwing: mismatch
                            ? ServiceError.unreachable("the server's certificate doesn't match the one you paired with (\(error))")
                            : ServiceError.unreachable(error.localizedDescription))
                    }
                case let .waiting(error):
                    once.run { continuation.resume(throwing: ServiceError.unreachable(error.localizedDescription)) }
                case .cancelled:
                    once.run { continuation.resume(throwing: ServiceError.unreachable("cancelled")) }
                default:
                    break
                }
            }
            connection.start(queue: queue)
            // TCP's own timeout doesn't cover resolving a Bonjour name, which
            // can sit "in progress" indefinitely; give up so the next address gets a turn.
            queue.asyncAfter(deadline: .now() + Self.startTimeout) { [connection] in
                once.run {
                    connection.cancel()
                    continuation.resume(throwing: ServiceError.unreachable("no answer within \(Int(Self.startTimeout)) seconds"))
                }
            }
        }
    }

    static let startTimeout: TimeInterval = 8

    func send(_ data: Data) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            connection.send(content: data, completion: .contentProcessed { error in
                if let error { continuation.resume(throwing: ServiceError.io(error.localizedDescription)) } else { continuation.resume() }
            })
        }
    }

    func readLine() async throws -> Data {
        while true {
            if let nl = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex ..< nl]
                buffer.removeSubrange(buffer.startIndex ... nl)
                return Data(line)
            }
            let chunk: Data = try await withCheckedThrowingContinuation { continuation in
                connection.receive(minimumIncompleteLength: 1, maximumLength: 256 * 1024) { data, _, isComplete, error in
                    if let error { continuation.resume(throwing: ServiceError.io(error.localizedDescription)) }
                    else if let data, !data.isEmpty { continuation.resume(returning: data) }
                    else if isComplete { continuation.resume(throwing: ServiceError.io("the server closed the connection")) }
                    else { continuation.resume(returning: Data()) }
                }
            }
            buffer.append(chunk)
        }
    }

    func call(id: UInt64, _ request: Request) async throws -> Response {
        try await send(Wire.line(id: id, request))
        while true {
            let (got, response) = try Wire.decode(try await readLine())
            if got == id { return response }
        }
    }

    func cancel() { connection.cancel() }
}

private final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false
    func run(_ body: () -> Void) {
        lock.lock()
        defer { lock.unlock() }
        guard !done else { return }
        done = true
        body()
    }
}

// MARK: - Pairing

/// Where to pair: a server found over Bonjour, or an address typed in.
public struct ServerTarget: Hashable, Sendable {
    public var name: String
    public var serviceName: String?
    public var address: String?
    public var port: UInt16

    public init(name: String, serviceName: String? = nil, address: String? = nil, port: UInt16 = ServerLink.defaultPort) {
        self.name = name
        self.serviceName = serviceName
        self.address = address
        self.port = port
    }

    var endpoints: [NWEndpoint] {
        ServerLink(deviceID: "", name: name, serviceName: serviceName, address: address, port: port, fingerprint: "", sshHost: "").endpoints
    }
}

public enum Pairing {
    /// Step one: connect and report the certificate's fingerprint, sending nothing.
    public static func fingerprint(of target: ServerTarget) async throws -> String {
        var last: Error = ServiceError.unreachable("no address to try")
        for endpoint in target.endpoints {
            let c = LineConnection(endpoint: endpoint, pin: nil)
            defer { c.cancel() }
            do {
                try await c.start()
                if let fp = c.fingerprint { return fp }
            } catch {
                last = error
            }
        }
        throw last
    }

    /// Step two, once the user has checked the fingerprint: pinned to it, trade the code for a token.
    public static func pair(_ target: ServerTarget, fingerprint: String, code: String, device: String, sshHost: String) async throws -> ServerLink {
        var last: Error = ServiceError.unreachable("no address to try")
        for endpoint in target.endpoints {
            let c = LineConnection(endpoint: endpoint, pin: fingerprint)
            defer { c.cancel() }
            do {
                try await c.start()
            } catch {
                last = error
                continue
            }
            switch try await c.call(id: 0, .pair(code: code, device: device)) {
            case let .paired(token, deviceID, host):
                let link = ServerLink(deviceID: deviceID, name: host, serviceName: target.serviceName, address: target.address, port: target.port, fingerprint: fingerprint, sshHost: sshHost)
                try ServerStore.save(link, token: token)
                return link
            case let .error(message):
                throw ServiceError.daemon(message)
            default:
                throw ServiceError.unexpectedResponse
            }
        }
        throw last
    }
}

// MARK: - The service

/// Talks to a paired server: reads hosted repos, creates them, follows pushes.
/// One authenticated connection per call, plus one held open for events.
public final class ServerService: NeutrinoService {
    public let link: ServerLink

    public init(link: ServerLink) {
        self.link = link
    }

    private func open() async throws -> LineConnection {
        guard let token = ServerStore.token(for: link.deviceID) else { throw ServiceError.unreachable("this Mac isn't paired with \(link.name) any more") }
        var last: Error = ServiceError.unreachable("no address for \(link.name)")
        for endpoint in link.endpoints {
            let c = LineConnection(endpoint: endpoint, pin: link.fingerprint)
            do {
                try await c.start()
            } catch {
                c.cancel()
                last = error
                continue
            }
            switch try await c.call(id: 0, .auth(token: token)) {
            case .authenticated: return c
            case let .error(message):
                c.cancel()
                throw ServiceError.daemon(message)
            default:
                c.cancel()
                throw ServiceError.unexpectedResponse
            }
        }
        throw last
    }

    public func call(_ request: Request) async throws -> Response {
        let c = try await open()
        defer { c.cancel() }
        let response = try await c.call(id: 1, request)
        if case let .error(message) = response { throw ServiceError.daemon(message) }
        return response
    }

    private static let notHere = ServiceError.daemon("That isn't available on the server; it works on this Mac's own repositories.")

    public func ping() async throws -> UInt32 {
        guard case let .pong(v) = try await call(.ping) else { throw ServiceError.unexpectedResponse }
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

    public func addRepo(path _: String) async throws -> Repo { throw Self.notHere }
    public func removeRepo(id _: String) async throws { throw Self.notHere }

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

    public func createHosted(name: String, description: String?) async throws -> Repo {
        guard case let .repo(r) = try await call(.createHosted(name: name, description: description)) else { throw ServiceError.unexpectedResponse }
        return r
    }

    /// Stop hosting a repo; the server moves it to its deleted folder.
    public func deleteHosted(repo: String) async throws {
        guard case .removed = try await call(.deleteHosted(repo: repo)) else { throw ServiceError.unexpectedResponse }
    }

    public func status(repo _: String) async throws -> WorkingStatus { throw Self.notHere }
    public func workDiff(repo _: String, path _: String, area _: Area) async throws -> FileDiff { throw Self.notHere }
    public func stage(repo _: String, paths _: [String]) async throws { throw Self.notHere }
    public func unstage(repo _: String, paths _: [String]) async throws { throw Self.notHere }
    public func stageLines(repo _: String, path _: String, area _: Area, lines _: [LinePick]) async throws { throw Self.notHere }
    public func discard(repo _: String, paths _: [String]) async throws { throw Self.notHere }
    public func discardLines(repo _: String, path _: String, lines _: [LinePick]) async throws { throw Self.notHere }
    public func commit(repo _: String, message _: String, amend _: Bool) async throws -> (sha: String, output: String) { throw Self.notHere }
    public func createBranch(repo _: String, name _: String, start _: String?, checkout _: Bool) async throws { throw Self.notHere }
    public func checkout(repo _: String, branch _: String) async throws { throw Self.notHere }
    public func branchStacks(repo _: String) async throws -> [BranchStackStatus] { [] }
    public func hibernateBranchStack(repo _: String, branch _: String) async throws -> BranchStackStatus { throw Self.notHere }
    public func thawBranchStack(repo _: String, branch _: String) async throws -> BranchStackStatus { throw Self.notHere }
    public func discardBranchStack(repo _: String, branch _: String) async throws -> BranchStackStatus { throw Self.notHere }
    public func shadowStatus(repo _: String) async throws -> ShadowStatus { throw Self.notHere }
    public func configureShadow(repo _: String, enabled _: Bool, cmd _: String?, useGraviton _: Bool?, image _: String?) async throws -> ShadowStatus { throw Self.notHere }
    public func runShadowNow(repo _: String) async throws -> ShadowStatus { throw Self.notHere }
    public func renameBranch(repo _: String, from _: String, to _: String) async throws { throw Self.notHere }
    public func deleteBranch(repo _: String, name _: String, force _: Bool) async throws { throw Self.notHere }
    public func remotes(repo _: String) async throws -> [RemoteInfo] { [] }
    public func addRemote(repo _: String, name _: String, url _: String) async throws { throw Self.notHere }
    public func fetch(repo _: String, remote _: String?) async throws -> String { throw Self.notHere }
    public func push(repo _: String, remote _: String?, branch _: String?, setUpstream _: Bool) async throws -> String { throw Self.notHere }

    /// Pushes and other changes on the server, until the stream is dropped.
    public func events() -> AsyncStream<RepoEvent> {
        AsyncStream { continuation in
            let task = Task {
                guard let c = try? await self.open() else {
                    continuation.finish()
                    return
                }
                defer { c.cancel() }
                do {
                    try await c.send(Wire.line(id: 2, .subscribe))
                    while !Task.isCancelled {
                        if case let .event(e) = try Wire.decode(try await c.readLine()).1 { continuation.yield(e) }
                    }
                } catch {}
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

// MARK: - Discovery

/// Servers advertising `_neutrino._tcp` on the local network.
@MainActor
public final class ServerBrowser {
    public struct Found: Hashable, Identifiable, Sendable {
        public var name: String
        public var id: String { name }
    }

    public private(set) var found: [Found] = []
    public var onChange: (([Found]) -> Void)?
    private var browser: NWBrowser?

    public init() {}

    public func start() {
        guard browser == nil else { return }
        let b = NWBrowser(for: .bonjour(type: bonjourType, domain: nil), using: .tcp)
        b.browseResultsChangedHandler = { [weak self] results, _ in
            let names = results.compactMap { r -> Found? in
                if case let .service(name, _, _, _) = r.endpoint { return Found(name: name) }
                return nil
            }
            Task { @MainActor in
                self?.found = names.sorted { $0.name < $1.name }
                self?.onChange?(self?.found ?? [])
            }
        }
        b.start(queue: .main)
        browser = b
    }

    public func stop() {
        browser?.cancel()
        browser = nil
    }
}
