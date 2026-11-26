import AppKit
import NeutrinoKit
import SwiftUI

/// A repo's home: its files at a branch, the README, and what the repo is
/// made of, with Stratus's pitch-black editorial precision.
struct CodeScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        HStack(spacing: 0) {
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 24) {
                    CodeToolbar()

                    if code.file == nil, code.path.isEmpty, let overview = code.overview {
                        kpiStrip(overview)
                    }

                    if code.file != nil || !code.path.isEmpty {
                        Breadcrumb()
                    }

                    if code.file != nil {
                        FileView()
                    } else {
                        VStack(alignment: .leading, spacing: 20) {
                            FileTable()
                            if let readme = code.readme {
                                ReadmeCard(blob: readme)
                            }
                        }
                    }
                }
                .padding(.horizontal, 32)
                .padding(.top, 22)
                .padding(.bottom, 40)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.ground)

            AboutPanel()
                .frame(width: 280)
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? "")") {
            code.reset(for: "\(model.source.rawValue):\(model.repoID ?? "")")
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(code.rev ?? ""):\(code.path):\(model.isServerOnline)") {
            guard let repo = model.repoID else { return }
            await code.load(model.activeService, repo: repo)
        }
    }

    private func kpiStrip(_ o: RepoOverview) -> some View {
        let branchCount = model.branches.filter { $0.kind == .local }.count
        return HStack(spacing: 28) {
            MetricStatColumn(
                label: "Commits",
                value: "\(o.commits.formatted())\(o.commitsCapped ? "+" : "")",
                subtitle: model.code.rev ?? model.repo?.head?.label ?? "HEAD"
            )
            MetricStatColumn(
                label: "Branches",
                value: "\(branchCount)",
                subtitle: "\(model.branches.filter { $0.kind == .tag }.count) tags"
            )
            MetricStatColumn(
                label: "Files",
                value: "\(o.files.formatted())",
                subtitle: o.languages.first?.name
            )
            MetricStatColumn(
                label: "Contributors",
                value: "\(o.contributors)",
                subtitle: o.license
            )
        }
    }
}

// MARK: - Toolbar

private struct CodeToolbar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var code = model.code
        let branches = model.branches.filter { $0.kind == .local }
        let tags = model.branches.filter { $0.kind == .tag }

        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text(model.repo?.name ?? "Repository")
                    .font(N.pageTitle())
                    .foregroundStyle(N.text)
                HStack(spacing: 10) {
                    RefPicker()
                    Button {
                        model.section = .branches
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "arrow.triangle.branch").font(.system(size: 11))
                            Text("\(branches.count) \(branches.count == 1 ? "branch" : "branches")")
                        }
                        .font(.system(size: 12))
                        .foregroundStyle(N.muted)
                    }
                    .buttonStyle(.plain)
                    .help("Show the branches")

                    HStack(spacing: 4) {
                        Image(systemName: "tag").font(.system(size: 11))
                        Text("\(tags.count) \(tags.count == 1 ? "tag" : "tags")")
                    }
                    .font(.system(size: 12))
                    .foregroundStyle(N.faint)
                }
            }

            Spacer(minLength: 12)

            if code.file == nil {
                BorderlessSearchField(text: $code.filter, placeholder: "Filter files...", width: 190)
            }

            CloneMenu()
        }
    }
}

