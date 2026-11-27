import NeutrinoKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ZStack {
            N.ground.ignoresSafeArea()
            WindowAccessor()

            HStack(spacing: 0) {
                if !model.sidebarCollapsed {
                    Sidebar()
                        .frame(width: 232)
                        .transition(.move(edge: .leading).combined(with: .opacity))
                }

                VStack(spacing: 0) {
                    WorkspaceTopBar()

                    Group {
                        if model.repos.isEmpty, model.hostedRepos.isEmpty, model.connection != .connecting {
                            NoRepositories()
                        } else {
                            switch model.section {
                            case .code: CodeScreen()
                            case .changes: ChangesScreen()
                            case .history: HistoryScreen()
                            case .branches: BranchesScreen()
                            case .pulls: PullsScreen()
                            case .runs: RunsScreen()
                            case .secrets: SecretsScreen()
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(N.ground)
            }
            .ignoresSafeArea(.container, edges: .top)

            if model.paletteOpen {
                Color.black.opacity(0.58)
                    .ignoresSafeArea()
                    .onTapGesture { model.paletteOpen = false }

                VStack {
                    CommandPalette()
                        .padding(.top, 96)
                    Spacer()
                }
                .transition(.opacity.combined(with: .scale(scale: 0.97)))
            }

            if let notice = model.notice {
                VStack {
                    Spacer()
                    HStack {
                        Spacer()
                        ToastBanner(message: notice, isError: true) {
                            model.notice = nil
                        }
                    }
                }
                .padding(24)
                .transition(.opacity.combined(with: .move(edge: .bottom)))
            } else if let toast = model.toastMessage {
                VStack {
                    Spacer()
                    HStack {
                        Spacer()
                        ToastBanner(message: toast, isError: model.toastIsError)
                    }
                }
                .padding(24)
                .allowsHitTesting(false)
                .transition(.opacity.combined(with: .move(edge: .bottom)))
            }
        }
        .foregroundStyle(N.body)
        .animation(.easeOut(duration: 0.16), value: model.paletteOpen)
        .animation(.easeOut(duration: 0.16), value: model.toastMessage)
        .animation(.easeOut(duration: 0.16), value: model.notice)
        .animation(.easeInOut(duration: 0.18), value: model.sidebarCollapsed)
        .onKeyPress(.escape) {
            if model.paletteOpen {
                model.paletteOpen = false
                return .handled
            }
            if model.notice != nil {
                model.notice = nil
                return .handled
            }
            return .ignored
        }
        .task { model.start() }
    }
}

struct NoRepositories: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.isLive {
            EmptyState(
                icon: "folder.badge.plus",
                title: "No repositories yet",
                message: "Add a folder inside a git repository. Neutrino reads it in place and changes nothing.",
                action: ("Add repository…", add)
            )
        } else {
            EmptyState(
                icon: "bolt.slash",
                title: "No repositories",
                message: "neutrinod isn't running. Start it with scripts/dev-neutrinod.sh to inspect local repositories."
            )
        }
    }

    private func add() {
        Task { await model.addRepository() }
    }
}

// MARK: - Workspace Top Bar

struct WorkspaceTopBar: View {
    @Environment(AppModel.self) private var model
    @State private var toggleHovered = false
    @State private var reloadHovered = false

