import AppKit
import NeutrinoKit
import SwiftUI

/// A repo's home: its files at a branch, the README, and what the repo is
/// made of, the way a forge shows a repository.
struct CodeScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        HStack(alignment: .top, spacing: 10) {
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    CodeToolbar()
                    if code.file != nil || !code.path.isEmpty { Breadcrumb() }
                    if code.file != nil {
                        FileView()
                    } else {
                        FileTable()
                        if let readme = code.readme { ReadmeCard(blob: readme) }
                    }
                }
                .padding(.bottom, 10)
            }
            .scrollIndicators(.automatic)
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            AboutPanel()
                .frame(width: 276)
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? "")") {
            code.reset(for: "\(model.source.rawValue):\(model.repoID ?? "")")
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(code.rev ?? ""):\(code.path):\(model.isServerOnline)") {
            guard let repo = model.repoID else { return }
            await code.load(model.activeService, repo: repo)
        }
    }
}

// MARK: - Toolbar

private struct CodeToolbar: View {
    @Environment(AppModel.self) private var model
    @FocusState private var filterFocused: Bool

    var body: some View {
        @Bindable var code = model.code
        let branches = model.branches.filter { $0.kind == .local }
        let tags = model.branches.filter { $0.kind == .tag }
        HStack(spacing: 10) {
            RefPicker()
            Button {
                model.section = .branches
            } label: {
                Label("\(branches.count) \(branches.count == 1 ? "branch" : "branches")", systemImage: "arrow.triangle.branch")
            }
            .buttonStyle(MetaButtonStyle())
            .help("Show the branches")
            Label("\(tags.count) \(tags.count == 1 ? "tag" : "tags")", systemImage: "tag")
                .font(.system(size: 12))
                .foregroundStyle(N.muted)
            Spacer(minLength: 8)
            if code.file == nil {
                HStack(spacing: 7) {
                    Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(N.faint)
                    TextField("Go to file", text: $code.filter)
                        .textFieldStyle(.plain)
                        .font(.system(size: 12.5))
                        .focused($filterFocused)
                        .frame(width: 150)
                        .onKeyPress(.escape) {
                            code.filter = ""
                            return .handled
                        }
                }
                .padding(.horizontal, 10)
                .frame(height: 28)
                .background(N.well, in: Capsule())
            }
            CloneMenu()
        }
        .padding(.horizontal, 2)
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
            HStack(spacing: 7) {
                Image(systemName: "arrow.triangle.branch").font(.system(size: 11, weight: .semibold)).foregroundStyle(N.accent)
                Text(label).font(N.mono(12, weight: .medium)).foregroundStyle(N.text).lineLimit(1)
                Image(systemName: "chevron.down").font(.system(size: 8.5, weight: .bold)).foregroundStyle(N.muted)
            }
            .padding(.horizontal, 12)
            .frame(height: 30)
            .background(N.island, in: Capsule())
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
                    Button("Copy Clone URL") { model.copy(link.pushURL(for: repo.path)) }
                    Button("Copy git clone Command") { model.copy("git clone \(link.pushURL(for: repo.path))") }
                } else {
                    Button("Copy Path") { model.copy(repo.path) }
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
            .frame(height: 28)
            .background(N.accent, in: Capsule())
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
        HStack(spacing: 4) {
            Button(model.repo?.name ?? "root") { code.open(dir: "") }
                .buttonStyle(CrumbStyle(current: false))
            ForEach(Array(code.crumbs.enumerated()), id: \.offset) { i, crumb in
                Text("/").font(N.mono(13)).foregroundStyle(N.faint)
                let last = i == code.crumbs.count - 1
                if last {
                    Text(crumb.name).font(N.mono(13, weight: .semibold)).foregroundStyle(N.text)
                } else {
                    Button(crumb.name) { code.open(dir: crumb.path) }.buttonStyle(CrumbStyle(current: false))
                }
            }
            Spacer()
            if let path = code.file ?? (code.path.isEmpty ? nil : code.path) {
                Button {
                    model.copy(path)
                } label: {
                    Image(systemName: "doc.on.doc").font(.system(size: 11))
                }
                .buttonStyle(.plain)
                .foregroundStyle(N.faint)
                .help("Copy the path")
                .accessibilityLabel("Copy path")
            }
        }
        .padding(.horizontal, 4)
        .lineLimit(1)
    }
}