/// Branches and tags to browse.
private struct RefPicker: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        let label = code.rev ?? model.repo?.head?.label ?? "HEAD"
        Menu {
            SwiftUI.Section("Branches") {
                ForEach(model.branches.filter { $0.kind == .local }) { b in
                    Button { pick(b.isHead ? nil : b.name) } label: { Text(b.isHead ? "\(b.name) (default)" : b.name) }
                }
            }
            let tags = model.branches.filter { $0.kind == .tag }
            if !tags.isEmpty {
                SwiftUI.Section("Tags") {
                    ForEach(tags.prefix(30)) { t in
                        Button(t.name) { pick(t.name) }
                    }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "arrow.triangle.branch")
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundStyle(N.indigo)
                Text(label)
                    .font(N.mono(11.5, weight: .medium))
                    .foregroundStyle(N.text)
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(.system(size: 8.5, weight: .bold))
                    .foregroundStyle(N.faint)
            }
            .padding(.horizontal, 10)
            .frame(height: 26)
            .background(N.surfacePill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .buttonStyle(.plain)
        .fixedSize()
        .help("Switch branch or tag")
    }

    private func pick(_ rev: String?) {
        model.code.rev = rev
    }
}

private struct CloneMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Menu {
            if let repo = model.repo {
                if model.source == .server, let link = model.serverLink {
                    Button("Copy Clone URL") { model.copy(link.pushURL(for: repo.path), label: "clone URL") }
                    Button("Copy git clone Command") { model.copy("git clone \(link.pushURL(for: repo.path))", label: "clone command") }
                } else {
                    Button("Copy Path") { model.copy(repo.path, label: "repository path") }
                    Button("Show in Finder") { model.reveal(repo) }
                    Button("Open in Terminal") { model.openInTerminal(repo) }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.left.forwardslash.chevron.right").font(.system(size: 11, weight: .semibold))
                Text(model.source == .server ? "Clone" : "Open")
                Image(systemName: "chevron.down").font(.system(size: 8.5, weight: .bold))
            }
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(N.onAccent)
            .padding(.horizontal, 12)
            .frame(height: 29)
            .background(N.indigo, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .menuStyle(.button)
        .menuIndicator(.hidden)
        .buttonStyle(.plain)
        .fixedSize()
    }
}

private struct Breadcrumb: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        HStack(spacing: 6) {
            Button(model.repo?.name ?? "root") { code.open(dir: "") }
                .buttonStyle(CrumbStyle())
            ForEach(Array(code.crumbs.enumerated()), id: \.offset) { i, crumb in
                Text("/")
                    .font(N.mono(12))
                    .foregroundStyle(N.faint)
                let last = i == code.crumbs.count - 1
                if last {
                    Text(crumb.name)
                        .font(N.mono(12, weight: .semibold))
                        .foregroundStyle(N.text)
                } else {
                    Button(crumb.name) { code.open(dir: crumb.path) }
                        .buttonStyle(CrumbStyle())
                }
            }
            Spacer()
            if let path = code.file ?? (code.path.isEmpty ? nil : code.path) {
                Button {
                    model.copy(path, label: "path")
                } label: {
                    HStack(spacing: 5) {
                        Image(systemName: "doc.on.doc").font(.system(size: 10.5))
                        Text("Copy path").font(.system(size: 11, weight: .medium))
                    }
                    .foregroundStyle(N.muted)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(N.surfacePill, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                }
                .buttonStyle(.plain)
                .help("Copy the path")
                .accessibilityLabel("Copy path")
            }
        }
        .lineLimit(1)
    }
}

private struct CrumbStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(N.mono(12, weight: .medium))
            .foregroundStyle(N.sky)
            .opacity(configuration.isPressed ? 0.7 : 1)
            .contentShape(Rectangle())
    }
}

// MARK: - Files

private struct FileTable: View {
    @Environment(AppModel.self) private var model
    @State private var expanded = false

    private var isExpanded: Bool {
        expanded || !model.code.path.isEmpty || !model.code.filter.isEmpty
    }

