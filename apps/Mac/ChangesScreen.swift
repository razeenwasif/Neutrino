import NeutrinoKit
import SwiftUI

struct ChangesScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 0) {
            ChangeList()
                .frame(width: 340)
                .frame(maxHeight: .infinity)

            WorkDiffPanel()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

// MARK: - The list

struct ChangeList: View {
    @Environment(AppModel.self) private var model
    @State private var discarding: StatusFile?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Changes")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(N.text)
                    if let s = model.status {
                        Text(s.isClean ? "Working tree clean" : "\(s.changeCount) \(s.changeCount == 1 ? "file" : "files") modified")
                            .font(.system(size: 11.5))
                            .foregroundStyle(N.muted)
                    }
                }
                Spacer()
                if !model.notStaged.isEmpty || !(model.status?.conflicted.isEmpty ?? true) {
                    ActionPillButton(title: "Stage all", systemImage: "plus", tint: N.text, background: N.surfacePill) {
                        Task { await model.stageAll() }
                    }
                    .disabled(model.writing)
                } else if !(model.status?.staged.isEmpty ?? true) {
                    ActionPillButton(title: "Unstage all", systemImage: "minus", tint: N.muted, background: N.surfacePill) {
                        Task { await model.unstageAll() }
                    }
                    .disabled(model.writing)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 18)
            .padding(.bottom, 12)

            if let op = model.status?.operation {
                HStack(alignment: .top, spacing: 8) {
                    StatusDot(color: N.amber, size: 5.5)
                        .padding(.top, 3)
                    Text("A \(op) is in progress. Resolve conflicts, stage the files, then commit.")
                        .font(.system(size: 12))
                        .foregroundStyle(N.amber)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(N.amber.opacity(0.08), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .padding(.horizontal, 14)
                .padding(.bottom, 10)
            }

            if let error = model.statusError {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't read the working copy", message: error)
            } else if model.repo?.bare == true {
                EmptyState(icon: "folder", title: "No working copy", message: "This is a bare repository, so there's nothing to stage.")
            } else if let s = model.status {
                ScrollView(.vertical, showsIndicators: false) {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        if !s.conflicted.isEmpty {
                            group("Conflicted · \(s.conflicted.count)")
                            ForEach(s.conflicted) { f in row(f, staged: false) }
                        }
                        group("Staged · \(s.staged.count)")
                        if s.staged.isEmpty {
                            hint(s.isClean ? "Nothing to commit." : "Nothing staged. Tick a file, or pick lines from its diff.")
                        }
                        ForEach(s.staged) { f in row(f, staged: true) }
                        group("Not staged · \(model.notStaged.count)")
                        if model.notStaged.isEmpty {
                            hint(s.isClean ? "The working tree matches HEAD." : "Everything that changed is staged.")
                        }
                        ForEach(model.notStaged) { f in row(f, staged: false) }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
                }
            } else {
                Spacer()
            }

            CommitBox()
        }
        .background(N.inspector)
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
        SectionLabel(title)
            .padding(.horizontal, 8)
            .padding(.top, 12)
            .padding(.bottom, 4)
    }

    private func hint(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(N.faint)
            .padding(.horizontal, 8)
            .padding(.bottom, 6)
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
                Button("Copy path") { model.copy(f.path, label: "path") }
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
                    RoundedRectangle(cornerRadius: 4.5, style: .continuous)
                        .fill(staged ? N.indigo : N.surfaceHover)
                        .overlay {
                            if !staged {
                                RoundedRectangle(cornerRadius: 4.5, style: .continuous)
                                    .strokeBorder(N.faint.opacity(0.6), lineWidth: 1)
                            }
                        }
                    if staged {
                        Image(systemName: "checkmark")
                            .font(.system(size: 9, weight: .heavy))
                            .foregroundStyle(N.onAccent)
                    }
                }
                .frame(width: 15, height: 15)
                .frame(width: 24, height: 28)
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