private struct CrumbStyle: ButtonStyle {
    var current: Bool
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(N.mono(13, weight: .medium))
            .foregroundStyle(N.accent)
            .opacity(configuration.isPressed ? 0.7 : 1)
            .contentShape(Rectangle())
    }
}

private struct MetaButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 12))
            .foregroundStyle(N.muted)
            .opacity(configuration.isPressed ? 0.7 : 1)
            .contentShape(Rectangle())
    }
}

// MARK: - Files

private struct FileTable: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        VStack(spacing: 0) {
            LatestCommitBar()
            Rectangle().fill(N.hairline).frame(height: 1)
            if let error = code.listingError {
                EmptyState(title: "Can't list these files", message: error).frame(minHeight: 160)
            } else if code.listing == nil {
                HStack { Spacer(); ProgressView().controlSize(.small); Spacer() }.frame(height: 120)
            } else {
                if !code.path.isEmpty, code.filter.isEmpty {
                    EntryRowButton(action: { code.up() }) {
                        Image(systemName: "arrow.turn.left.up").font(.system(size: 12)).foregroundStyle(N.faint).frame(width: 18)
                        Text("..").font(N.mono(13)).foregroundStyle(N.muted)
                        Spacer()
                    }
                    .accessibilityLabel("Parent directory")
                }
                let entries = code.visibleEntries
                if entries.isEmpty {
                    Text(code.filter.isEmpty ? "This directory is empty." : "No file here matches “\(code.filter)”.")
                        .font(.system(size: 12.5)).foregroundStyle(N.faint)
                        .frame(maxWidth: .infinity, minHeight: 60)
                }
                ForEach(entries) { entry in
                    EntryRow(entry: entry, commit: code.entryCommits[entry.name])
                }
            }
        }
        .island(radius: 14)
    }
}

private struct LatestCommitBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        HStack(spacing: 10) {
            if let c = code.latest {
                Avatar(name: c.author.name)
                Text(c.author.name).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(N.text).lineLimit(1).fixedSize()
                Button {
                    showCommit(c.sha)
                } label: {
                    Text(c.subject).font(.system(size: 12.5)).foregroundStyle(N.muted).lineLimit(1)
                }
                .buttonStyle(.plain)
                .help(c.subject)
                Spacer(minLength: 8)
                Text(c.shortSHA).font(N.mono(11.5)).foregroundStyle(N.faint)
                Text("·").foregroundStyle(N.faint)
                Text(Format.ago(c.time)).font(.system(size: 12)).foregroundStyle(N.faint).fixedSize().help(Format.full(c.time))
            } else {
                Text(code.loadingListing ? "Reading…" : "No commits").font(.system(size: 12.5)).foregroundStyle(N.faint)
                Spacer()
            }
            if let o = code.overview {
                Button {
                    model.rev = code.rev
                    model.scope = .current
                    model.section = .history
                    Task { await model.reloadHistory() }
                } label: {
                    Label("\(o.commits.formatted())\(o.commitsCapped ? "+" : "") commits", systemImage: "clock.arrow.circlepath")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(N.body)
                        .padding(.horizontal, 10)
                        .frame(height: 24)
                        .background(N.control, in: Capsule())
                }
                .buttonStyle(.plain)
                .help("Open the history")
                .fixedSize()
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 48)
        .background(N.raised.opacity(0.6))
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

