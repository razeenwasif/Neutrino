import AppKit
import NeutrinoKit
import SwiftUI

// MARK: - Model

/// What the menu bar panel shows beyond the main window's state: every working
/// copy's status and the selected repo's open pull requests.
extension AppModel {
    func glanceStatus(_ repo: Repo) -> WorkingStatus? {
        repo.id == repoID && source == .local ? status : glance[repo.id]
    }

    var workingCopies: [Repo] { repos.filter { !$0.bare && $0.error == nil } }

    func refreshGlance() async {
        let service = self.service
        let ids = workingCopies.map(\.id)
        let statuses = await withTaskGroup(of: (String, WorkingStatus?).self) { group in
            for id in ids {
                group.addTask { (id, try? await service.status(repo: id)) }
            }
            var all: [String: WorkingStatus] = [:]
            for await (id, s) in group { if let s { all[id] = s } }
            return all
        }
        glance = statuses
        await refreshRepoList()
        await refreshGlancePulls()
        publishWidgetSnapshot()
    }

    func refreshGlancePulls() async {
        guard let pulls = pullService else {
            glancePulls = []
            glancePullsNote = serverLink == nil ? "Pair with your server to see pull requests." : "\(serverLink?.name ?? "The server") is offline."
            return
        }
        guard let hosted = await resolvePullRepo() else {
            glancePulls = []
            glancePullsNote = "Not published to \(serverLink?.name ?? "the server")."
            return
        }
        do {
            glancePulls = try await pulls.pulls(repo: hosted).filter { $0.state == .open }.sorted { $0.updatedAt > $1.updatedAt }
            glancePullsNote = nil
        } catch {
            glancePulls = []
            glancePullsNote = error.localizedDescription
        }
    }

    /// Pick a repo from the panel without leaving it.
    func glanceSelect(_ repo: Repo, hosted: Bool) async {
        if hosted { await selectHosted(repo.id) } else { await select(repo: repo.id) }
        await refreshGlancePulls()
    }

    func openInTerminal(_ repo: Repo) {
        let url = URL(fileURLWithPath: repo.path)
        guard let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") else { return }
        NSWorkspace.shared.open([url], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
    }
}

/// Bring the main window forward, opening it again if it was closed.
@MainActor
func showMainWindow(_ openWindow: OpenWindowAction) {
    NSApp.activate()
    if let window = NSApp.windows.first(where: { $0.identifier?.rawValue.hasPrefix(NeutrinoMacApp.mainWindow) == true }) {
        window.makeKeyAndOrderFront(nil)
    } else {
        openWindow(id: NeutrinoMacApp.mainWindow)
    }
}

// MARK: - Label

/// The menu bar item: the glyph, plus the selected repo's change count when it has any.
struct MenuBarLabel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let count = model.source == .local ? (model.status?.changeCount ?? 0) : 0
        HStack(spacing: 3) {
            Image(nsImage: MenuBarIcon.image(model.connection == .connecting || model.isLive ? .live : .offline))
            if count > 0 {
                Text("\(count)").monospacedDigit()
            }
        }
        .accessibilityLabel(count > 0 ? "Neutrino, \(count) changed files" : "Neutrino")
    }
}

// MARK: - Panel

struct MenuBarPanel: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            PanelHeader()
            if model.repo == nil {
                PanelEmpty()
            } else {
                FocusCard()
                SectionJumps()
                if model.serverLink != nil { PanelPulls() }
                if !model.commits.isEmpty { PanelCommits() }
            }
            if model.workingCopies.count + model.hostedRepos.count > 1 { PanelRepos() }
            PanelFooter()
        }
        .padding(14)
        .frame(width: 364)
        .background(N.palette)
        .foregroundStyle(N.body)
        .environment(\.colorScheme, .dark)
        .tint(N.indigo)
        .task {
            model.start()
            await model.refreshGlance()
        }
    }
}

