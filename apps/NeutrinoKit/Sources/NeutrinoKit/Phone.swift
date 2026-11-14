import CryptoKit
import Foundation
import LocalAuthentication

// The phone's side of milestone 8: a signing key in the Secure Enclave,
// signed approvals, the Inbox, and the text behind push notifications.

public struct Waiting: Codable, Hashable, Identifiable, Sendable {
    public var repo: String
    public var run: Int64
    public var number: UInt32
    public var runName: String
    public var job: Int64
    public var jobName: String
    public var environment: String?
    public var branch: String?
    public var since: Int64
    public var id: Int64 { job }
}

public struct NotificationText: Codable, Hashable, Identifiable, Sendable {
    public var id: String
    public var kind: String
    public var title: String
    public var body: String
    public var repo: String
    public var run: Int64?
    public var pull: UInt32?
    public var at: Int64
}

public struct Inbox: Codable, Hashable, Sendable {
    public var waiting: [Waiting]
    public var ready: [PullRequest]
    public var open: [PullRequest]
    public var runs: [Run]
    public var events: [NotificationText]

    public init(waiting: [Waiting] = [], ready: [PullRequest] = [], open: [PullRequest] = [], runs: [Run] = [], events: [NotificationText] = []) {
        self.waiting = waiting
        self.ready = ready
        self.open = open
        self.runs = runs
        self.events = events
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        waiting = try c.decode([Waiting].self, forKey: .waiting)
        ready = try c.decode([PullRequest].self, forKey: .ready)
        open = try c.decode([PullRequest].self, forKey: .open)
        runs = try c.decode([Run].self, forKey: .runs)
        events = try c.decodeIfPresent([NotificationText].self, forKey: .events) ?? []
    }
}

public enum DeviceKeyError: Error, LocalizedError {
    case cancelled
    case unavailable(String)

    public var errorDescription: String? {
        switch self {
        case .cancelled: "Not approved."
        case let .unavailable(why): why
        }
    }
}

/// The key this phone signs approvals with. In the Secure Enclave, usable
/// only after Face ID (or the passcode); the server has its public half.
/// The simulator has no Secure Enclave, so there it's a Keychain key, and
/// Face ID is asked for before each signature instead.
public enum DeviceKey {
    private static let account = "device-signing-key"
    private static let service = "dev.neutrino.devicekey"

    private static var query: [String: Any] {
        var q: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service, kSecAttrAccount as String: account]
        if let group = ServerStore.shared?.keychainGroup { q[kSecAttrAccessGroup as String] = group }
        return q
    }

    private static func stored() -> Data? {
        var q = query
        q[kSecReturnData as String] = true
        var out: AnyObject?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }

    public static var exists: Bool { stored() != nil }

    /// Real hardware only: the simulator may claim an enclave it can't use.
    private static var enclave: Bool {
        #if targetEnvironment(simulator)
        false
        #else
        SecureEnclave.isAvailable
        #endif
    }

    /// Make a new key (replacing any) and return its public half: an
    /// uncompressed P-256 point, base64, as the server takes it.
    public static func create() throws -> String {
        forget()
        let blob: Data
        let publicKey: Data
        if enclave {
            var error: Unmanaged<CFError>?
            guard let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, [.privateKeyUsage, .userPresence], &error) else {
                throw DeviceKeyError.unavailable("can't set up the key's protection")
            }
            let key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: access)
            blob = key.dataRepresentation
            publicKey = key.publicKey.x963Representation
        } else {
            let key = P256.Signing.PrivateKey()
            blob = key.rawRepresentation
            publicKey = key.publicKey.x963Representation
        }
        var add = query
        add[kSecValueData as String] = blob
        add[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        let status = SecItemAdd(add as CFDictionary, nil)
        guard status == errSecSuccess else { throw DeviceKeyError.unavailable("couldn't keep the signing key (\(status))") }
        return publicKey.base64EncodedString()
    }

    public static func forget() {
        SecItemDelete(query as CFDictionary)
    }

    /// Sign `message`, asking for Face ID with `reason`. DER, base64.
    public static func sign(_ message: Data, reason: String) async throws -> String {
        guard let blob = stored() else { throw DeviceKeyError.unavailable("this phone has no signing key; pair it again") }
        let context = LAContext()
        context.localizedReason = reason
        if enclave {
            // The key itself asks for Face ID when it's used.
            let key = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: context)
            do {
                return try key.signature(for: message).derRepresentation.base64EncodedString()
            } catch {
                throw DeviceKeyError.cancelled
            }
        }
        do {
            guard try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) else { throw DeviceKeyError.cancelled }
        } catch let error as LAError where error.code == .passcodeNotSet || error.code == .biometryNotEnrolled || error.code == .biometryNotAvailable {
            // A simulator without Face ID set up: sign anyway, it's a development key.
        } catch {
            throw DeviceKeyError.cancelled
        }
        let key = try P256.Signing.PrivateKey(rawRepresentation: blob)
        return try key.signature(for: message).derRepresentation.base64EncodedString()
    }
}

/// What the phone asks of the server.
public protocol PhoneService: Sendable {
    func inbox() async throws -> Inbox
    func notification(id: String) async throws -> NotificationText
    func registerKey(publicKey: String) async throws
    func registerPush(token: String?, sandbox: Bool, events: [String]) async throws
    /// Merge, approve or cancel, signed with this phone's key (Face ID first).
    func signed(_ request: Request, reason: String) async throws -> Response
}

extension ServerService: PhoneService {
    public func inbox() async throws -> Inbox {
        guard case let .inbox(i) = try await call(.inbox) else { throw ServiceError.unexpectedResponse }
        return i
    }

    public func notification(id: String) async throws -> NotificationText {
        guard case let .notification(n) = try await call(.notification(id: id)) else { throw ServiceError.unexpectedResponse }
        return n
    }

    public func registerKey(publicKey: String) async throws {
        guard case .done = try await call(.registerKey(publicKey: publicKey)) else { throw ServiceError.unexpectedResponse }
    }

    public func registerPush(token: String?, sandbox: Bool, events: [String]) async throws {
        guard case .done = try await call(.registerPush(token: token, sandbox: sandbox, events: events)) else { throw ServiceError.unexpectedResponse }
    }

    public func signed(_ request: Request, reason: String) async throws -> Response {
        var line = try Wire.line(id: 0, request)
        if line.last == 0x0A { line.removeLast() }
        let inner = String(decoding: line, as: UTF8.self)
        let at = Int64(Date().timeIntervalSince1970)
        var bytes = [UInt8](repeating: 0, count: 16)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        let nonce = bytes.map { String(format: "%02x", $0) }.joined()
        let signature = try await DeviceKey.sign(Data("\(at)\n\(nonce)\n\(inner)".utf8), reason: reason)
        return try await call(.signed(request: inner, at: at, nonce: nonce, signature: signature))
    }
}
