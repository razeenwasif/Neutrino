import NeutrinoKit
import SwiftUI

struct PullsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let problem = model.pullRepoProblem {
                EmptyState(title: "No pull requests here", message: problem).island()
            } else {
                HStack(alignment: .top, spacing: 10) {
                    PullList().frame(width: 340).frame(maxHeight: .infinity)
                    PullDetailPanel().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") { await model.loadPulls() }
    }
}

// MARK: - List

struct PullList: View {
    @Environment(AppModel.self) private var model
    @State private var creating = false
    @State private var editingRules = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("Pull requests").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
                Spacer(minLength: 6)
                Button {
                    editingRules = true
                } label: {
                    Image(systemName: "lock.shield").font(.system(size: 12, weight: .semibold))
                }
                .buttonStyle(PillButtonStyle(kind: .secondary, height: 26))
                .help("Branch rules")
                .accessibilityLabel("Branch rules")
                Button("New") { creating = true }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 26))
                    .help("Open a pull request")
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 8)
            HStack {
                Segmented(options: [(PullFilter.open, "Open \(count(.open))"), (.closed, "Closed \(count(.closed))")], selection: $model.pullFilter)
                Spacer()
            }
            .padding(.horizontal, 14)
            .padding(.bottom, 8)

            if let error = model.pullError, model.pullList.isEmpty {
                EmptyState(title: "Can't load pull requests", message: error)
            } else if model.visiblePulls.isEmpty, !model.loadingPulls {
                EmptyState(
                    title: model.pullFilter == .open ? "Nothing open" : "Nothing closed yet",
                    message: model.pullFilter == .open ? "Push a branch, then open a pull request to merge it." : "Merged and closed pull requests show up here."
                )
            } else {
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(model.visiblePulls) { p in
                            PullRow(pull: p, selected: p.number == model.pullNumber)
                        }
                    }
                    .padding(.horizontal, 8)
                    .padding(.bottom, 10)
                }
            }
        }
        .island()
        .onChange(of: model.pullFilter) {
            if let first = model.visiblePulls.first, !model.visiblePulls.contains(where: { $0.number == model.pullNumber }) {
                Task { await model.select(pull: first.number) }
            }
        }
        .sheet(isPresented: $creating) { NewPullSheet() }
        .sheet(isPresented: $editingRules) { RulesSheet() }
    }

    private func count(_ filter: PullFilter) -> Int {
        model.pullList.filter { filter == .open ? $0.state == .open : $0.state != .open }.count
    }
}

struct StateChip: View {
    var state: PullState

    var body: some View {
        let color: Color = switch state {
        case .open: N.ok
        case .merged: N.accent
        case .closed: N.danger
        }
        Text(state.label)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .frame(height: 19)
            .background(color.opacity(0.13), in: Capsule())
    }
}

struct BranchFlow: View {
    var source: String
    var target: String

    var body: some View {
        HStack(spacing: 5) {
            Text(source).foregroundStyle(N.accent).lineLimit(1).truncationMode(.middle)
            Image(systemName: "arrow.right").font(.system(size: 9, weight: .bold)).foregroundStyle(N.faint)
            Text(target).foregroundStyle(N.second).lineLimit(1)
        }
        .font(N.mono(11.5, weight: .medium))
    }
}

struct PullRow: View {
    @Environment(AppModel.self) private var model
    var pull: PullRequest
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        Button {
            Task { await model.select(pull: pull.number) }
        } label: {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 7) {
                    Text("#\(pull.number)").font(N.mono(11.5, weight: .semibold)).foregroundStyle(N.faint)
                    Text(pull.title).font(.system(size: 13, weight: .medium)).foregroundStyle(N.text).lineLimit(1)
                    Spacer(minLength: 0)
                }
                HStack(spacing: 8) {
                    BranchFlow(source: pull.source, target: pull.target)
                    Spacer(minLength: 4)
                    if pull.openNotes > 0 {
                        Label("\(pull.openNotes)", systemImage: "text.bubble").font(.system(size: 11)).foregroundStyle(N.warn)
                            .help("\(pull.openNotes) unresolved notes")
                    }
                    if pull.state != .open { StateChip(state: pull.state) }
                    Text(Format.ago(pull.updatedAt)).font(.system(size: 11)).foregroundStyle(N.faint)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? N.selected : (hovering ? Color.white.opacity(0.04) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

// MARK: - Detail

struct PullDetailPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Group {
            if let d = model.pullDetail {
                VStack(spacing: 0) {
                    PullHeader(detail: d)
                    HStack {
                        Segmented(options: [
                            (PullTab.overview, "Overview"),
                            (.files, "Files \(d.files.count)"),
                            (.commits, "Commits \(d.commits.count)"),
                        ], selection: $model.pullTab)
                        Spacer()
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 10)
                    Rectangle().fill(N.hairline).frame(height: 1)
                    switch model.pullTab {
                    case .overview: PullOverview(detail: d)
                    case .files: PullFiles(detail: d)
                    case .commits: PullCommits(detail: d)
                    }
                }
            } else if let error = model.pullError {
                EmptyState(title: "Can't show this pull request", message: error)
            } else {
                EmptyState(title: "No pull request selected", message: "Pick one from the list, or open a new one.")
            }
        }
        .island()
    }
}

struct PullHeader: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail
    @State private var editing = false

