import NeutrinoKit
import UserNotifications

/// Apple delivers only an id and "Something needs you". This wakes on the
/// push, fetches the real text from the paired server, and shows that
/// instead; if the server can't be reached in time, the generic alert stays.
/// Its state is set once before the fetch and read once after, so it's safe
/// to hand to the task that fetches.
final class NotificationService: UNNotificationServiceExtension, @unchecked Sendable {
    private var handler: ((UNNotificationContent) -> Void)?
    private var best: UNMutableNotificationContent?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        let keychainGroup = (Bundle.main.object(forInfoDictionaryKey: "NeutrinoKeychainGroup") as? String)
            .flatMap { !$0.isEmpty && !$0.contains("$(") ? $0 : nil }
        ServerStore.shared = ("group.dev.neutrino", keychainGroup)
        handler = contentHandler
        let content = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
        best = content
        guard let id = request.content.userInfo["e"] as? String, let link = ServerStore.load() else {
            contentHandler(content)
            return
        }
        let service = ServerService(link: link)
        Task {
            let n = try? await service.notification(id: id)
            self.finish(with: n)
        }
    }

    override func serviceExtensionTimeWillExpire() {
        finish(with: nil)
    }

    private func finish(with n: NotificationText?) {
        guard let handler, let best else { return }
        self.handler = nil
        if let n {
            best.title = n.title
            best.body = n.body
            best.threadIdentifier = n.repo
        }
        handler(best)
    }
}
