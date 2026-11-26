import NeutrinoKit
import SwiftUI

@main
struct NeutrinoMacApp: App {
    static let mainWindow = "main"

    @State private var model: AppModel
    @AppStorage("menuBarIcon") private var showMenuBarIcon = true

    init() {
        let model = AppModel()
        _model = State(initialValue: model)
        // Connect at launch, so the menu bar item is live even before the window opens.
        model.start()
    }

    var body: some Scene {
        WindowGroup("Neutrino", id: Self.mainWindow) {
            RootView()
                .environment(model)
                .frame(minWidth: 1080, minHeight: 700)
                .preferredColorScheme(.dark)
                .tint(N.indigo)
        }
        .windowStyle(.hiddenTitleBar)
        // The header shares the title bar's row; empty space still drags the window.
        .windowBackgroundDragBehavior(.enabled)
        .defaultSize(width: 1440, height: 900)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Add Repository…") { Task { await model.addRepository() } }
                    .keyboardShortcut("o")
                    .disabled(!model.isLive)
            }
            CommandGroup(after: .toolbar) {
                Toggle("Show in Menu Bar", isOn: $showMenuBarIcon)
            }
            CommandMenu("Repository") {
                Button("Go to…") { model.paletteOpen.toggle() }.keyboardShortcut("k")
                Button("Toggle Sidebar") {
                    withAnimation(.easeInOut(duration: 0.18)) {
                        model.sidebarCollapsed.toggle()
                    }
                }
                .keyboardShortcut("\\", modifiers: .command)
                Button("Reload") { Task { await model.reload() } }.keyboardShortcut("r")
                Divider()
                Button("Code") { model.section = .code }.keyboardShortcut("0")
                Button("Changes") { model.section = .changes }.keyboardShortcut("1")
                Button("History") { model.section = .history }.keyboardShortcut("2")
                Button("Branches") { model.section = .branches }.keyboardShortcut("3")
                Button("Pull Requests") { model.section = .pulls }.keyboardShortcut("4")
                Button("Runs") { model.section = .runs }.keyboardShortcut("5")
                Button("Secrets") { model.section = .secrets }.keyboardShortcut("6")
                Divider()
                Button("Stage All") { Task { await model.stageAll() } }.keyboardShortcut("a", modifiers: [.command, .shift]).disabled(!model.isLive && model.status == nil)
                Divider()
                Button("Unified Diff") { model.layout = .unified }.keyboardShortcut("u", modifiers: [.command, .option])
                Button("Split Diff") { model.layout = .split }.keyboardShortcut("s", modifiers: [.command, .option])
            }
        }

        MenuBarExtra(isInserted: $showMenuBarIcon) {
            MenuBarPanel().environment(model)
        } label: {
            MenuBarLabel().environment(model)
        }
        .menuBarExtraStyle(.window)
    }
}