    var body: some View {
        HStack(spacing: 10) {
            if model.sidebarCollapsed {
                // Room for macOS window traffic lights when sidebar is hidden.
                Color.clear.frame(width: 72, height: 1)
            }

            Button {
                withAnimation(.easeInOut(duration: 0.18)) {
                    model.sidebarCollapsed.toggle()
                }
            } label: {
                Image(systemName: "sidebar.left")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(toggleHovered ? N.text : N.muted)
                    .frame(width: 28, height: 28)
                    .background(toggleHovered ? N.surfaceHover : Color.clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
            }
            .buttonStyle(.plain)
            .onHover { toggleHovered = $0 }
            .help("Toggle Sidebar (⌘\\)")
            .accessibilityLabel("Toggle Sidebar")

            if let repo = model.repo {
                HStack(spacing: 8) {
                    Text(repo.name)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(N.text)
                        .lineLimit(1)

                    TagPill(
                        text: model.source == .server ? (model.serverLink?.name ?? "server") : "local",
                        color: model.source == .server ? N.violet : N.mint,
                        textColor: model.source == .server ? N.violet : N.muted
                    )

                    if let head = repo.head {
                        HStack(spacing: 5) {
                            Image(systemName: "arrow.triangle.branch")
                                .font(.system(size: 10.5, weight: .semibold))
                                .foregroundStyle(N.indigo)
                            Text(head.label)
                                .font(N.mono(11.5, weight: .medium))
                                .foregroundStyle(N.text)
                                .lineLimit(1)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3.5)
                        .background(N.surfacePill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .help("Checked out")
                    }

                    if model.source == .local, let s = model.status {
                        telemetryPills(s)
                    }
                }
            }

            Spacer(minLength: 12)

            if model.source == .local, model.isLive, model.repo?.bare == false {
                ActionPillButton(
                    title: model.syncing == "Fetching" ? "Fetching…" : "Fetch",
                    systemImage: "arrow.down.circle",
                    tint: N.text,
                    background: N.surfacePill,
                    isLoading: model.syncing == "Fetching"
                ) {
                    Task { await model.fetch() }
                }
                .disabled(model.syncing != nil)
                .help("Fetch every remote")

                let ahead = model.status?.ahead ?? 0
                ActionPillButton(
                    title: model.syncing == "Pushing" ? "Pushing…" : (ahead > 0 ? "Push ↑\(ahead)" : "Push"),
                    systemImage: "arrow.up.circle",
                    tint: ahead > 0 ? N.onAccent : N.text,
                    background: ahead > 0 ? N.indigo : N.surfacePill,
                    isLoading: model.syncing == "Pushing"
                ) {
                    Task { await model.push() }
                }
                .disabled(model.syncing != nil)
                .help(model.status?.upstream.map { "Push to \($0)" } ?? "Push this branch and track it")
            }

            Button {
                Task { await model.reload() }
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(reloadHovered ? N.text : N.muted)
                    .frame(width: 28, height: 28)
                    .background(reloadHovered ? N.surfaceHover : N.surfacePill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
            }
            .buttonStyle(.plain)
            .onHover { reloadHovered = $0 }
            .help("Reload (⌘R)")
            .accessibilityLabel("Reload")
        }
        .padding(.horizontal, 18)
        .frame(height: 46)
        .background(N.ground)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }

    @ViewBuilder
    private func telemetryPills(_ s: WorkingStatus) -> some View {
        HStack(spacing: 6) {
            if let a = s.ahead, let b = s.behind, (a > 0 || b > 0) {
                Text("↑\(a) ↓\(b)")
                    .font(N.mono(11, weight: .medium).monospacedDigit())
                    .foregroundStyle(a > 0 ? N.indigo : N.sky)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    .help(s.upstream.map { "Against \($0)" } ?? "Ahead / behind upstream")
            }
            if !s.isClean {
                TagPill(
                    text: "\(s.changeCount) changed",
                    color: N.coral,
                    textColor: N.body,
                    monospaced: true
                )
            }
        }
    }
}

// MARK: - Sidebar

struct Sidebar: View {
    @Environment(AppModel.self) private var model
    @FocusState private var filterFocused: Bool
    @State private var paletteHovered = false
    @State private var addHovered = false

    var body: some View {
        @Bindable var model = model

        VStack(alignment: .leading, spacing: 0) {
            // Breathing room under macOS window traffic lights.
            Spacer().frame(height: 36)

            repoSwitcher
                .padding(.horizontal, 14)

            // Quick Command Palette Trigger (⌘K)
            Button {
                model.paletteOpen = true
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(N.faint)
                    Text("Jump to repo, branch...")
                        .font(.system(size: 12))
                        .foregroundStyle(N.faint)
                        .lineLimit(1)
                    Spacer()
                    Text("⌘K")
                        .font(N.mono(10, weight: .medium))
                        .foregroundStyle(N.faint)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(paletteHovered ? N.surfaceHover : N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            .buttonStyle(.plain)
            .onHover { paletteHovered = $0 }
            .padding(.horizontal, 14)
            .padding(.top, 10)
            .padding(.bottom, 16)

            // Navigation Groups + Repositories in one clean scroll area.
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 18) {
                    ForEach(Section.Group.allCases, id: \.rawValue) { group in
                        let items = Section.allCases.filter { $0.group == group }
                        VStack(alignment: .leading, spacing: 3) {
                            SectionLabel(group.rawValue)
                                .padding(.horizontal, 10)
                                .padding(.bottom, 3)

                            ForEach(items) { section in
                                navItem(section)
                            }
                        }
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        BorderlessSearchField(
                            text: $model.repoFilter,
                            placeholder: "Filter repositories",
                            focused: $filterFocused
                        )
                        .help("Filter repositories (⌘⇧F)")
                        .background {
                            Button("") { filterFocused = true }.keyboardShortcut("f", modifiers: [.command, .shift]).hidden()
                        }

                        let recents = model.repoFilter.isEmpty ? model.recentEntries() : []
                        if !recents.isEmpty {
                            SectionLabel("Recent")
                                .padding(.horizontal, 10)
                                .padding(.top, 8)
                                .padding(.bottom, 2)
                            VStack(spacing: 2) {
                                ForEach(recents, id: \.repo.id) { entry in
                                    if entry.source == .server {
                                        HostedRow(repo: entry.repo)
                                    } else {
                                        RepoRow(repo: entry.repo, compact: true)
                                    }
                                }
                            }
                        }

                        ServerSection()

                        SectionLabel("Repositories")
                            .padding(.horizontal, 10)
                            .padding(.top, 10)
                            .padding(.bottom, 2)

                        let local = model.repos.filter(model.matchesFilter)
                        if local.isEmpty, !model.repos.isEmpty {
                            Text("None match.")
                                .font(.system(size: 12))
                                .foregroundStyle(N.faint)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 4)
                        }
                        VStack(spacing: 2) {
                            ForEach(local) { repo in
                                RepoRow(repo: repo)
                            }
                        }

                        Button {
                            Task { await model.addRepository() }
                        } label: {
                            Label("Add repository…", systemImage: "plus")
                                .font(.system(size: 12, weight: .medium))
                                .foregroundStyle(model.isLive ? (addHovered ? N.text : N.muted) : N.faint)
                                .padding(.horizontal, 10)
                                .frame(maxWidth: .infinity, minHeight: 30, alignment: .leading)
                                .background(addHovered && model.isLive ? N.surfaceHover : Color.clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                                .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                        .buttonStyle(.plain)
                        .onHover { addHovered = $0 }
                        .disabled(!model.isLive)
                        .help(model.isLive ? "Register a working copy" : "Start neutrinod to add your own repositories")
                    }
                }
                .padding(.horizontal, 14)
                .padding(.bottom, 12)
            }

            MachineCard()
                .padding(.horizontal, 14)
                .padding(.top, 8)
                .padding(.bottom, 14)
        }
        .frame(maxHeight: .infinity)
        .background(N.sidebar)
    }

    private var repoSwitcher: some View {
        Menu {
            if !model.repos.isEmpty {
                SwiftUI.Section("This Mac") {
                    ForEach(model.repos) { r in
                        Button {
                            Task { await model.select(repo: r.id) }
                        } label: {
                            HStack {
                                Text(r.name)
                                if model.source == .local && r.id == model.repoID {
                                    Image(systemName: "checkmark")
                                }
                            }
                        }
                        .disabled(r.error != nil)
                    }
                }
            }
            if !model.hostedRepos.isEmpty {
                SwiftUI.Section(model.serverLink?.name ?? "Server") {
                    ForEach(model.hostedRepos) { r in
                        Button {
                            Task { await model.selectHosted(r.id) }
                        } label: {
                            HStack {
                                Text(r.name)
                                if model.source == .server && r.id == model.repoID {
                                    Image(systemName: "checkmark")
                                }
                            }
                        }
                    }
                }
            }
            Divider()
            Button("Add repository…") { Task { await model.addRepository() } }.disabled(!model.isLive)
        } label: {
            HStack(spacing: 10) {
                ZStack {
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(N.surfaceActive)
                        .frame(width: 26, height: 26)
                    Image(systemName: model.source == .server ? "server.rack" : "arrow.triangle.branch")
                        .font(.system(size: 11.5, weight: .semibold))
                        .foregroundStyle(model.source == .server ? N.violet : N.indigo)
                }

                VStack(alignment: .leading, spacing: 1) {
                    Text(model.repo?.name ?? "Select Repository")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                    Text(switcherSubtitle)
                        .font(N.mono(10))
                        .foregroundStyle(N.faint)
                        .lineLimit(1)
                }

                Spacer(minLength: 4)

                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(N.faint)
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 7)
            .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        }
        .menuStyle(.borderlessButton)
    }

    private var switcherSubtitle: String {
        if let head = model.repo?.head?.label {
            return model.source == .server ? "\(model.serverLink?.name ?? "server") · \(head)" : head
        }
        return model.source == .server ? (model.serverLink?.name ?? "server") : "no repository"
    }

    private func navItem(_ section: Section) -> some View {
        let enabled = section != .changes || model.source == .local
        let count: Int? = switch section {
        case .changes: model.status?.changeCount
        case .branches: model.branches.filter { $0.kind == .local }.count
        case .pulls: model.pullList.filter { $0.state == .open }.count
        case .runs: model.runList.filter { !$0.state.finished }.count
        default: nil
        }
        let helpText: String? = switch section {
        case .changes: model.source == .server ? "A hosted repository has no working copy." : nil
        case .pulls: model.serverLink == nil ? "Pull requests live on your server. Pair with it first." : nil
        case .runs: model.serverLink == nil ? "CI runs on your server. Pair with it first." : nil
        case .secrets: model.serverLink == nil ? "Secrets live on your server. Pair with it first." : nil
        default: nil
        }
        return NavRow(
            title: section.title,
            icon: section.icon,
            accent: section.accentColor,
            selected: model.section == section,
            enabled: enabled,
            count: count,
            help: helpText
        ) {
            model.section = section
        }
    }
}

struct NavRow: View {
    var title: String
    var icon: String
    var accent: Color = N.indigo
    var selected = false
    var enabled = true
    var count: Int? = nil
    var help: String? = nil
    var action: () -> Void = {}

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .font(.system(size: 12, weight: selected ? .semibold : .regular))
                    .foregroundStyle(selected ? accent : (enabled ? N.muted : N.faint.opacity(0.6)))
                    .frame(width: 16)

                Text(title)
                    .font(.system(size: 13, weight: selected ? .medium : .regular))
                    .foregroundStyle(selected ? N.text : (enabled ? N.muted : N.faint.opacity(0.6)))

                Spacer()

                if let count, count > 0 {
                    Text("\(count)")
                        .font(N.mono(10.5, weight: .medium))
                        .foregroundStyle(selected ? N.body : N.faint)
                }

                if selected {
                    Circle()
                        .fill(accent)
                        .frame(width: 5, height: 5)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                selected ? N.surfaceActive : (hovering && enabled ? N.surfaceHover : Color.clear),
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .onHover { hovering = $0 }
        .help(help ?? "")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

struct RepoRow: View {
    @Environment(AppModel.self) private var model
    var repo: Repo
    /// One line, like a hosted row (in Recent).
    var compact = false
    @State private var hovering = false

    var body: some View {
        let selected = model.source == .local && model.repoID == repo.id
        if compact {
            Button {
                Task { await model.select(repo: repo.id) }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "laptopcomputer")
                        .font(.system(size: 11))
                        .foregroundStyle(selected ? N.indigo : N.faint)
                        .frame(width: 16)
                    Text(repo.name)
                        .font(.system(size: 12.5, weight: selected ? .medium : .regular))
                        .foregroundStyle(selected ? N.text : N.body)
                        .lineLimit(1)
                    Spacer()
                    Text(repo.head?.label ?? "")
                        .font(N.mono(10.5))
                        .foregroundStyle(N.faint)
                        .lineLimit(1)
                }
                .padding(.horizontal, 10)
                .frame(height: 29)
                .background(selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
            .help(repo.path)
        } else {
            full(selected: selected)
        }
    }

    private func full(selected: Bool) -> some View {
        Button {
            Task { await model.select(repo: repo.id) }
        } label: {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(repo.name)
                        .font(.system(size: 12.5, weight: selected ? .medium : .regular))
                        .foregroundStyle(repo.error != nil ? N.faint : (selected ? N.text : N.body))
                        .lineLimit(1)
                    Text(repo.error ?? repo.head?.label ?? "")
                        .font(repo.error != nil ? .system(size: 10.5) : N.mono(10.5))
                        .foregroundStyle(repo.error != nil ? N.rose : N.faint)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer(minLength: 4)
                if selected {
                    Circle()
                        .fill(N.indigo)
                        .frame(width: 5, height: 5)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(repo.error != nil && !model.isLive)
        .onHover { hovering = $0 }
        .help(repo.path)
        .contextMenu {
            Button("Reveal in Finder") { model.reveal(repo) }
            Button("Copy path") { model.copy(repo.path, label: "path") }
            Divider()
            Button("Remove from Neutrino") { Task { await model.remove(repo: repo.id) } }.disabled(!model.isLive)
        }
    }
}

struct MachineCard: View {
    @Environment(AppModel.self) private var model

    private var daemonLabel: String {
        switch model.connection {
        case let .live(info): "neutrinod \(info.version)"
        case .demo: "Demo data"
        case .connecting: "Connecting…"
        }
    }

    private var daemonSublabel: String {
        switch model.connection {
        case let .live(info): "\(info.repos) repos · workstation"
        case .demo: "start neutrinod to sync"
        case .connecting: "looking for socket"
        }
    }

    private var serverText: String {
        switch model.serverState {
        case .unpaired: "not paired"
        case .connecting: "connecting"
        case let .online(info): "\(info.repos) hosted"
        case .offline: "offline"
        }
    }

    private var serverHelp: String {
        switch model.serverState {
        case .unpaired: "Pair from the sidebar to host repositories there."
        case .connecting: "Connecting to the server"
        case let .online(info): "neutrinod \(info.version), \(info.repos) hosted repositories"
        case let .offline(why): why
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 9) {
                StatusDot(
                    color: model.isLive ? N.mint : (model.connection == .connecting ? N.faint : N.amber),
                    size: 6,
                    pulse: model.isLive
                )
                VStack(alignment: .leading, spacing: 1) {
                    Text(daemonLabel)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                    Text(daemonSublabel)
                        .font(N.mono(10))
                        .foregroundStyle(N.faint)
                        .lineLimit(1)
                }
                Spacer()
            }

            HStack(spacing: 9) {
                StatusDot(
                    color: model.isServerOnline ? N.violet : N.slate,
                    size: 5,
                    pulse: model.isServerOnline
                )
                Text(model.serverLink?.name ?? "server")
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(model.isServerOnline ? N.body : N.muted)
                    .lineLimit(1)
                Spacer()
                Text(serverText)
                    .font(N.mono(10))
                    .foregroundStyle(N.faint)
            }
            .help(serverHelp)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 9)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
    }
}
