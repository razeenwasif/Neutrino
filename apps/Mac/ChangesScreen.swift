import NeutrinoKit
import SwiftUI

struct ChangesScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            ChangeList().frame(width: 360).frame(maxHeight: .infinity)
            WorkDiffPanel().frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

// MARK: - The list

struct ChangeList: View {
    @Environment(AppModel.self) private var model
    @State private var discarding: StatusFile?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("Changes").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
                Spacer()
                if !model.notStaged.isEmpty || !(model.status?.conflicted.isEmpty ?? true) {
                    Button("Stage all") { Task { await model.stageAll() } }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 26))
                        .disabled(model.writing)
                } else if !(model.status?.staged.isEmpty ?? true) {
                    Button("Unstage all") { Task { await model.unstageAll() } }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 26))
                        .disabled(model.writing)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 8)

            if let op = model.status?.operation {
                Text("A \(op) is in progress. Resolve conflicts, stage the files, then commit (or finish it in a terminal).")
                    .font(.system(size: 12))
                    .foregroundStyle(N.warn)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(N.warn.opacity(0.1), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .padding(.horizontal, 10)
                    .padding(.bottom, 6)
            }

            if let error = model.statusError {
                EmptyState(title: "Can't read the working copy", message: error)
            } else if model.repo?.bare == true {
                EmptyState(title: "No working copy", message: "This is a bare repository, so there's nothing to stage.")
            } else if let s = model.status {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 1) {
                        if !s.conflicted.isEmpty {
                            group("Conflicted · \(s.conflicted.count)")
                            ForEach(s.conflicted) { f in row(f, staged: false) }
                        }
                        group("Staged · \(s.staged.count)")
                        if s.staged.isEmpty {
                            hint(s.isClean ? "Nothing to commit." : "Nothing staged. Tick a file, or stage lines from its diff.")
                        }
                        ForEach(s.staged) { f in row(f, staged: true) }
                        group("Not staged · \(model.notStaged.count)")
                        if model.notStaged.isEmpty {
                            hint(s.isClean ? "The working tree matches HEAD." : "Everything that changed is staged.")
                        }
                        ForEach(model.notStaged) { f in row(f, staged: false) }
                    }
                    .padding(.horizontal, 8)
                    .padding(.bottom, 8)
                }
            } else {
                Spacer()
            }
            CommitBox()
        }
        .island()
        .confirmationDialog(
            discarding.map { "Discard changes to \(($0.path as NSString).lastPathComponent)?" } ?? "",
            isPresented: Binding(get: { discarding != nil }, set: { if !$0 { discarding = nil } }),
            presenting: discarding
        ) { f in
            Button(f.status == .untracked ? "Move to Trash" : "Discard changes", role: .destructive) {
                Task { await model.discard(paths: [f.path]) }
            }
        } message: { f in
            Text(f.status == .untracked
                ? "The file goes to the Trash. A copy is also kept in the repository and listed in ~/.neutrino/discarded.log."
                : "The file goes back to its staged version. A copy of your changes is kept in the repository and listed in ~/.neutrino/discarded.log.")
        }
    }

    private func group(_ title: String) -> some View {
        SectionLabel(title).padding(.horizontal, 8).padding(.top, 12).padding(.bottom, 6)
    }

    private func hint(_ text: String) -> some View {
        Text(text).font(.system(size: 12)).foregroundStyle(N.muted).padding(.horizontal, 8).padding(.bottom, 6)
    }

    private func row(_ f: StatusFile, staged: Bool) -> some View {
        let area: Area = staged ? .staged : .unstaged
        return ChangeRow(file: f, staged: staged, selected: model.change == ChangeRef(path: f.path, area: area))
            .onTapGesture { Task { await model.select(change: ChangeRef(path: f.path, area: area)) } }
            .contextMenu {
                Button(staged ? "Unstage" : "Stage") { Task { await model.toggle(f, staged: staged) } }
                if !staged, f.status != .conflicted {
                    Button(f.status == .untracked ? "Move to Trash…" : "Discard changes…") { discarding = f }
                }
                Divider()
                Button("Copy path") { model.copy(f.path) }
                if let root = model.repo?.path {
                    Button("Reveal in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: root).appendingPathComponent(f.path)])
                    }
                }
            }
    }
}