            (Text(name).foregroundStyle(N.text) + Text(dir.isEmpty ? "" : "  " + dir).foregroundStyle(N.faint).font(.system(size: 11)))
                .font(.system(size: 12.5))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: 4)

            if file.binary {
                Text("binary")
                    .font(N.mono(10.5))
                    .foregroundStyle(N.faint)
            } else if file.status != .conflicted {
                StatCounts(additions: file.additions, deletions: file.deletions)
            }
        }
        .padding(.horizontal, 6)
        .frame(height: 32)
        .background(
            selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .onHover { hovering = $0 }
        .help(file.oldPath.map { "\($0) → \(file.path)" } ?? file.path)
    }

    private var color: Color {
        switch file.status {
        case .added, .untracked: N.mint
        case .deleted, .conflicted: N.rose
        default: N.amber
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
        VStack(alignment: .leading, spacing: 10) {
            if let last = model.lastCommit {
                HStack(spacing: 8) {
                    StatusDot(color: N.mint, size: 5)
                    Text("Committed \(String(last.sha.prefix(7)))")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(N.mint)
                    Spacer()
                    if !last.output.isEmpty {
                        Button(showOutput ? "Hide output" : "Hooks") { showOutput.toggle() }
                            .buttonStyle(.plain)
                            .font(.system(size: 11.5))
                            .foregroundStyle(N.muted)
                    }
                    Button {
                        model.lastCommit = nil
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(N.muted)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dismiss")
                }
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(N.mint.opacity(0.09), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

                if showOutput {
                    ScrollView {
                        Text(last.output)
                            .font(N.mono(11))
                            .foregroundStyle(N.body)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 110)
                    .padding(9)
                    .background(N.inspector, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("Summary")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(N.muted)
                TextField("", text: $model.commitSummary, prompt: Text("What changed").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .focused($summaryFocused)
                    .onSubmit { Task { await model.commit() } }
            }

            VStack(alignment: .leading, spacing: 4) {
                Text("Description")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(N.muted)
                TextEditor(text: $model.commitBody)
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.text)
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 5)
                    .frame(height: 60)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

            HStack(spacing: 8) {
                Toggle(isOn: Binding(get: { model.amend }, set: { $0 ? model.startAmend() : (model.amend = false) })) {
                    Text("Amend last commit")
                        .font(.system(size: 12))
                        .foregroundStyle(N.muted)
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
                    if let branch = branchName {
                        Text(branch).font(N.mono(12, weight: .semibold))
                    }
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(PillButtonStyle(kind: model.canCommit ? .primary : .secondary, height: 34))
            .disabled(!model.canCommit)
            .keyboardShortcut(.return, modifiers: .command)
            .help("Commit (⌘↩). Runs git, so your hooks and signing apply.")
        }
        .padding(14)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(12)
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
                    let statusColor = N.status(d.status == .untracked ? .added : d.status)
                    TagPill(
                        text: d.status.label,
                        textColor: statusColor,
                        background: statusColor.opacity(0.14),
                        monospaced: true
                    )
                    Text(d.path)
                        .font(N.mono(12.5, weight: .medium))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                        .truncationMode(.head)
                        .layoutPriority(-1)
                    StatCounts(additions: d.additions, deletions: d.deletions)
                    TagPill(
                        text: c.area == .staged ? "staged" : "unstaged",
                        textColor: c.area == .staged ? N.mint : N.muted
                    )
                }
                Spacer(minLength: 8)
                actions
                HStack(spacing: 2) {
                    FilterPillButton(title: "Unified", isSelected: model.layout == .unified) {
                        model.layout = .unified
                    }
                    FilterPillButton(title: "Split", isSelected: model.layout == .split) {
                        model.layout = .split
                    }
                }
                .padding(2)
                .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .fixedSize()
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 12)
            .background(N.ground)

            Group {
                if let d = model.workDiff {
                    if d.binary {
                        EmptyState(icon: "doc.zipper", title: "Binary file", message: "Stage or discard it whole.")
                    } else if d.tooLarge {
                        EmptyState(icon: "doc.text.magnifyingglass", title: "Too large to show", message: "One side of this file is over 8 MB. Stage or discard it whole.")
                    } else if d.hunks.isEmpty {
                        EmptyState(icon: "doc.text", title: "No line changes", message: "Only the file's mode changed, or it's empty.")
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
                    EmptyState(icon: "exclamationmark.triangle", title: "Can't show this file", message: error)
                } else if model.status?.isClean == true {
                    EmptyState(icon: "checkmark.circle", title: "Working tree clean", message: "Everything is committed.")
                } else {
                    EmptyState(icon: "doc.text.magnifyingglass", title: "No file selected", message: "Pick a file to see its changes. Click lines to pick them, then stage, unstage or discard just those.")
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.ground)
        }
        .background(N.ground)
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
                Button("Clear") { model.picks = [] }
                    .buttonStyle(PillButtonStyle(kind: .quiet, height: 28))
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