    var body: some View {
        EntryRowButton {
            guard let repo = model.repoID else { return }
            Task { await model.code.open(entry, model.activeService, repo: repo) }
        } content: {
            Image(systemName: EntryIcon.symbol(entry))
                .font(.system(size: 13))
                .foregroundStyle(entry.kind.isFolder ? N.accent : N.muted)
                .frame(width: 18)
            Text(entry.name)
                .font(.system(size: 13, weight: entry.kind.isFolder ? .medium : .regular))
                .foregroundStyle(entry.kind == .submodule ? N.second : N.text)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(width: 230, alignment: .leading)
            Text(commit?.subject ?? "").font(.system(size: 12.5)).foregroundStyle(N.faint).lineLimit(1).help(commit?.subject ?? "")
            Spacer(minLength: 8)
            Text(entry.size.flatMap { entry.kind == .symlink ? nil : ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .file) } ?? "")
                .font(N.mono(11)).foregroundStyle(N.faint.opacity(0.8))
                .frame(width: 70, alignment: .trailing)
            Text(commit.map { Format.ago($0.time) } ?? "")
                .font(.system(size: 12)).foregroundStyle(N.faint)
                .frame(width: 84, alignment: .trailing)
                .help(commit.map { Format.full($0.time) } ?? "")
        }
        .disabled(entry.kind == .submodule)
        .help(entry.kind == .submodule ? "A submodule: another repository at a pinned commit" : entry.path)
        .contextMenu {
            Button("Copy Path") { model.copy(entry.path) }
            if let commit { Button("Copy Last Commit SHA") { model.copy(commit.sha) } }
        }
    }
}

private struct EntryRowButton<Content: View>: View {
    var action: () -> Void
    @ViewBuilder var content: Content
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) { content }
                .padding(.horizontal, 14)
                .frame(height: 38)
                .background(hovering ? Color.white.opacity(0.035) : .clear)
                .overlay(alignment: .bottom) { Rectangle().fill(N.hairline).frame(height: 1) }
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
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
}

// MARK: - README

