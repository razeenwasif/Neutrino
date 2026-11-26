import NeutrinoKit
import Observation
import SwiftUI
import UserNotifications

/// Shared with the notification extension: where the pairing and the token live.
enum Shared {
    static let appGroup = "group.dev.neutrino"
    static let keychainGroup = "YOUR_TEAM_ID.dev.neutrino.shared"

    static func configure() {
        ServerStore.shared = (appGroup, keychainGroup)
    }
}

struct RunRef: Hashable, Identifiable {
    var repo: String
    var id: Int64
}

struct PullRef: Hashable, Identifiable {
    var repo: String
    var number: UInt32
    var id: String { "\(repo)#\(number)" }
}

/// Everything the phone shows, from the paired server.
@MainActor @Observable
final class PhoneModel {
    enum Status: Equatable {
        case unpaired
        case loading
        case online
        case offline(String)
    }

    var link: ServerLink? = ServerStore.load()
    var status: Status = .unpaired
    var inbox = Inbox()
    var notice: String?
    var busy: String?
    /// Opened from a notification.
    var openRun: RunRef?
    var openPull: PullRef?

    // Notification switches, kept with the pairing.
    var notifyApprovals: Bool {
        get { UserDefaults(suiteName: Shared.appGroup)?.object(forKey: "notifyApprovals") as? Bool ?? true }
        set { UserDefaults(suiteName: Shared.appGroup)?.set(newValue, forKey: "notifyApprovals"); Task { await registerPush() } }
    }

    var notifyFailures: Bool {
        get { UserDefaults(suiteName: Shared.appGroup)?.object(forKey: "notifyFailures") as? Bool ?? true }
        set { UserDefaults(suiteName: Shared.appGroup)?.set(newValue, forKey: "notifyFailures"); Task { await registerPush() } }
    }

    var pushToken: String?
    var pushError: String?

    var service: ServerService? { link.map(ServerService.init(link:)) }
    var signsApprovals: Bool { DeviceKey.exists }

    func start() async {
        status = link == nil ? .unpaired : .loading
        await refresh()
    }

    func refresh() async {
        guard let service else {
            status = .unpaired
            publishWidgetSnapshot()
            return
        }
        do {
            inbox = try await service.inbox()
            status = .online
        } catch {
            status = .offline(error.localizedDescription)
        }
        publishWidgetSnapshot()
    }

    private func publishWidgetSnapshot() {
        NeutrinoWidgetStore.save(
            NeutrinoWidgetSnapshot.fromInbox(inbox, serverName: link?.name, online: status == .online)
        )
    }

    func handleURL(_ url: URL) {
        guard url.scheme == "neutrino" else { return }
        let host = url.host ?? ""
        let segments = url.pathComponents.dropFirst().map { $0 }
        switch host {
        case "pull":
            if segments.count >= 2, let number = UInt32(segments[1]) {
                openPull = PullRef(repo: segments[0], number: number)
            }
        case "run":
            if segments.count >= 2, let id = Int64(segments[1]) {
                openRun = RunRef(repo: segments[0], id: id)
            }
        case "inbox", "pulls", "runs", "changes":
            Task { await refresh() }
        default:
            break
        }
    }

    // MARK: Pairing

    /// Pair, then give the server this phone's signing key.
    func pair(target: ServerTarget, fingerprint: String, code: String) async throws {
        let device = UIDevice.current.name
        let link = try await Pairing.pair(target, fingerprint: fingerprint, code: code.trimmingCharacters(in: .whitespaces), device: device, sshHost: "")
        self.link = link
        let service = ServerService(link: link)
        do {
            let key = try DeviceKey.create()
            try await service.registerKey(publicKey: key)
        } catch {
            notice = "Paired, but the signing key didn't register: \(error.localizedDescription). Approvals will be refused until you pair again."
        }
        await refresh()
        await askForNotifications()
    }

    func unpair() {
        ServerStore.forget()
        link = nil
        inbox = Inbox()
        status = .unpaired
        publishWidgetSnapshot()
    }

    // MARK: Actions (signed)

    private func act(_ what: String, reason: String, _ request: Request) async -> Response? {
        guard let service, busy == nil else { return nil }
        busy = what
        defer { busy = nil }
        do {
            let r = try await service.signed(request, reason: reason)
            await refresh()
            return r
        } catch DeviceKeyError.cancelled {
            return nil
        } catch {
            notice = error.localizedDescription
            return nil
        }
    }

    func approve(_ w: Waiting, _ yes: Bool) async {
        _ = await act(yes ? "Approving" : "Turning down",
                      reason: yes ? "Approve deploying \(w.jobName) to \(w.environment ?? "its environment")" : "Turn down \(w.jobName)",
                      .approveJob(repo: w.repo, job: w.job, approve: yes))
    }

    func approve(repo: String, job: RunJob, _ yes: Bool) async -> RunDetail? {
        let r = await act(yes ? "Approving" : "Turning down", reason: yes ? "Approve deploying \(job.name)" : "Turn down \(job.name)",
                          .approveJob(repo: repo, job: job.id, approve: yes))
        if case let .run(d)? = r { return d }
        return nil
    }

    func cancel(repo: String, run: Run) async -> RunDetail? {
        let r = await act("Cancelling", reason: "Cancel \(run.name) run \(run.number)", .cancelRun(repo: repo, id: run.id))
        if case let .run(d)? = r { return d }
        return nil
    }

    func merge(_ p: PullRequest, strategy: MergeStrategy, deleteSource: Bool) async -> Bool {
        let r = await act("Merging", reason: "Merge #\(p.number) \(p.title) into \(p.target)",
                          .mergePull(repo: p.repo, number: p.number, strategy: strategy, message: nil, deleteSource: deleteSource))
        if case .merged? = r { return true }
        return false
    }

    // MARK: Notifications

    func askForNotifications() async {
        let center = UNUserNotificationCenter.current()
        let granted = (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        if granted { UIApplication.shared.registerForRemoteNotifications() }
    }

    func gotPushToken(_ token: Data) async {
        pushToken = token.map { String(format: "%02x", $0) }.joined()
        await registerPush()
    }

    func registerPush() async {
        guard let service, let pushToken else { return }
        var events: [String] = []
        if notifyApprovals { events.append("approvals") }
        if notifyFailures { events.append("failures") }
        #if DEBUG
        let sandbox = true
        #else
        let sandbox = false
        #endif
        do {
            try await service.registerPush(token: events.isEmpty ? nil : pushToken, sandbox: sandbox, events: events)
            pushError = nil
        } catch {
            pushError = error.localizedDescription
        }
    }

    /// A tapped notification: open what it was about.
    func open(notification id: String) async {
        guard let service, let n = try? await service.notification(id: id) else { return }
        if let run = n.run { openRun = RunRef(repo: n.repo, id: run) }
        if let pull = n.pull, n.run == nil { openPull = PullRef(repo: n.repo, number: pull) }
    }
}