    var body: some View {
        let code = model.code
        let shown = isExpanded
        VStack(alignment: .leading, spacing: 8) {
            LatestCommitBar(
                isExpanded: shown,
                itemCount: code.listing?.entries.count,
                canCollapse: code.path.isEmpty && code.filter.isEmpty
            ) {
                withAnimation(.snappy(duration: 0.2)) {
                    expanded.toggle()
                }
            }

            if shown {
                if let error = code.listingError {
                    EmptyState(icon: "exclamationmark.triangle", title: "Can't list these files", message: error)
                        .frame(minHeight: 180)
                } else if code.listing == nil {
                    HStack { Spacer(); ProgressView().controlSize(.small); Spacer() }
                        .frame(height: 120)
                } else {
                    LazyVStack(spacing: 2) {
                        if !code.path.isEmpty, code.filter.isEmpty {
                            EntryRowButton(action: { code.up() }) {
                                Image(systemName: "arrow.turn.left.up")
                                    .font(.system(size: 12))
                                    .foregroundStyle(N.faint)
                                    .frame(width: 18)
                                Text("..")
                                    .font(N.mono(13))
                                    .foregroundStyle(N.muted)
                                Spacer()
                            }
                            .accessibilityLabel("Parent directory")
                        }
                        let entries = code.visibleEntries
                        if entries.isEmpty {
                            Text(code.filter.isEmpty ? "This directory is empty." : "No file here matches “\(code.filter)”.")
                                .font(.system(size: 12.5))
                                .foregroundStyle(N.faint)
                                .frame(maxWidth: .infinity, minHeight: 64)
                        }
                        ForEach(entries) { entry in
                            EntryRow(entry: entry, commit: code.entryCommits[entry.name])
                        }
                    }
                    .transition(.opacity.combined(with: .move(edge: .top)))
                }
            }
        }
        .onChange(of: "\(model.source.rawValue):\(model.repoID ?? "")") {
            expanded = false
        }
    }
}

private struct LatestCommitBar: View {
    @Environment(AppModel.self) private var model
    var isExpanded: Bool
    var itemCount: Int?
    var canCollapse: Bool
    var onToggle: () -> Void