    var body: some View {
        let p = detail.pull
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(p.title).font(.system(size: 18, weight: .semibold)).foregroundStyle(N.text).lineLimit(2)
                Text("#\(p.number)").font(N.mono(15)).foregroundStyle(N.faint)
                Spacer(minLength: 8)
                if p.state != .merged {
                    Button("Edit") { editing = true }.buttonStyle(PillButtonStyle(kind: .quiet, height: 26))
                }
            }
            HStack(spacing: 10) {
                StateChip(state: p.state)
                BranchFlow(source: p.source, target: p.target)
                Text("·").foregroundStyle(N.faint)
                Text(p.versions == 1 ? "1 version" : "\(p.versions) versions").help("Each push to \(p.source) while it's open is a new version.")
                Text("·").foregroundStyle(N.faint)
                Text("opened \(Format.ago(p.createdAt))")
            }
            .font(.system(size: 12))
            .foregroundStyle(N.muted)
        }
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .sheet(isPresented: $editing) { EditPullSheet(pull: p) }
    }
}

// MARK: Overview

struct PullOverview: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if detail.pull.body.isEmpty {
                    Text("No description.").font(.system(size: 13)).foregroundStyle(N.faint).italic()
                } else {
                    Text(detail.pull.body).font(.system(size: 13)).foregroundStyle(N.body).textSelection(.enabled)
                }
                MergeBox(detail: detail)
                NotesList(detail: detail)
                VersionList(detail: detail)
            }
            .padding(16)
            .frame(maxWidth: 760, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct MergeBox: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail
    @AppStorage("mergeStrategy") private var strategyName = MergeStrategy.merge.rawValue
    @AppStorage("deleteSourceAfterMerge") private var deleteSource = true

    private var strategy: MergeStrategy { MergeStrategy(rawValue: strategyName) ?? .merge }

    var body: some View {
        let p = detail.pull
        VStack(alignment: .leading, spacing: 12) {
            switch p.state {
            case .merged:
                status(icon: "checkmark.circle.fill", color: N.accent, title: "Merged into \(p.target)",
                       detail: [p.mergeSha.map { "as \($0.prefix(7))" }, p.mergedAt.map { Format.ago($0) }].compactMap { $0 }.joined(separator: ", "))
            case .closed:
                status(icon: "xmark.circle.fill", color: N.danger, title: "Closed without merging", detail: "Reopen it to carry on.")
                Button("Reopen") { Task { await model.setPullState(.open) } }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                    .disabled(model.pullBusy != nil)
            case .open:
                checks
                Rectangle().fill(N.hairline).frame(height: 1)
                HStack(spacing: 10) {
                    Menu {
                        ForEach(MergeStrategy.allCases) { s in
                            Button {
                                strategyName = s.rawValue
                            } label: {
                                if s == strategy { Label(s.label, systemImage: "checkmark") } else { Text(s.label) }
                            }
                            .disabled(s == .fastForward && detail.mergeability?.fastForward != true)
                        }
                    } label: {
                        HStack(spacing: 6) {
                            Text(strategy.label)
                            Image(systemName: "chevron.down").font(.system(size: 9, weight: .bold))
                        }
                        .font(.system(size: 12, weight: .medium))
                        .padding(.horizontal, 12)
                        .frame(height: 30)
                        .background(N.control, in: Capsule())
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .fixedSize()
                    .help(strategy.detail)
                    Toggle("Delete \(p.source) afterwards", isOn: $deleteSource).toggleStyle(.checkbox).font(.system(size: 12))
                    Spacer()
                    Button("Close") { Task { await model.setPullState(.closed) } }
                        .buttonStyle(PillButtonStyle(kind: .quiet, height: 30))
                        .disabled(model.pullBusy != nil)
                    Button {
                        Task { await model.merge(strategy: strategy, deleteSource: deleteSource) }
                    } label: {
                        Text(model.pullBusy == "Merging" ? "Merging…" : "Merge")
                    }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                    .disabled(!canMerge || model.pullBusy != nil)
                    .help(mergeHelp)
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .well()
    }

    private var canMerge: Bool {
        guard let m = detail.mergeability else { return false }
        if strategy == .fastForward { return m.fastForward }
        return m.clean && detail.rule?.requiredChecks.isEmpty != false
    }

    private var mergeHelp: String {
        guard let m = detail.mergeability else { return "One of the branches is gone." }
        if strategy == .fastForward, !m.fastForward { return "\(detail.pull.target) has moved on, so it can't fast-forward." }
        if !m.clean { return "Resolve the conflicts first." }
        if let r = detail.rule, !r.requiredChecks.isEmpty { return "The rule needs checks, which arrive with CI." }
        return "\(strategy.label) \(detail.pull.source) into \(detail.pull.target). \(strategy.detail)"
    }

    @ViewBuilder private var checks: some View {
        let p = detail.pull
        if let m = detail.mergeability {
            if !m.clean {
                status(icon: "exclamationmark.triangle.fill", color: N.danger, title: "Conflicts with \(p.target)",
                       detail: "In \(m.conflicts.prefix(4).joined(separator: ", "))\(m.conflicts.count > 4 ? " and \(m.conflicts.count - 4) more" : ""). Merge \(p.target) into \(p.source), resolve them and push.")
            } else if m.fastForward {
                status(icon: "checkmark.circle.fill", color: N.ok, title: "Ready to merge", detail: "\(p.target) hasn't moved, so this can fast-forward.")
            } else {
                status(icon: "checkmark.circle.fill", color: N.ok, title: "Merges cleanly",
                       detail: "\(p.target) is \(m.behind) commit\(m.behind == 1 ? "" : "s") ahead of where \(p.source) started.")
            }
        } else {
            status(icon: "questionmark.circle", color: N.warn, title: "Can't check", detail: "\(p.source) or \(p.target) is gone from the server.")
        }
        if p.openNotes > 0 {
            status(icon: "text.bubble", color: N.warn, title: "\(p.openNotes) unresolved note\(p.openNotes == 1 ? "" : "s")", detail: "Notes don't block merging.")
        }
        if let r = detail.rule {
            status(icon: "lock.shield", color: N.second, title: "\(p.target) is guarded by \(r.pattern)",
                   detail: r.requiredChecks.isEmpty ? r.summary : "\(r.summary); needs checks \(r.requiredChecks.joined(separator: ", ")), which arrive with CI")
        }
    }

    private func status(icon: String, color: Color, title: String, detail: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).font(.system(size: 14)).foregroundStyle(color).frame(width: 18)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13, weight: .semibold)).foregroundStyle(N.text)
                if !detail.isEmpty { Text(detail).font(.system(size: 12)).foregroundStyle(N.muted) }
            }
        }
    }
}