private struct ReadmeCard: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL
    var blob: BlobContent

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "book.closed").font(.system(size: 12)).foregroundStyle(N.muted)
                Text(blob.path.split(separator: "/").last.map(String.init) ?? blob.path)
                    .font(.system(size: 12.5, weight: .semibold)).foregroundStyle(N.text)
                Spacer()
            }
            .padding(.horizontal, 16)
            .frame(height: 42)
            .background(N.raised.opacity(0.6))
            Rectangle().fill(N.hairline).frame(height: 1)
            Group {
                if let text = blob.text {
                    if CodeModel.isMarkdown(blob.path) {
                        MarkdownView(text: text)
                    } else {
                        Text(text).font(N.mono(12.5)).foregroundStyle(N.body).textSelection(.enabled)
                    }
                } else {
                    Text("This README is too large to show.").font(.system(size: 12.5)).foregroundStyle(N.faint)
                }
            }
            .padding(.horizontal, 22)
            .padding(.vertical, 18)
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
        .island(radius: 14)
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
                    Text(c.subject).font(.system(size: 12.5)).foregroundStyle(N.muted).lineLimit(1)
                    Text(String(c.sha.prefix(7))).font(N.mono(11.5)).foregroundStyle(N.faint)
                    Text(Format.ago(c.time)).font(.system(size: 12)).foregroundStyle(N.faint)
                }
                Spacer(minLength: 8)
                if let b = code.blob {
                    Text(stats(b)).font(N.mono(11.5)).foregroundStyle(N.faint).fixedSize()
                    if CodeModel.isMarkdown(b.path), b.text != nil {
                        Segmented(options: [(false, "Preview"), (true, "Code")], selection: $code.showSource)
                    }
                    if let text = b.text {
                        Button {
                            model.copy(text)
                        } label: {
                            Label("Copy", systemImage: "doc.on.doc")
                        }
                        .buttonStyle(PillButtonStyle(height: 26))
                        .help("Copy the file's contents")
                    }
                }
            }
            .padding(.horizontal, 14)
            .frame(height: 46)
            .background(N.raised.opacity(0.6))
            Rectangle().fill(N.hairline).frame(height: 1)

            if let error = code.blobError {
                EmptyState(title: "Can't open this file", message: error).frame(minHeight: 200)
            } else if let b = code.blob {
                if b.binary {
                    EmptyState(title: "Binary file", message: "\(ByteCountFormatter.string(fromByteCount: Int64(b.size), countStyle: .file)), not shown.").frame(minHeight: 200)
                } else if b.tooLarge || b.text == nil {
                    EmptyState(title: "Too large to show", message: "\(ByteCountFormatter.string(fromByteCount: Int64(b.size), countStyle: .file)). Open it from a clone instead.").frame(minHeight: 200)
                } else if CodeModel.isMarkdown(b.path), !code.showSource {
                    MarkdownView(text: b.text ?? "").padding(.horizontal, 22).padding(.vertical, 18)
                } else {
                    CodeLines(text: b.text ?? "")
                }
            } else {
                HStack { Spacer(); ProgressView().controlSize(.small); Spacer() }.frame(height: 160)
            }
        }
        .island(radius: 14)
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
        let gutter = CGFloat(String(shown.count).count) * 7.5 + 22
        ScrollView(.horizontal) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(Array(shown.enumerated()), id: \.offset) { i, line in
                    HStack(spacing: 0) {
                        Text("\(i + 1)")
                            .font(N.mono(12))
                            .foregroundStyle(N.faint.opacity(0.7))
                            .frame(width: gutter, alignment: .trailing)
                            .padding(.trailing, 14)
                        Text(line.isEmpty ? " " : String(line).replacingOccurrences(of: "\t", with: "    "))
                            .font(N.mono(12))
                            .foregroundStyle(N.body)
                            .fixedSize()
                    }
                    .frame(height: 19)
                }
            }
            .padding(.vertical, 10)
            .padding(.trailing, 16)
            .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

// MARK: - About

private struct AboutPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let code = model.code
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 6) {
                    SectionLabel("About")
                    if let repo = model.repo {
                        Text(repo.name).font(.system(size: 16, weight: .semibold)).foregroundStyle(N.text)
                        HStack(spacing: 6) {
                            Dot(color: model.source == .server ? N.atlas : N.ok, glow: false, size: 6)
                            Text(model.source == .server ? "Hosted on \(model.serverLink?.name ?? "the server")" : "Working copy on this Mac")
                                .font(.system(size: 12)).foregroundStyle(N.muted)
                        }
                        Text(repo.path).font(N.mono(10.5)).foregroundStyle(N.faint).lineLimit(2).truncationMode(.middle).textSelection(.enabled)
                    }
                }

                if let o = code.overview {
                    VStack(alignment: .leading, spacing: 8) {
                        stat("clock.arrow.circlepath", "\(o.commits.formatted())\(o.commitsCapped ? "+" : "")", o.commits == 1 ? "commit" : "commits")
                        stat("arrow.triangle.branch", "\(model.branches.filter { $0.kind == .local }.count)", "branches")
                        stat("tag", "\(model.branches.filter { $0.kind == .tag }.count)", "tags")
                        stat("person.2", "\(o.contributors)", o.contributors == 1 ? "contributor" : "contributors")
                        stat("doc.on.doc", o.files.formatted(), o.files == 1 ? "file" : "files")
                        if let license = o.license {
                            Button {
                                guard let repo = model.repoID else { return }
                                Task { await code.open(file: license, model.activeService, repo: repo) }
                            } label: {
                                stat("scroll", license, "")
                            }
                            .buttonStyle(.plain)
                            .help("Open the licence")
                        }
                    }
                }

                if let o = code.overview, !o.languages.isEmpty {
                    Languages(languages: o.languages)
                }

                if model.serverLink != nil {
                    ServerActivity()
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .scrollIndicators(.never)
        .island()
    }

    private func stat(_ icon: String, _ value: String, _ label: String) -> some View {
        HStack(spacing: 9) {
            Image(systemName: icon).font(.system(size: 11.5)).foregroundStyle(N.faint).frame(width: 16)
            Text(value).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(N.text).lineLimit(1)
            Text(label).font(.system(size: 12.5)).foregroundStyle(N.muted)
            Spacer(minLength: 0)
        }
    }
}