    var body: some View {
        let code = model.code
        HStack(spacing: 10) {
            Button(action: onToggle) {
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(N.muted)
                    .frame(width: 16, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(!canCollapse)
            .help(isExpanded ? "Collapse files & folders" : "Expand files & folders")

            if let c = code.latest {
                Avatar(name: c.author.name, size: 22)
                Text(c.author.name)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(N.text)
                    .lineLimit(1)
                    .fixedSize()
                Button {
                    showCommit(c.sha)
                } label: {
                    Text(c.subject)
                        .font(.system(size: 12.5))
                        .foregroundStyle(N.muted)
                        .lineLimit(1)
                }
                .buttonStyle(.plain)
                .help(c.subject)
                Spacer(minLength: 8)
                TagPill(text: c.shortSHA, textColor: N.sky, monospaced: true)
                Text(Format.ago(c.time))
                    .font(N.mono(11))
                    .foregroundStyle(N.faint)
                    .fixedSize()
                    .help(Format.full(c.time))
            } else {
                Text(code.loadingListing ? "Reading…" : "No commits")
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.faint)
                Spacer()
            }

            if canCollapse {
                ActionPillButton(
                    title: itemCount.map { isExpanded ? "Hide \($0)" : "\($0) files" } ?? (isExpanded ? "Hide" : "Files"),
                    systemImage: isExpanded ? "chevron.up" : "folder",
                    tint: N.muted,
                    background: N.surfacePill,
                    action: onToggle
                )
                .help(isExpanded ? "Collapse file list" : "Expand file list")
                .fixedSize()
            }

            if let o = code.overview {
                ActionPillButton(
                    title: "\(o.commits.formatted())\(o.commitsCapped ? "+" : "")",
                    systemImage: "clock.arrow.circlepath",
                    tint: N.body,
                    background: N.surfacePill
                ) {
                    model.rev = code.rev
                    model.scope = .current
                    model.section = .history
                    Task { await model.reloadHistory() }
                }
                .help("Open the history")
                .fixedSize()
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 46)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .contentShape(Rectangle())
        .onTapGesture {
            if canCollapse { onToggle() }
        }
    }

    private func showCommit(_ sha: String) {
        model.rev = model.code.rev
        model.scope = .current
        model.section = .history
        Task {
            await model.reloadHistory()
            await model.select(commit: sha)
        }
    }
}

struct Avatar: View {
    var name: String
    var size: CGFloat = 22

    var body: some View {
        let initial = name.split(separator: " ").compactMap(\.first).prefix(2).map(String.init).joined()
        let hue = name.unicodeScalars.reduce(0) { ($0 &* 31 &+ Int($1.value)) & 0xFFFF }
        Text(initial.uppercased())
            .font(.system(size: size * 0.42, weight: .bold))
            .foregroundStyle(N.onAccent)
            .frame(width: size, height: size)
            .background(N.lanes[hue % N.lanes.count], in: Circle())
            .accessibilityHidden(true)
    }
}

private struct EntryRow: View {
    @Environment(AppModel.self) private var model
    var entry: TreeEntry
    var commit: EntryCommit?
    @State private var hovering = false

    var body: some View {
        EntryRowButton(onHoverChange: { hovering = $0 }) {
            guard let repo = model.repoID else { return }
            Task { await model.code.open(entry, model.activeService, repo: repo) }
        } content: {
            Image(systemName: EntryIcon.symbol(entry))
                .font(.system(size: 13))
                .foregroundStyle(EntryIcon.color(entry))
                .frame(width: 18)
            Text(entry.name)
                .font(.system(size: 13, weight: entry.kind.isFolder ? .medium : .regular))
                .foregroundStyle(entry.kind == .submodule ? N.sky : N.text)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: 220, alignment: .leading)
            Text(commit?.subject ?? "")
                .font(.system(size: 12.5))
                .foregroundStyle(N.muted)
                .lineLimit(1)
                .help(commit?.subject ?? "")
            Spacer(minLength: 8)
            if hovering {
                Button {
                    model.copy(entry.path, label: "path")
                } label: {
                    Image(systemName: "doc.on.doc")
                        .font(.system(size: 11))
                        .foregroundStyle(N.muted)
                        .padding(4)
                }
                .buttonStyle(.plain)
                .help("Copy path")
            }
            Text(entry.size.flatMap { entry.kind == .symlink ? nil : ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .file) } ?? "")
                .font(N.mono(11).monospacedDigit())
                .foregroundStyle(N.faint)
                .frame(width: 68, alignment: .trailing)
            Text(commit.map { Format.ago($0.time) } ?? "")
                .font(N.mono(11))
                .foregroundStyle(N.faint)
                .frame(width: 80, alignment: .trailing)
                .help(commit.map { Format.full($0.time) } ?? "")
        }
        .disabled(entry.kind == .submodule)
        .help(entry.kind == .submodule ? "A submodule: another repository at a pinned commit" : entry.path)
        .contextMenu {
            Button("Copy Path") { model.copy(entry.path, label: "path") }
            if let commit { Button("Copy Last Commit SHA") { model.copy(commit.sha, label: "commit SHA") } }
        }
    }
}

private struct EntryRowButton<Content: View>: View {
    var onHoverChange: ((Bool) -> Void)? = nil
    var action: () -> Void
    @ViewBuilder var content: Content
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) { content }
                .padding(.horizontal, 12)
                .frame(height: 36)
                .background(hovering ? N.surfaceHover : Color.clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover {
            hovering = $0
            onHoverChange?($0)
        }
    }
}