struct NotesList: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail

    var body: some View {
        if !detail.notes.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                SectionLabel("Notes")
                ForEach(detail.notes.sorted { ($0.resolved ? 1 : 0, $0.id) < ($1.resolved ? 1 : 0, $1.id) }) { note in
                    NoteCard(note: note, showPlace: true)
                }
            }
        }
    }
}

struct NoteCard: View {
    @Environment(AppModel.self) private var model
    var note: Note
    var showPlace = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: note.resolved ? "checkmark.bubble" : "text.bubble")
                .foregroundStyle(note.resolved ? N.faint : N.warn)
                .font(.system(size: 13))
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    if showPlace {
                        Button {
                            model.pullTab = .files
                            Task { await model.select(pullFile: note.path) }
                        } label: {
                            Text("\(note.path):\(note.currentLine ?? note.line)").font(N.mono(11.5, weight: .medium)).foregroundStyle(N.second)
                        }
                        .buttonStyle(.plain)
                        .help("Show it in Files")
                    }
                    if note.outdated {
                        Text("outdated").font(.system(size: 10.5, weight: .semibold)).foregroundStyle(N.faint)
                            .padding(.horizontal, 6).frame(height: 17).background(Color.white.opacity(0.06), in: Capsule())
                            .help("The line changed after version \(note.version), where this was written.")
                    }
                    Text("v\(note.version) · \(Format.ago(note.createdAt))").font(.system(size: 11)).foregroundStyle(N.faint)
                }
                Text(note.text).font(.system(size: 12.5)).foregroundStyle(note.resolved ? N.muted : N.body).textSelection(.enabled)
            }
            Spacer(minLength: 8)
            Button(note.resolved ? "Reopen" : "Resolve") { Task { await model.resolve(note, !note.resolved) } }
                .buttonStyle(PillButtonStyle(kind: .secondary, height: 24))
                .font(.system(size: 11))
                .disabled(model.pullBusy != nil)
        }
        .padding(10)
        .background(N.raised.opacity(note.resolved ? 0.5 : 1), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

struct VersionList: View {
    var detail: PullDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionLabel("Versions")
            ForEach(detail.versions.reversed()) { v in
                HStack(spacing: 10) {
                    Text("v\(v.version)").font(N.mono(11.5, weight: .semibold)).foregroundStyle(v.version == detail.versions.last?.version ? N.accent : N.muted)
                        .frame(width: 30, alignment: .leading)
                    Text(String(v.headSha.prefix(7))).font(N.mono(11.5)).foregroundStyle(N.body)
                    Text("from \(v.baseSha.prefix(7))").font(N.mono(11)).foregroundStyle(N.faint)
                    Spacer()
                    Text(Format.ago(v.createdAt)).font(.system(size: 11)).foregroundStyle(N.faint)
                }
            }
        }
    }
}