private struct PanelHeader: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 8) {
            Image(nsImage: NSApp.applicationIconImage)
                .resizable()
                .frame(width: 22, height: 22)
            Text("Neutrino")
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundStyle(N.text)
            Spacer()
            TagPill(text: daemonLabel, color: daemonColor, textColor: N.muted, monospaced: true)
                .help(daemonHelp)
            if let link = model.serverLink {
                TagPill(text: link.name, color: model.isServerOnline ? N.violet : N.slate, textColor: N.muted, monospaced: true)
                    .help(serverHelp(link.name))
            }
            Button {
                Task { await model.reload(); await model.refreshGlance() }
            } label: {
                Image(systemName: "arrow.clockwise").font(.system(size: 11, weight: .semibold))
            }
            .buttonStyle(IconButtonStyle())
            .help("Reload")
            .accessibilityLabel("Reload")
        }
    }

    private var daemonColor: Color {
        switch model.connection {
        case .live: N.mint
        case .demo: N.amber
        case .connecting: N.slate
        }
    }

    private var daemonLabel: String {
        switch model.connection {
        case .live: "neutrinod"
        case .demo: "demo"
        case .connecting: "connecting"
        }
    }

    private var daemonHelp: String {
        switch model.connection {
        case let .live(info): "neutrinod \(info.version), \(info.repos) repos"
        case let .demo(reason): "Showing demo data: \(reason)"
        case .connecting: "Connecting to neutrinod"
        }
    }

    private func serverHelp(_ name: String) -> String {
        switch model.serverState {
        case let .online(info): "\(name) online, neutrinod \(info.version)"
        case let .offline(why): "\(name) offline: \(why)"
        case .connecting: "Connecting to \(name)"
        case .unpaired: name
        }
    }
}

/// The selected repo: branch, sync state, what's changed, and quick actions.
private struct FocusCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        if let repo = model.repo {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(repo.name)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(N.text)
                            .lineLimit(1)
                        Text(model.source == .server ? "on \(model.serverLink?.name ?? "server")" : abbreviated(repo.path))
                            .font(N.mono(10.5))
                            .foregroundStyle(N.faint)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer(minLength: 8)
                    if let head = repo.head {
                        HStack(spacing: 5) {
                            Image(systemName: "arrow.triangle.branch")
                                .font(.system(size: 10, weight: .semibold))
                            Text(head.label)
                                .font(N.mono(11, weight: .medium))
                        }
                        .foregroundStyle(N.indigo)
                        .lineLimit(1)
                        .padding(.horizontal, 8)
                        .frame(height: 22)
                        .background(N.indigo.opacity(0.14), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .fixedSize()
                    }
                }

                if model.source == .local, let s = model.status {
                    SyncLine(status: s, syncing: model.syncing)
                    ChangeCounts(status: s)
                    QuickCommit(status: s)
                } else if model.source == .local, let error = model.statusError {
                    Text(error).font(.system(size: 11.5)).foregroundStyle(N.rose).lineLimit(2)
                }

                HStack(spacing: 6) {
                    if model.source == .local {
                        Button { Task { await model.fetch() } } label: { Label("Fetch", systemImage: "arrow.down") }
                            .help("Fetch from the upstream remote")
                        Button { Task { await model.push() } } label: { Label("Push", systemImage: "arrow.up") }
                            .help("Push the current branch")
                    }
                    Spacer()
                    if model.source == .local {
                        Button { model.reveal(repo) } label: { Image(systemName: "folder") }
                            .buttonStyle(IconButtonStyle()).help("Show in Finder").accessibilityLabel("Show in Finder")
                        Button { model.openInTerminal(repo) } label: { Image(systemName: "terminal") }
                            .buttonStyle(IconButtonStyle()).help("Open in Terminal").accessibilityLabel("Open in Terminal")
                    }
                    Button { model.copy(model.source == .server ? (model.serverLink?.pushURL(for: repo.path) ?? repo.path) : repo.path) } label: {
                        Image(systemName: "doc.on.doc")
                    }
                    .buttonStyle(IconButtonStyle())
                    .help(model.source == .server ? "Copy the clone URL" : "Copy the path")
                    .accessibilityLabel(model.source == .server ? "Copy clone URL" : "Copy path")
                }
                .buttonStyle(PillButtonStyle(height: 26))
                .disabled(model.syncing != nil)
            }
            .padding(12)
            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
    }

    private func abbreviated(_ path: String) -> String {
        let home = NSHomeDirectory()
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }
}