private struct Languages: View {
    var languages: [LanguageShare]

    var body: some View {
        let total = max(1, languages.reduce(0) { $0 + $1.bytes })
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel("Languages")
            GeometryReader { geo in
                HStack(spacing: 2) {
                    ForEach(Array(languages.enumerated()), id: \.offset) { i, l in
                        color(i).frame(width: max(3, (geo.size.width - CGFloat(languages.count - 1) * 2) * CGFloat(l.bytes) / CGFloat(total)))
                    }
                }
            }
            .frame(height: 8)
            .clipShape(Capsule())
            VStack(alignment: .leading, spacing: 5) {
                ForEach(Array(languages.enumerated()), id: \.offset) { i, l in
                    HStack(spacing: 7) {
                        Circle().fill(color(i)).frame(width: 8, height: 8)
                        Text(l.name).font(.system(size: 12, weight: .medium)).foregroundStyle(N.body)
                        Text(percent(l.bytes, total)).font(N.mono(11)).foregroundStyle(N.faint)
                        Spacer(minLength: 0)
                    }
                }
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func color(_ i: Int) -> Color { languages[i].name == "Other" ? N.faint : N.lanes[i % N.lanes.count] }

    private func percent(_ bytes: UInt64, _ total: UInt64) -> String {
        let p = Double(bytes) / Double(total) * 100
        return p < 0.1 ? "<0.1%" : String(format: "%.1f%%", p)
    }
}

/// Open pull requests and the newest CI run, from the server.
private struct ServerActivity: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("On \(model.serverLink?.name ?? "the server")")
            if !model.isServerOnline {
                Text("Offline.").font(.system(size: 12)).foregroundStyle(N.faint)
            } else if model.glancePullsNote != nil, model.glancePulls.isEmpty {
                Text(model.glancePullsNote ?? "").font(.system(size: 12)).foregroundStyle(N.faint).fixedSize(horizontal: false, vertical: true)
            } else {
                Button {
                    model.section = .pulls
                } label: {
                    HStack(spacing: 9) {
                        Image(systemName: "arrow.triangle.pull").font(.system(size: 11.5)).foregroundStyle(N.faint).frame(width: 16)
                        Text("\(model.glancePulls.count)").font(.system(size: 12.5, weight: .semibold)).foregroundStyle(N.text)
                        Text(model.glancePulls.count == 1 ? "open pull request" : "open pull requests").font(.system(size: 12.5)).foregroundStyle(N.muted)
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                if let run = model.runList.first {
                    Button {
                        model.section = .runs
                    } label: {
                        HStack(spacing: 9) {
                            Dot(color: runColor(run.state), glow: false, size: 7).frame(width: 16)
                            Text(run.name).font(.system(size: 12.5, weight: .medium)).foregroundStyle(N.text).lineLimit(1)
                            Text(run.state.label).font(.system(size: 12)).foregroundStyle(N.muted)
                            Spacer(minLength: 0)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("Latest run on \(run.branch ?? run.shortSHA)")
                }
            }
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") {
            await model.refreshGlancePulls()
            if model.runList.isEmpty { await model.loadRuns() }
        }
    }

    private func runColor(_ s: RunState) -> Color {
        switch s {
        case .success: N.ok
        case .failure: N.danger
        case .cancelled, .skipped: N.faint
        default: N.warn
        }
    }
}
