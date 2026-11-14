import NeutrinoKit
import SwiftUI

@main
struct NeutrinoMacApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup("Neutrino") {
            RootView()
                .environment(model)
                .frame(minWidth: 1180, minHeight: 720)
                .preferredColorScheme(.dark)
                .tint(N.accent)
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1440, height: 900)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Add Repository…") { Task { await model.addRepository() } }
                    .keyboardShortcut("o")
                    .disabled(!model.isLive)
            }
            CommandMenu("Repository") {
                Button("Reload") { Task { await model.reload() } }.keyboardShortcut("r")
                Divider()
                Button("History") { model.section = .history }.keyboardShortcut("1")
                Button("Branches") { model.section = .branches }.keyboardShortcut("2")
                Divider()
                Button("Unified Diff") { model.layout = .unified }.keyboardShortcut("u", modifiers: [.command, .option])
                Button("Split Diff") { model.layout = .split }.keyboardShortcut("s", modifiers: [.command, .option])
            }
        }
    }
}