enum EntryIcon {
    static func symbol(_ e: TreeEntry) -> String {
        switch e.kind {
        case .dir: return "folder.fill"
        case .submodule: return "shippingbox"
        case .symlink: return "arrow.up.right.square"
        case .executable: return "terminal"
        case .file:
            let n = e.name.lowercased()
            if CodeModel.isReadme(e) { return "book.closed" }
            if n.hasPrefix("license") || n.hasPrefix("licence") || n.hasPrefix("copying") { return "scroll" }
            if CodeModel.isMarkdown(n) { return "doc.richtext" }
            if [".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".icns", ".heic"].contains(where: n.hasSuffix) { return "photo" }
            if [".json", ".toml", ".yml", ".yaml", ".plist", ".lock"].contains(where: n.hasSuffix) { return "curlybraces" }
            return "doc.text"
        }
    }

    static func color(_ e: TreeEntry) -> Color {
        switch e.kind {
        case .dir: return N.indigo
        case .submodule: return N.violet
        case .symlink: return N.sky
        case .executable: return N.mint
        case .file:
            let n = e.name.lowercased()
            if CodeModel.isReadme(e) || CodeModel.isMarkdown(n) { return N.amber }
            if [".json", ".toml", ".yml", ".yaml", ".plist", ".lock"].contains(where: n.hasSuffix) { return N.sky }
            return N.muted
        }
    }
}

// MARK: - README

private struct ReadmeCard: View {
    @Environment(AppModel.self) private var model
    var blob: BlobContent

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "book.closed")
                    .font(.system(size: 12))
                    .foregroundStyle(N.amber)
                Text(blob.path.split(separator: "/").last.map(String.init) ?? blob.path)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(N.text)
                Spacer()
            }
            .padding(.horizontal, 16)
            .frame(height: 40)
            .background(N.surfaceSubtle)

            Group {
                if let text = blob.text {
                    if CodeModel.isMarkdown(blob.path) {
                        MarkdownView(text: text)
                    } else {
                        Text(text)
                            .font(N.mono(12.5))
                            .foregroundStyle(N.body)
                            .textSelection(.enabled)
                    }
                } else {
                    Text("This README is too large to show.")
                        .font(.system(size: 12.5))
                        .foregroundStyle(N.faint)
                }
            }
            .frame(maxWidth: 780, alignment: .leading)
            .padding(.horizontal, 22)
            .padding(.vertical, 20)
            .environment(\.openURL, OpenURLAction { url in
                guard let repo = model.repoID else { return .systemAction }
                let path = blob.path
                if url.scheme == nil {
                    Task { _ = await model.code.follow(url, from: path, model.activeService, repo: repo) }
                    return .handled
                }
                return .systemAction
            })
        }
        .background(N.inspector, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

// MARK: - A file

private struct FileView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var code = model.code
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                if let c = code.entryCommits[code.file?.split(separator: "/").last.map(String.init) ?? ""] {
                    Text(c.subject)
                        .font(.system(size: 12.5))
                        .foregroundStyle(N.muted)
                        .lineLimit(1)
                    TagPill(text: String(c.sha.prefix(7)), textColor: N.sky, monospaced: true)
                    Text(Format.ago(c.time))
                        .font(N.mono(11))
                        .foregroundStyle(N.faint)
                }
                Spacer(minLength: 8)
                if let b = code.blob {
                    Text(stats(b))
                        .font(N.mono(11.5))
                        .foregroundStyle(N.faint)
                        .fixedSize()
                    if CodeModel.isMarkdown(b.path), b.text != nil {
                        Segmented(options: [(false, "Preview"), (true, "Code")], selection: $code.showSource)
                    }
                    if let text = b.text {
                        ActionPillButton(title: "Copy", systemImage: "doc.on.doc", tint: N.text, background: N.surfacePill) {
                            model.copy(text, label: "file contents")
                        }
                        .help("Copy the file's contents")
                    }
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 44)
            .background(N.surfaceSubtle)

            if let error = code.blobError {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't open this file", message: error).frame(minHeight: 200)
            } else if let b = code.blob {
                if b.binary {
                    EmptyState(icon: "doc.zipper", title: "Binary file", message: "\(ByteCountFormatter.string(fromByteCount: Int64(b.size), countStyle: .file)), not shown.").frame(minHeight: 200)
                } else if b.tooLarge || b.text == nil {
                    EmptyState(icon: "doc.text.magnifyingglass", title: "Too large to show", message: "\(ByteCountFormatter.string(fromByteCount: Int64(b.size), countStyle: .file)). Open it from a clone instead.").frame(minHeight: 200)
                } else if CodeModel.isMarkdown(b.path), !code.showSource {
                    MarkdownView(text: b.text ?? "")
                        .frame(maxWidth: 780, alignment: .leading)
                        .padding(.horizontal, 24)
                        .padding(.vertical, 20)
                } else {
                    CodeLines(text: b.text ?? "")
                }
            } else {
                HStack { Spacer(); ProgressView().controlSize(.small); Spacer() }.frame(height: 160)
            }
        }
        .background(N.inspector, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func stats(_ b: BlobContent) -> String {
        let size = ByteCountFormatter.string(fromByteCount: Int64(b.size), countStyle: .file)
        return b.text == nil ? size : "\(b.lines) \(b.lines == 1 ? "line" : "lines") · \(size)"
    }
}

