import NeutrinoKit
import SwiftUI
import UserNotifications

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    var model: PhoneModel?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { @MainActor in await model?.gotPushToken(deviceToken) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Task { @MainActor in model?.pushError = error.localizedDescription }
    }

    /// Show notifications while the app is open, and refresh the Inbox.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        Task { @MainActor in await self.model?.refresh() }
        completionHandler([.banner, .sound])
    }

    /// A tapped notification opens what it was about.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let id = response.notification.request.content.userInfo["e"] as? String
        Task { @MainActor in
            if let id { await self.model?.open(notification: id) }
        }
        completionHandler()
    }
}

@main
struct NeutrinoPhoneApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model: PhoneModel

    init() {
        Shared.configure()
        _model = State(initialValue: PhoneModel())
    }

    var body: some Scene {
        WindowGroup {
            PhoneRoot()
                .environment(model)
                .preferredColorScheme(.dark)
                .tint(N.accent)
                .onAppear { delegate.model = model }
        }
    }
}

struct PhoneRoot: View {
    @Environment(PhoneModel.self) private var model
    @Environment(\.scenePhase) private var phase

    var body: some View {
        Group {
            if model.link == nil {
                PairView()
            } else {
                InboxView()
            }
        }
        .task { await model.start() }
        .onChange(of: phase) { _, now in
            if now == .active { Task { await model.refresh() } }
        }
    }
}