private struct SyncLine: View {
    var status: WorkingStatus
    var syncing: String?

    var body: some View {
        HStack(spacing: 8) {
            if let syncing {
                ProgressView().controlSize(.mini)
                Text("\(syncing)…").foregroundStyle(N.muted)
            } else if let up = status.upstream {
                let ahead = status.ahead ?? 0, behind = status.behind ?? 0
                Text("↑\(ahead)").foregroundStyle(ahead > 0 ? N.indigo : N.faint)
                Text("↓\(behind)").foregroundStyle(behind > 0 ? N.sky : N.faint)
                Text(ahead == 0 && behind == 0 ? "up to date with \(up)" : up)
                    .foregroundStyle(N.faint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            } else {
                Text("no upstream yet").foregroundStyle(N.faint)
            }
            if let op = status.operation {
                Spacer()
                Text(op).foregroundStyle(N.amber)
            }
        }
        .font(N.mono(11))
    }
}

private struct ChangeCounts: View {
    var status: WorkingStatus

    var body: some View {
        if status.isClean {
            HStack(spacing: 6) {
                StatusDot(color: N.mint, size: 5)
                Text("Working tree clean").foregroundStyle(N.muted)
            }
            .font(.system(size: 12))
        } else {
            HStack(spacing: 6) {
                count(status.staged.count, "staged", N.mint)
                count(status.unstaged.count, "modified", N.amber)
                count(status.untracked.count, "new", N.sky)
                count(status.conflicted.count, "conflicts", N.rose)
                Spacer(minLength: 0)
            }
        }
    }

    @ViewBuilder
    private func count(_ n: Int, _ label: String, _ color: Color) -> some View {
        if n > 0 {
            HStack(spacing: 4) {
                Text("\(n)").font(N.mono(11, weight: .semibold)).foregroundStyle(color)
                Text(label).font(.system(size: 11)).foregroundStyle(N.muted)
            }
            .padding(.horizontal, 7)
            .frame(height: 22)
            .background(color.opacity(0.12), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        }
    }
}

/// Commit what's staged, or stage everything first, from the panel.
private struct QuickCommit: View {
    @Environment(AppModel.self) private var model
    var status: WorkingStatus

    var body: some View {
        @Bindable var model = model
        if !status.staged.isEmpty {
            HStack(spacing: 6) {
                TextField("", text: $model.commitSummary, prompt: Text("Commit \(status.staged.count) staged \(status.staged.count == 1 ? "file" : "files")…").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(.system(size: 12))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 10)
                    .frame(height: 28)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .onSubmit { Task { await model.commit() } }
                Button {
                    Task { await model.commit() }
                } label: {
                    if model.committing { ProgressView().controlSize(.mini) } else { Text("Commit") }
                }
                .buttonStyle(PillButtonStyle(kind: model.canCommit ? .primary : .secondary, height: 28))
                .disabled(!model.canCommit)
                .keyboardShortcut(.return, modifiers: .command)
                .help("Commit (⌘↩). Runs git, so your hooks and signing apply.")
            }
        } else if !status.unstaged.isEmpty || !status.untracked.isEmpty {
            Button {
                Task { await model.stageAll() }
            } label: {
                Label("Stage all \(status.unstaged.count + status.untracked.count) to commit", systemImage: "plus.circle")
            }
            .buttonStyle(PillButtonStyle(height: 26))
            .disabled(model.writing)
        }
        if let last = model.lastCommit {
            Text("Committed \(String(last.sha.prefix(7)))")
                .font(N.mono(10.5))
                .foregroundStyle(N.mint)
        }
    }
}

/// Jump straight to a section of the main window.
private struct SectionJumps: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @State private var hovered: Section?