/// Source with line numbers. Lines are laid out lazily, so long files open quickly.
private struct CodeLines: View {
    var text: String

    var body: some View {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        let shown = text.hasSuffix("\n") ? lines.dropLast() : lines[...]
        let gutter = CGFloat(String(shown.count).count) * 7.5 + 24
        ScrollView(.horizontal) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(Array(shown.enumerated()), id: \.offset) { i, line in
                    HStack(spacing: 0) {
                        Text("\(i + 1)")
                            .font(N.mono(11.5).monospacedDigit())
                            .foregroundStyle(N.faint)
                            .frame(width: gutter, alignment: .trailing)
                            .padding(.trailing, 16)
                        Text(line.isEmpty ? " " : String(line).replacingOccurrences(of: "\t", with: "    "))
                            .font(N.mono(12))
                            .foregroundStyle(N.body)
                            .fixedSize()
                    }
                    .frame(height: 20)
                }
            }
            .padding(.vertical, 12)
            .padding(.trailing, 18)
            .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - About Inspector

private struct AboutPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 22) {
                VStack(alignment: .leading, spacing: 8) {
                    SectionLabel("About")
                    if let repo = model.repo {
                        Text(repo.name)
                            .font(.system(size: 16, weight: .semibold))
                            .foregroundStyle(N.text)
                        HStack(spacing: 7) {
                            StatusDot(color: model.source == .server ? N.violet : N.mint, size: 5.5)
                            Text(model.source == .server ? "Hosted on \(model.serverLink?.name ?? "the server")" : "Working copy on this Mac")
                                .font(.system(size: 12))
                                .foregroundStyle(N.muted)
                        }
                        Text(repo.path)
                            .font(N.mono(10.5))
                            .foregroundStyle(N.faint)
                            .lineLimit(2)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                    }
                }

                if let o = code.overview {
                    VStack(alignment: .leading, spacing: 10) {
                        SectionLabel("Overview")
                        VStack(alignment: .leading, spacing: 8) {
                            stat("clock.arrow.circlepath", "\(o.commits.formatted())\(o.commitsCapped ? "+" : "")", o.commits == 1 ? "commit" : "commits", tint: N.sky)
                            stat("arrow.triangle.branch", "\(model.branches.filter { $0.kind == .local }.count)", "branches", tint: N.violet)
                            stat("tag", "\(model.branches.filter { $0.kind == .tag }.count)", "tags", tint: N.amber)
                            stat("person.2", "\(o.contributors)", o.contributors == 1 ? "contributor" : "contributors", tint: N.mint)
                            stat("doc.on.doc", o.files.formatted(), o.files == 1 ? "file" : "files", tint: N.indigo)
                            if let license = o.license {
                                Button {
                                    guard let repo = model.repoID else { return }
                                    Task { await code.open(file: license, model.activeService, repo: repo) }
                                } label: {
                                    stat("scroll", license, "license", tint: N.coral)
                                }
                                .buttonStyle(.plain)
                                .help("Open the licence")
                            }
                        }
                        .padding(12)
                        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    }
                }