struct ChangeRow: View {
    @Environment(AppModel.self) private var model
    var file: StatusFile
    var staged: Bool
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            Button {
                Task { await model.toggle(file, staged: staged) }
            } label: {
                ZStack {
                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                        .fill(staged ? N.accent : .clear)
                        .overlay {
                            if !staged { RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(Color(hex: 0x6F6880), lineWidth: 1.5) }
                        }
                    if staged {
                        Image(systemName: "checkmark").font(.system(size: 9, weight: .heavy)).foregroundStyle(N.onAccent)
                    }
                }
                .frame(width: 15, height: 15)
                .frame(width: 26, height: 30)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(model.writing)
            .accessibilityLabel(staged ? "Unstage \(file.path)" : "Stage \(file.path)")

            Text(file.status.letter)
                .font(N.mono(10.5, weight: .semibold))
                .foregroundStyle(color)
                .frame(width: 10)
                .help(file.status.label)
            (Text(name).foregroundStyle(N.text) + Text("  " + dir).foregroundStyle(N.faint).font(.system(size: 11.5)))
                .font(.system(size: 12.5))
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 4)
            if file.binary {
                Text("binary").font(.system(size: 11)).foregroundStyle(N.faint)
            } else if file.status != .conflicted {
                StatCounts(additions: file.additions, deletions: file.deletions)
            }
        }
        .padding(.trailing, 8)
        .frame(height: 32)
        .background(selected ? N.selected : (hovering ? Color.white.opacity(0.035) : .clear), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .help(file.oldPath.map { "\($0) → \(file.path)" } ?? file.path)
    }

    private var color: Color {
        switch file.status {
        case .added, .untracked: N.ok
        case .deleted, .conflicted: N.danger
        default: N.warn
        }
    }

    private var name: String { (file.path as NSString).lastPathComponent }
    private var dir: String {
        let d = (file.path as NSString).deletingLastPathComponent
        if let old = file.oldPath { return "from \(old)" }
        return d.isEmpty ? "" : d + "/"
    }
}

// MARK: - Commit box

struct CommitBox: View {
    @Environment(AppModel.self) private var model
    @FocusState private var summaryFocused: Bool
    @State private var showOutput = false