// MARK: Files

struct PullFiles: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail
    @State private var composing: DiffLine?
    @State private var draft = ""

    var body: some View {
        HStack(spacing: 0) {
            ScrollView {
                VStack(spacing: 1) {
                    ForEach(detail.files) { f in
                        let notes = detail.notes.filter { $0.path == f.path && !$0.resolved }.count
                        Button {
                            composing = nil
                            Task { await model.select(pullFile: f.path) }
                        } label: {
                            HStack(spacing: 8) {
                                Text(f.status.letter).font(N.mono(11, weight: .semibold)).foregroundStyle(N.status(f.status)).frame(width: 12)
                                Text((f.path as NSString).lastPathComponent).font(.system(size: 12.5)).foregroundStyle(N.text).lineLimit(1)
                                Spacer(minLength: 4)
                                if notes > 0 { Image(systemName: "text.bubble").font(.system(size: 10)).foregroundStyle(N.warn) }
                                if !f.binary { StatCounts(additions: f.additions, deletions: f.deletions) }
                            }
                            .padding(.horizontal, 10)
                            .frame(height: 30)
                            .background(model.pullFile == f.path ? N.selected : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help(f.oldPath.map { "\($0) → \(f.path)" } ?? f.path)
                    }
                }
                .padding(8)
            }
            .frame(width: 220)
            Rectangle().fill(N.hairline).frame(width: 1)
            diff
        }
    }

    @ViewBuilder private var diff: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                if let d = model.pullDiff {
                    Text(d.path).font(N.mono(12.5, weight: .medium)).foregroundStyle(N.text).lineLimit(1).truncationMode(.head)
                    StatCounts(additions: d.additions, deletions: d.deletions)
                }
                // The hint gives way first when the panel is narrow; the layout switch never squeezes.
                ViewThatFits(in: .horizontal) {
                    Text("Click a line to leave a note").font(.system(size: 11)).foregroundStyle(N.faint).fixedSize()
                        .frame(maxWidth: .infinity, alignment: .trailing)
                    Spacer(minLength: 0)
                }
                Segmented(options: [(DiffLayout.unified, "Unified"), (.split, "Split")], selection: $model.layout)
                    .fixedSize()
                    .help("Click a line to leave a note on it")
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            Group {
                if let d = model.pullDiff {
                    if d.binary {
                        EmptyState(title: "Binary file", message: "Neutrino doesn't show binary changes yet.")
                    } else if d.tooLarge {
                        EmptyState(title: "Too large to show", message: "One side of this file is over 8 MB.")
                    } else if d.hunks.isEmpty {
                        EmptyState(title: "No line changes", message: d.oldPath != nil ? "Renamed without changes." : "Only the file's mode changed.")
                    } else {
                        DiffBody(diff: d, layout: model.layout, onPick: { line in
                            composing = line
                            draft = ""
                        }, pickContext: true, footer: { line in footer(for: line, path: d.path) })
                    }
                } else if let error = model.pullDiffError {
                    EmptyState(title: "Can't show this diff", message: error)
                } else if detail.files.isEmpty {
                    EmptyState(title: "No changes", message: "\(detail.pull.source) has nothing \(detail.pull.target) doesn't.")
                } else {
                    Color.clear
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.well)
        }
    }

    private func footer(for line: DiffLine, path: String) -> AnyView? {
        let notes = model.notes(on: line, path: path)
        let isComposing = composing == line
        guard !notes.isEmpty || isComposing else { return nil }
        return AnyView(
            VStack(alignment: .leading, spacing: 6) {
                ForEach(notes) { NoteCard(note: $0) }
                if isComposing {
                    NoteComposer(text: $draft, saving: model.pullBusy == "Saving") {
                        let text = draft
                        Task {
                            await model.addNote(on: line, path: path, text: text)
                            composing = nil
                        }
                    } cancel: {
                        composing = nil
                    }
                }
            }
            .padding(.leading, 112)
            .padding(.trailing, 14)
            .padding(.vertical, 6)
            .frame(maxWidth: 820, alignment: .leading)
            .textSelection(.enabled)
        )
    }
}

