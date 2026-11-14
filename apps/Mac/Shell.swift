import NeutrinoKit
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 10) {
            HeaderBar()
            HStack(alignment: .top, spacing: 10) {
                Sidebar().frame(width: 228)
                Group {
                    if model.repos.isEmpty, model.connection != .connecting {
                        NoRepositories().island()
                    } else {
                        switch model.section {
                        case .history: HistoryScreen()
                        case .branches: BranchesScreen()
                        }
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            StatusBar()
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 8)
        .background(N.ground.ignoresSafeArea())
        .foregroundStyle(N.body)
        .task { model.start() }
    }
}

struct NoRepositories: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.isLive {
            EmptyState(
                title: "No repositories yet",
                message: "Add a folder inside a git repository. Neutrino reads it in place and changes nothing.",
                action: ("Add repository…", add)
            )
        } else {
            EmptyState(title: "No repositories", message: "neutrinod isn't running.")
        }
    }

    private func add() {
        Task { await model.addRepository() }
    }
}

// MARK: - Header

struct HeaderBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        HStack(spacing: 10) {
            // Room for the traffic lights.
            Color.clear.frame(width: 66, height: 1)
            if let repo = model.repo {
                Menu {
                    ForEach(model.repos) { r in
                        Button(r.name) { Task { await model.select(repo: r.id) } }.disabled(r.error != nil)
                    }
                    Divider()
                    Button("Add repository…") { Task { await model.addRepository() } }.disabled(!model.isLive)
                } label: {
                    HStack(spacing: 7) {
                        Text(repo.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(N.text)
                        Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold)).foregroundStyle(N.muted)
                    }
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .background(N.island, in: Capsule())
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .fixedSize()

                if let head = repo.head {
                    HStack(spacing: 6) {
                        Image(systemName: "arrow.triangle.branch").font(.system(size: 11))
                        Text(head.label).font(N.mono(12, weight: .medium))
                    }
                    .foregroundStyle(N.body)
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .background(N.island, in: Capsule())
                    .help("Checked out")
                }
            }
            Spacer(minLength: 12)
            if let notice = model.notice {
                HStack(spacing: 8) {
                    Text(notice).lineLimit(1).truncationMode(.middle)
                    Button {
                        model.notice = nil
                    } label: {
                        Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss")
                }
                .font(.system(size: 12))
                .foregroundStyle(N.danger)
                .padding(.horizontal, 12)
                .frame(height: 30)
                .background(N.danger.opacity(0.1), in: Capsule())
                .frame(maxWidth: 420)
            }
            Button {
                Task { await model.reload() }
            } label: {
                Image(systemName: "arrow.clockwise").font(.system(size: 12, weight: .semibold))
            }
            .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
            .help("Reload (⌘R)")
            .accessibilityLabel("Reload")
            ConnectionPill()
        }
        .frame(height: 52)
        .contentShape(Rectangle())
        .gesture(WindowDragGesture())
    }
}

struct ConnectionPill: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let (color, title, detail): (Color, String, String) = switch model.connection {
        case .connecting: (N.faint, "Connecting", "Looking for neutrinod")
        case let .live(info): (N.ok, "neutrinod", "\(info.role) \(info.version), protocol \(info.protocol)")
        case let .demo(reason): (N.warn, "Demo data", "neutrinod isn't running: \(reason)\nStart it with scripts/dev-neutrinod.sh. Neutrino connects on its own once it's up.")
        }
        HStack(spacing: 8) {
            Dot(color: color, glow: model.isLive)
            Text(title).font(.system(size: 12, weight: .medium)).foregroundStyle(N.text)
        }
        .padding(.horizontal, 12)
        .frame(height: 30)
        .background(N.island, in: Capsule())
        .help(detail)
    }
}

// MARK: - Sidebar

struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            SectionLabel("This Mac").padding(.horizontal, 10).padding(.top, 4).padding(.bottom, 8)
            NavRow(title: "Changes", icon: "doc.badge.plus", enabled: false, help: "Status, staging and commits arrive in milestone 2.")
            NavRow(title: "History", icon: "clock.arrow.circlepath", selected: model.section == .history) { model.section = .history }
            NavRow(title: "Branches", icon: "arrow.triangle.branch", selected: model.section == .branches, count: model.branches.filter { $0.kind == .local }.count) {
                model.section = .branches
            }

            SectionLabel("On atlas").padding(.horizontal, 10).padding(.top, 18).padding(.bottom, 8)
            ForEach([("Pull requests", "arrow.triangle.pull"), ("Runs", "play.circle"), ("Secrets", "key"), ("Runners", "cpu")], id: \.0) { title, icon in
                NavRow(title: title, icon: icon, enabled: false, help: "Arrives once Neutrino runs on atlas (milestone 3 onwards).")
            }

            SectionLabel("Repositories").padding(.horizontal, 10).padding(.top, 18).padding(.bottom, 8)
            ScrollView {
                VStack(spacing: 2) {
                    ForEach(model.repos) { repo in
                        RepoRow(repo: repo)
                    }
                }
            }
            .scrollIndicators(.never)
            Button {
                Task { await model.addRepository() }
            } label: {
                Label("Add repository…", systemImage: "plus")
                    .font(.system(size: 12.5))
                    .foregroundStyle(model.isLive ? N.muted : N.faint)
                    .padding(.horizontal, 10)
                    .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!model.isLive)
            .help(model.isLive ? "Register a working copy" : "Start neutrinod to add your own repositories")

            Spacer(minLength: 12)
            MachineCard()
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 12)
        .frame(maxHeight: .infinity, alignment: .top)
        .island()
    }
}

struct NavRow: View {
    var title: String
    var icon: String
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
                    .font(.system(size: 13))
                    .frame(width: 18)
                    .foregroundStyle(selected ? N.accent : (enabled ? N.muted : N.faint.opacity(0.7)))
                Text(title)
                    .font(.system(size: 13, weight: selected ? .medium : .regular))
                    .foregroundStyle(selected ? N.text : (enabled ? N.muted : N.faint.opacity(0.7)))
                Spacer()
                if let count, count > 0 {
                    Text("\(count)").font(N.mono(11, weight: .medium)).foregroundStyle(N.faint)
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 34)
            .background(selected ? N.selected : (hovering && enabled ? Color.white.opacity(0.04) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
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
    @State private var hovering = false

    var body: some View {
        let selected = model.repoID == repo.id
        Button {
            Task { await model.select(repo: repo.id) }
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text(repo.name)
                    .font(.system(size: 12.5, weight: selected ? .medium : .regular))
                    .foregroundStyle(repo.error != nil ? N.faint : (selected ? N.text : N.body))
                    .lineLimit(1)
                Text(repo.error ?? repo.head?.label ?? "")
                    .font(repo.error != nil ? .system(size: 11) : N.mono(11))
                    .foregroundStyle(repo.error != nil ? N.danger : N.faint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? N.selected : (hovering ? Color.white.opacity(0.04) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(repo.error != nil && !model.isLive)
        .onHover { hovering = $0 }
        .help(repo.path)
        .contextMenu {
            Button("Reveal in Finder") { model.reveal(repo) }
            Button("Copy path") { model.copy(repo.path) }
            Divider()
            Button("Remove from Neutrino") { Task { await model.remove(repo: repo.id) } }.disabled(!model.isLive)
        }
    }
}

struct MachineCard: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Dot(color: model.isLive ? N.ok : N.warn, glow: model.isLive)
                Text("This Mac").foregroundStyle(N.text)
                Spacer()
                Text(model.isLive ? "workstation" : "demo").foregroundStyle(N.muted)
            }
            HStack(spacing: 8) {
                Dot(color: N.atlas, glow: false).opacity(0.45)
                Text("atlas").foregroundStyle(N.text)
                Spacer()
                Text("not paired yet").foregroundStyle(N.faint)
            }
            .help("Pairing with atlas arrives in milestone 3.")
        }
        .font(.system(size: 12))
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .well()
    }
}

// MARK: - Status bar

struct StatusBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 18) {
            if let repo = model.repo {
                Text(repo.head?.label ?? "").foregroundStyle(N.muted)
                Text(repo.path).lineLimit(1).truncationMode(.middle)
            }
            if model.section == .history, !model.commits.isEmpty {
                Text("\(model.commits.count)\(model.hasMore ? "+" : "") commits")
                if let ms = model.historyMillis { Text(ms < 1 ? "first page in under 1 ms" : "first page in \(ms) ms") }
            }
            Spacer()
            switch model.connection {
            case let .live(info): Text("neutrinod \(info.version) · \(info.repos) repos")
            case .demo: Text("demo data · start neutrinod to read your repos")
            case .connecting: Text("connecting")
            }
        }
        .font(N.mono(11))
        .foregroundStyle(N.faint)
        .padding(.horizontal, 8)
        .frame(height: 16)
    }
}