                if let o = code.overview, !o.languages.isEmpty {
                    Languages(languages: o.languages)
                }

                if model.serverLink != nil {
                    ServerActivity()
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: .infinity)
        .background(N.inspector)
    }

    private func stat(_ icon: String, _ value: String, _ label: String, tint: Color) -> some View {
        HStack(spacing: 9) {
            Image(systemName: icon)
                .font(.system(size: 11))
                .foregroundStyle(tint)
                .frame(width: 15)
            Text(value)
                .font(.system(size: 12.5, weight: .semibold).monospacedDigit())
                .foregroundStyle(N.text)
                .lineLimit(1)
            Text(label)
                .font(.system(size: 12))
                .foregroundStyle(N.muted)
            Spacer(minLength: 0)
        }
    }
}

private struct Languages: View {
    var languages: [LanguageShare]

    var body: some View {
        let total = max(1, languages.reduce(0) { $0 + $1.bytes })
        let segments = languages.enumerated().map { i, l in
            let p = Double(l.bytes) / Double(total) * 100
            let pct = p < 0.1 ? "<0.1%" : String(format: "%.1f%%", p)
            let color = l.name == "Other" ? N.slate : N.lanes[i % N.lanes.count]
            return BarSegment(label: l.name, value: Double(l.bytes), formattedValue: pct, color: color)
        }
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel("Languages")
            SegmentedBar(segments: segments)
        }
        .accessibilityElement(children: .combine)
    }
}

/// Open pull requests and the newest CI run, from the server.
private struct ServerActivity: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel("On \(model.serverLink?.name ?? "the server")")
            if !model.isServerOnline {
                Text("Offline.")
                    .font(.system(size: 12))
                    .foregroundStyle(N.faint)
            } else if model.glancePullsNote != nil, model.glancePulls.isEmpty {
                Text(model.glancePullsNote ?? "")
                    .font(.system(size: 12))
                    .foregroundStyle(N.faint)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                VStack(spacing: 6) {
                    Button {
                        model.section = .pulls
                    } label: {
                        HStack(spacing: 9) {
                            Image(systemName: "arrow.triangle.pull")
                                .font(.system(size: 11.5))
                                .foregroundStyle(N.mint)
                                .frame(width: 15)
                            Text("\(model.glancePulls.count)")
                                .font(.system(size: 12.5, weight: .semibold).monospacedDigit())
                                .foregroundStyle(N.text)
                            Text(model.glancePulls.count == 1 ? "open pull request" : "open pull requests")
                                .font(.system(size: 12))
                                .foregroundStyle(N.muted)
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    }
                    .buttonStyle(.plain)

                    if let run = model.runList.first {
                        Button {
                            model.section = .runs
                        } label: {
                            HStack(spacing: 9) {
                                StatusDot(color: runColor(run.state), size: 5.5, pulse: run.state == .running)
                                    .frame(width: 15)
                                Text(run.name)
                                    .font(.system(size: 12, weight: .medium))
                                    .foregroundStyle(N.text)
                                    .lineLimit(1)
                                Spacer(minLength: 4)
                                Text(run.state.label)
                                    .font(N.mono(10.5))
                                    .foregroundStyle(N.muted)
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                        .buttonStyle(.plain)
                        .help("Latest run on \(run.branch ?? run.shortSHA)")
                    }
                }
            }
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") {
            await model.refreshGlancePulls()
            await model.loadRuns()
        }
    }

    private func runColor(_ s: RunState) -> Color {
        switch s {
        case .success: N.mint
        case .failure: N.rose
        case .cancelled, .skipped: N.slate
        default: N.amber
        }
    }
}