struct NoteComposer: View {
    @Binding var text: String
    var saving: Bool
    var save: () -> Void
    var cancel: () -> Void
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextEditor(text: $text)
                .font(.system(size: 12.5))
                .scrollContentBackground(.hidden)
                .focused($focused)
                .frame(minHeight: 54, maxHeight: 120)
                .padding(6)
                .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            HStack {
                Text("⌘↩ to save").font(.system(size: 11)).foregroundStyle(N.faint)
                Spacer()
                Button("Cancel", action: cancel).buttonStyle(PillButtonStyle(kind: .quiet, height: 26)).keyboardShortcut(.cancelAction)
                Button(saving ? "Saving…" : "Leave note", action: save)
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 26))
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || saving)
            }
        }
        .padding(10)
        .background(N.island, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .onAppear { focused = true }
    }
}

// MARK: Commits

struct PullCommits: View {
    var detail: PullDetail

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(detail.commits) { c in
                    HStack(spacing: 12) {
                        Text(c.shortSHA).font(N.mono(12, weight: .medium)).foregroundStyle(N.accent)
                        Text(c.subject).font(.system(size: 13)).foregroundStyle(N.text).lineLimit(1)
                        Spacer(minLength: 8)
                        Text(c.author.name).font(.system(size: 12)).foregroundStyle(N.muted).lineLimit(1)
                        Text(Format.ago(c.time)).font(.system(size: 11.5)).foregroundStyle(N.faint).frame(width: 80, alignment: .trailing)
                    }
                    .padding(.horizontal, 16)
                    .frame(height: 36)
                    .textSelection(.enabled)
                    Rectangle().fill(N.hairline).frame(height: 1).padding(.leading, 16)
                }
                if detail.commits.count >= 250 {
                    Text("Showing the first 250 commits.").font(.system(size: 12)).foregroundStyle(N.faint).padding(16)
                }
            }
        }
    }
}

// MARK: - Sheets

private struct Field<Content: View>: View {
    var label: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.system(size: 11)).foregroundStyle(N.muted)
            content
        }
    }
}

private extension View {
    func sheetField() -> some View {
        textFieldStyle(.plain)
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
    }
}