    var body: some View {
        HStack(spacing: 6) {
            jump(.code, "Code", "chevron.left.forwardslash.chevron.right")
            jump(.changes, "Changes", "square.and.pencil").disabled(model.source == .server)
            jump(.history, "History", "clock")
            jump(.branches, "Branches", "arrow.triangle.branch")
            jump(.pulls, "Pulls", "arrow.triangle.pull")
        }
    }

    private func jump(_ section: Section, _ title: String, _ symbol: String) -> some View {
        let isHovered = hovered == section
        return Button {
            model.section = section
            if section == .pulls { Task { await model.loadPulls() } }
            showMainWindow(openWindow)
        } label: {
            VStack(spacing: 4) {
                Image(systemName: symbol)
                    .font(.system(size: 12.5))
                    .foregroundStyle(isHovered ? section.accentColor : N.muted)
                Text(title)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(isHovered ? N.text : N.body)
            }
            .frame(maxWidth: .infinity)
            .frame(height: 44)
            .background(isHovered ? N.surfaceHover : N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovered = $0 ? section : nil }
        .help("Open \(title) in Neutrino")
    }
}

private struct PanelPulls: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                SectionLabel("Open pull requests")
                Spacer()
                if !model.glancePulls.isEmpty {
                    Text("\(model.glancePulls.count)")
                        .font(N.mono(10.5, weight: .semibold))
                        .foregroundStyle(N.mint)
                }
            }
            if let note = model.glancePullsNote {
                Text(note).font(.system(size: 11.5)).foregroundStyle(N.faint)
            } else if model.glancePulls.isEmpty {
                Text("None open.").font(.system(size: 11.5)).foregroundStyle(N.faint)
            } else {
                ForEach(model.glancePulls.prefix(3)) { pull in
                    PanelRow {
                        model.section = .pulls
                        showMainWindow(openWindow)
                        Task {
                            await model.loadPulls()
                            await model.select(pull: pull.number)
                        }
                    } content: {
                        Text("#\(pull.number)").font(N.mono(11, weight: .semibold)).foregroundStyle(N.mint)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(pull.title).font(.system(size: 12)).foregroundStyle(N.text).lineLimit(1)
                            Text("\(pull.source) → \(pull.target)").font(N.mono(10)).foregroundStyle(N.faint).lineLimit(1)
                        }
                        Spacer(minLength: 6)
                        if pull.openNotes > 0 {
                            Label("\(pull.openNotes)", systemImage: "text.bubble").font(.system(size: 10.5)).foregroundStyle(N.amber)
                        }
                        Text(Format.ago(pull.updatedAt)).font(N.mono(10)).foregroundStyle(N.faint)
                    }
                }
            }
        }
    }
}

private struct PanelCommits: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionLabel("Recent · \(model.historyTitle)")
            ForEach(model.commits.prefix(3)) { c in
                PanelRow {
                    model.section = .history
                    showMainWindow(openWindow)
                    Task { await model.select(commit: c.sha) }
                } content: {
                    Text(c.shortSHA).font(N.mono(11)).foregroundStyle(N.sky)
                    Text(c.subject).font(.system(size: 12)).foregroundStyle(N.text).lineLimit(1)
                    Spacer(minLength: 6)
                    Text(Format.ago(c.time)).font(N.mono(10)).foregroundStyle(N.faint).fixedSize()
                }
                .contextMenu {
                    Button("Copy SHA") { model.copy(c.sha) }
                    Button("Copy Subject") { model.copy(c.subject) }
                }
            }
        }
    }
}