    var body: some View {
        @Bindable var model = model
        VStack(alignment: .leading, spacing: 8) {
            if let last = model.lastCommit {
                HStack(spacing: 8) {
                    Image(systemName: "checkmark").font(.system(size: 11, weight: .bold))
                    Text("Committed \(String(last.sha.prefix(7)))")
                    Spacer()
                    if !last.output.isEmpty {
                        Button(showOutput ? "Hide output" : "Hooks said…") { showOutput.toggle() }.buttonStyle(.plain).foregroundStyle(N.muted)
                    }
                    Button {
                        model.lastCommit = nil
                    } label: {
                        Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss")
                }
                .font(.system(size: 12))
                .foregroundStyle(N.ok)
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(N.ok.opacity(0.1), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                if showOutput {
                    ScrollView {
                        Text(last.output).font(N.mono(11)).foregroundStyle(N.body).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 110)
                    .padding(8)
                    .well(radius: 9)
                }
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("Summary").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("What changed", text: $model.commitSummary)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .focused($summaryFocused)
                    .onSubmit { Task { await model.commit() } }
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Description").font(.system(size: 11)).foregroundStyle(N.muted)
                TextEditor(text: $model.commitBody)
                    .font(.system(size: 12.5))
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 5)
                    .frame(height: 64)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            HStack(spacing: 8) {
                Toggle(isOn: Binding(get: { model.amend }, set: { $0 ? model.startAmend() : (model.amend = false) })) {
                    Text("Amend last commit").font(.system(size: 12)).foregroundStyle(N.muted)
                }
                .toggleStyle(.checkbox)
                .disabled(model.repo?.head?.sha == nil)
                Spacer()
            }
            Button {
                Task { await model.commit() }
            } label: {
                HStack(spacing: 6) {
                    if model.committing { ProgressView().controlSize(.small) }
                    Text(commitTitle)
                    if let branch = branchName { Text(branch).font(N.mono(12, weight: .semibold)) }
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(PillButtonStyle(kind: model.canCommit ? .primary : .secondary, height: 34))
            .disabled(!model.canCommit)
            .keyboardShortcut(.return, modifiers: .command)
            .help("Commit (⌘↩). Runs git, so your hooks and signing apply.")
        }
        .padding(12)
        .well()
        .padding(10)
    }

    private var commitTitle: String {
        if model.committing { return "Committing…" }
        if model.amend { return "Amend" }
        let n = model.status?.staged.count ?? 0
        return n == 0 ? "Nothing staged" : "Commit \(n) to"
    }

    private var branchName: String? {
        guard !model.committing, model.amend || !(model.status?.staged.isEmpty ?? true) else { return nil }
        if case let .branch(name, _)? = model.status?.head { return name }
        return nil
    }
}

// MARK: - The diff

struct WorkDiffPanel: View {
    @Environment(AppModel.self) private var model
    @State private var confirmDiscardLines = false
    @State private var confirmDiscardFile = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                if let d = model.workDiff, let c = model.change {
                    Text(d.status.label)
                        .font(N.mono(10.5, weight: .semibold))
                        .foregroundStyle(N.status(d.status == .untracked ? .added : d.status))
                        .padding(.horizontal, 7)
                        .frame(height: 20)
                        .background(N.status(d.status == .untracked ? .added : d.status).opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
                    Text(d.path).font(N.mono(12.5, weight: .medium)).foregroundStyle(N.text).lineLimit(1).truncationMode(.head)
                    StatCounts(additions: d.additions, deletions: d.deletions)
                    Text(c.area == .staged ? "staged" : "not staged").font(.system(size: 11.5)).foregroundStyle(N.faint)
                }
                Spacer(minLength: 8)
                actions
                Segmented(options: [(DiffLayout.unified, "Unified"), (.split, "Split")], selection: $model.layout)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)

            Group {
                if let d = model.workDiff {
                    if d.binary {
                        EmptyState(title: "Binary file", message: "Stage or discard it whole.")
                    } else if d.tooLarge {
                        EmptyState(title: "Too large to show", message: "One side of this file is over 8 MB. Stage or discard it whole.")
                    } else if d.hunks.isEmpty {
                        EmptyState(title: "No line changes", message: "Only the file's mode changed, or it's empty.")
                    } else {
                        DiffBody(
                            diff: d,
                            layout: model.layout,
                            picks: model.picks,
                            onPick: { model.togglePick($0) },
                            hunkAction: model.change?.area == .staged ? "Unstage hunk" : "Stage hunk",
                            onHunk: { h in Task { await model.applyHunk(h) } }
                        )
                    }
                } else if let error = model.workDiffError {
                    EmptyState(title: "Can't show this file", message: error)
                } else if model.status?.isClean == true {
                    EmptyState(title: "Working tree clean", message: "Everything is committed.")
                } else {
                    EmptyState(title: "No file selected", message: "Pick a file to see its changes. Click lines to pick them, then stage, unstage or discard just those.")
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.well)
        }
        .island()
        .confirmationDialog("Discard \(model.picks.count) picked lines?", isPresented: $confirmDiscardLines) {
            Button("Discard lines", role: .destructive) { Task { await model.discardPicks() } }
        } message: {
            Text("Those lines go back to their staged version. A copy of the file is kept in the repository and listed in ~/.neutrino/discarded.log.")
        }
        .confirmationDialog("Discard all changes to this file?", isPresented: $confirmDiscardFile) {
            Button(model.workDiff?.status == .untracked ? "Move to Trash" : "Discard changes", role: .destructive) {
                if let p = model.change?.path { Task { await model.discard(paths: [p]) } }
            }
        } message: {
            Text("A copy is kept in the repository and listed in ~/.neutrino/discarded.log.")
        }
    }

    @ViewBuilder
    private var actions: some View {
        if let c = model.change, let d = model.workDiff {
            let editable = !d.binary && !d.tooLarge && d.status != .conflicted
            if !model.picks.isEmpty {
                Button("Clear") { model.picks = [] }.buttonStyle(PillButtonStyle(kind: .quiet, height: 28))
                if c.area == .unstaged, d.status != .untracked {
                    Button("Discard \(model.picks.count)…") { confirmDiscardLines = true }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                        .disabled(model.writing)
                }
                Button(c.area == .staged ? "Unstage \(model.picks.count) lines" : "Stage \(model.picks.count) lines") {
                    Task { await model.applyPicks() }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 28))
                .disabled(model.writing || !editable)
                .keyboardShortcut("s", modifiers: [.command, .shift])
            } else if let f = (c.area == .staged ? model.status?.staged : (model.notStaged + (model.status?.conflicted ?? [])))?.first(where: { $0.path == c.path }) {
                if c.area == .unstaged, f.status != .conflicted {
                    Button(f.status == .untracked ? "Trash…" : "Discard…") { confirmDiscardFile = true }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                        .disabled(model.writing)
                }
                Button(c.area == .staged ? "Unstage file" : (f.status == .conflicted ? "Mark resolved" : "Stage file")) {
                    Task { await model.toggle(f, staged: c.area == .staged) }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 28))
                .disabled(model.writing)
                .keyboardShortcut("s", modifiers: [.command, .shift])
            }
        }
    }
}