struct NewPullSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var branches: [Branch] = []
    @State private var source = ""
    @State private var target = ""
    @State private var title = ""
    @State private var bodyText = ""
    @State private var working = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New pull request").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            HStack(spacing: 10) {
                Field(label: "Merge") { picker($source) }
                Image(systemName: "arrow.right").foregroundStyle(N.faint).padding(.top, 16)
                Field(label: "Into") { picker($target) }
            }
            Field(label: "Title") {
                TextField("What this changes", text: $title).font(.system(size: 13)).sheetField()
            }
            Field(label: "Description") {
                TextEditor(text: $bodyText)
                    .font(.system(size: 12.5))
                    .scrollContentBackground(.hidden)
                    .frame(height: 110)
                    .padding(6)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            if let error {
                Text(error).font(.system(size: 12)).foregroundStyle(N.danger).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Text("Branches on \(model.serverLink?.name ?? "the server"). Push yours first.").font(.system(size: 11)).foregroundStyle(N.faint)
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button(working ? "Opening…" : "Open pull request", action: create)
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                    .keyboardShortcut(.defaultAction)
                    .disabled(working || source.isEmpty || target.isEmpty || source == target || title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(N.island)
        .task {
            branches = await model.hostedBranches()
            let main = model.hostedDefaultBranch ?? "main"
            target = branches.first { $0.name == main }?.name ?? branches.first?.name ?? ""
            // Start from the most recently pushed branch that isn't the target.
            let candidates = branches.filter { $0.name != target }.sorted { $0.time > $1.time }
            let current = model.source == .local ? model.repo?.head?.label : nil
            source = candidates.first { $0.name == current }?.name ?? candidates.first?.name ?? ""
            if title.isEmpty { title = branches.first { $0.name == source }?.subject ?? "" }
        }
        .onChange(of: source) { _, new in
            if let subject = branches.first(where: { $0.name == new })?.subject, title.isEmpty || branches.contains(where: { $0.subject == title }) {
                title = subject
            }
        }
    }

    private func picker(_ selection: Binding<String>) -> some View {
        Picker("", selection: selection) {
            ForEach(branches) { b in Text(b.name).tag(b.name) }
        }
        .labelsHidden()
        .frame(width: 210)
    }

    private func create() {
        working = true
        error = nil
        Task {
            defer { working = false }
            do {
                try await model.createPull(title: title, body: bodyText, source: source, target: target)
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}

struct EditPullSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    var pull: PullRequest
    @State private var title = ""
    @State private var bodyText = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Edit #\(pull.number)").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            Field(label: "Title") { TextField("Title", text: $title).font(.system(size: 13)).sheetField() }
            Field(label: "Description") {
                TextEditor(text: $bodyText)
                    .font(.system(size: 12.5))
                    .scrollContentBackground(.hidden)
                    .frame(height: 140)
                    .padding(6)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button("Save") {
                    Task {
                        await model.editPull(title: title, body: bodyText)
                        dismiss()
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                .keyboardShortcut(.defaultAction)
                .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 520)
        .background(N.island)
        .onAppear {
            title = pull.title
            bodyText = pull.body
        }
    }
}

struct RulesSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var rules: [BranchRule] = []
    @State private var draft = BranchRule(pattern: "")
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Branch rules").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            Text("Rules guard branches on \(model.serverLink?.name ?? "the server") when people push. A push can still override one with `git push -o neutrino-override=\"why\"`; overrides are recorded.")
                .font(.system(size: 12)).foregroundStyle(N.muted).fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 6) {
                if rules.isEmpty {
                    Text("No rules yet.").font(.system(size: 12)).foregroundStyle(N.faint).frame(maxWidth: .infinity, alignment: .leading)
                }
                ForEach(rules) { r in
                    HStack {
                        Text(r.pattern).font(N.mono(12.5, weight: .semibold)).foregroundStyle(N.text)
                        Text(r.summary).font(.system(size: 12)).foregroundStyle(N.muted)
                        Spacer()
                        Button("Edit") { draft = r }.buttonStyle(PillButtonStyle(kind: .quiet, height: 24))
                        Button("Remove") { Task { await run { try await model.deleteRule(r.pattern) } } }
                            .buttonStyle(PillButtonStyle(kind: .secondary, height: 24))
                    }
                    .padding(.horizontal, 10)
                    .frame(height: 34)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                }
            }
            Rectangle().fill(N.hairline).frame(height: 1)
            Field(label: "Branch or pattern") {
                TextField("main, release/*", text: $draft.pattern).font(N.mono(13)).sheetField()
            }
            HStack(spacing: 16) {
                Toggle("Pull requests only", isOn: $draft.requirePull)
                Toggle("No force-push", isOn: $draft.blockForce)
                Toggle("No delete", isOn: $draft.blockDelete)
            }
            .toggleStyle(.checkbox)
            .font(.system(size: 12.5))
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.danger) }
            HStack {
                Spacer()
                Button("Done") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button(rules.contains { $0.pattern == draft.pattern } ? "Update rule" : "Add rule") {
                    let rule = draft
                    Task {
                        await run { try await model.setRule(rule) }
                        if error == nil { draft = BranchRule(pattern: "") }
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                .disabled(draft.pattern.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 560)
        .background(N.island)
        .task { rules = await model.loadRules() }
    }

    private func run(_ body: () async throws -> [BranchRule]) async {
        do {
            rules = try await body()
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}