/// Switch repos without opening the window.
private struct PanelRepos: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionLabel("Repositories")
            ScrollView(.vertical, showsIndicators: false) {
                VStack(spacing: 2) {
                    ForEach(model.workingCopies) { repo in
                        row(repo, hosted: false)
                    }
                    if model.isServerOnline {
                        ForEach(model.hostedRepos) { repo in
                            row(repo, hosted: true)
                        }
                    }
                }
            }
            .frame(maxHeight: 168)
        }
    }

    private func row(_ repo: Repo, hosted: Bool) -> some View {
        let selected = repo.id == model.repoID && (model.source == .server) == hosted
        let s = hosted ? nil : model.glanceStatus(repo)
        return PanelRow(selected: selected) {
            Task { await model.glanceSelect(repo, hosted: hosted) }
        } content: {
            StatusDot(color: hosted ? N.violet : (s.map { $0.isClean ? N.mint : N.amber } ?? N.slate), size: 5)
            Text(repo.name).font(.system(size: 12, weight: selected ? .semibold : .regular)).foregroundStyle(N.text).lineLimit(1)
            Text(repo.head?.label ?? "").font(N.mono(10.5)).foregroundStyle(N.faint).lineLimit(1)
            Spacer(minLength: 6)
            if hosted {
                Text(model.serverLink?.name ?? "server").font(N.mono(10)).foregroundStyle(N.violet.opacity(0.85))
            } else if let s {
                if let a = s.ahead, a > 0 { Text("↑\(a)").font(N.mono(10.5)).foregroundStyle(N.indigo) }
                if let b = s.behind, b > 0 { Text("↓\(b)").font(N.mono(10.5)).foregroundStyle(N.sky) }
                if !s.isClean { Text("\(s.changeCount)").font(N.mono(10.5, weight: .semibold)).foregroundStyle(N.amber) }
            }
        }
        .contextMenu {
            if !hosted {
                Button("Show in Finder") { model.reveal(repo) }
                Button("Open in Terminal") { model.openInTerminal(repo) }
            }
            Button("Copy Path") { model.copy(repo.path) }
        }
    }
}

private struct PanelEmpty: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 6) {
            Text(model.connection == .connecting ? "Connecting…" : model.isLive ? "No repositories yet" : "neutrinod isn't running")
                .font(.system(size: 13, weight: .semibold)).foregroundStyle(N.text)
            Text(model.connection == .connecting ? "Looking for neutrinod." : model.isLive ? "Add a folder inside a git repository." : "Start it with scripts/dev-neutrinod.sh, or install the launch agent.")
                .font(.system(size: 11.5)).foregroundStyle(N.muted).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(18)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

private struct PanelFooter: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @AppStorage("menuBarIcon") private var showIcon = true

    var body: some View {
        HStack(spacing: 6) {
            Button("Open Neutrino") { showMainWindow(openWindow) }
                .buttonStyle(PillButtonStyle(kind: .primary))
                .keyboardShortcut("o", modifiers: [.command, .shift])
            Button("Add…") {
                NSApp.activate()
                Task { await model.addRepository(); await model.refreshGlance() }
            }
            .buttonStyle(PillButtonStyle())
            .disabled(!model.isLive)
            .help("Add a repository")
            Spacer()
            Menu {
                Button("Hide Menu Bar Icon") { showIcon = false }
                Divider()
                Button("Quit Neutrino") { NSApp.terminate(nil) }.keyboardShortcut("q")
            } label: {
                Image(systemName: "ellipsis").font(.system(size: 12, weight: .semibold))
            }
            .menuStyle(.button)
            .menuIndicator(.hidden)
            .buttonStyle(IconButtonStyle())
            .fixedSize()
            .accessibilityLabel("More")
        }
        .padding(.top, 2)
    }
}

// MARK: - Pieces

private struct PanelRow<Content: View>: View {
    var selected = false
    var action: () -> Void
    @ViewBuilder var content: Content
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) { content }
                .padding(.horizontal, 8)
                .frame(height: 30)
                .background(selected ? N.surfaceActive : (hovering ? N.surfaceHover : .clear), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

private struct IconButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(N.body)
            .frame(width: 26, height: 26)
            .background(N.surfacePill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .scaleEffect(configuration.isPressed ? 0.96 : 1.0)
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
    }
}
