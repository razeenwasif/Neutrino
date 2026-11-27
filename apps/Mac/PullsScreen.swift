import NeutrinoKit
import SwiftUI

struct PullsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let problem = model.pullRepoProblem {
                EmptyState(icon: "arrow.triangle.pull", title: "No pull requests here", message: problem)
                    .background(N.ground)
            } else {
                HStack(spacing: 0) {
                    PullList()
                        .frame(width: 340)
                        .frame(maxHeight: .infinity)
                    PullDetailPanel()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
    @State private var rulesHovered = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("Pull requests")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Spacer(minLength: 6)
                Button {
                    editingRules = true
                } label: {
                    Image(systemName: "lock.shield")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(rulesHovered ? N.text : N.muted)
                        .frame(width: 28, height: 28)
                        .background(rulesHovered ? N.surfaceActive : N.surfacePill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                }
                .buttonStyle(.plain)
                .onHover { rulesHovered = $0 }
                .help("Branch rules")
                .accessibilityLabel("Branch rules")

                Button("New") { creating = true }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 28))
                    .help("Open a pull request")
            }
            .padding(.horizontal, 16)
            .padding(.top, 18)
            .padding(.bottom, 12)

            HStack(spacing: 4) {
                FilterPillButton(title: "Open \(count(.open))", isSelected: model.pullFilter == .open) {
                    model.pullFilter = .open
                }
                FilterPillButton(title: "Closed \(count(.closed))", isSelected: model.pullFilter == .closed) {
                    model.pullFilter = .closed
                }
                Spacer()
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 12)

            if let error = model.pullError, model.pullList.isEmpty {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't load pull requests", message: error)
            } else if model.visiblePulls.isEmpty, !model.loadingPulls {
                EmptyState(
                    icon: "arrow.triangle.pull",
                    title: model.pullFilter == .open ? "Nothing open" : "Nothing closed yet",
                    message: model.pullFilter == .open ? "Push a branch, then open a pull request to merge it." : "Merged and closed pull requests show up here."
                )
            } else {
                ScrollView(.vertical, showsIndicators: false) {
                    LazyVStack(spacing: 2) {
                        ForEach(model.visiblePulls) { p in
                            PullRow(pull: p, selected: p.number == model.pullNumber)
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 16)
                }
            }
        }
        .background(N.inspector)
        .onChange(of: model.pullFilter) {
            if let first = model.visiblePulls.first, !model.visiblePulls.contains(where: { $0.number == model.pullNumber }) {
                Task { await model.select(pull: first.number) }
            }
        }
        .sheet(isPresented: $creating) { NewPullSheet() }
        .onAppear { takeSheetRequest() }
        .onChange(of: model.sheetRequest) { takeSheetRequest() }
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
        case .open: N.mint
        case .merged: N.violet
        case .closed: N.rose
        }
        TagPill(text: state.label, color: color, textColor: color, background: color.opacity(0.14))
    }
}

struct BranchFlow: View {
    var source: String
    var target: String

    var body: some View {
        HStack(spacing: 5) {
            Text(source).foregroundStyle(N.indigo).lineLimit(1).truncationMode(.middle)
            Image(systemName: "arrow.right").font(.system(size: 9, weight: .bold)).foregroundStyle(N.faint)
            Text(target).foregroundStyle(N.sky).lineLimit(1)
        }
        .font(N.mono(11.5, weight: .medium))
    }
}

struct PullRow: View {
    @Environment(AppModel.self) private var model
    var pull: PullRequest
    var selected: Bool
    @State private var hovering = false

    private var dotColor: Color {
        switch pull.state {
        case .open: N.mint
        case .merged: N.violet
        case .closed: N.rose
        }
    }

    var body: some View {
        Button {
            Task { await model.select(pull: pull.number) }
        } label: {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 7) {
                    StatusDot(color: dotColor, size: 5)
                    Text("#\(pull.number)")
                        .font(N.mono(11, weight: .semibold))
                        .foregroundStyle(N.faint)
                    Text(pull.title)
                        .font(.system(size: 13, weight: selected ? .semibold : .medium))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                }
                HStack(spacing: 8) {
                    BranchFlow(source: pull.source, target: pull.target)
                    Spacer(minLength: 4)
                    if pull.openNotes > 0 {
                        Label("\(pull.openNotes)", systemImage: "text.bubble")
                            .font(.system(size: 11))
                            .foregroundStyle(N.amber)
                            .help("\(pull.openNotes) unresolved notes")
                    }
                    if pull.state != .open { StateChip(state: pull.state) }
                    Text(Format.ago(pull.updatedAt))
                        .font(N.mono(10.5))
                        .foregroundStyle(N.faint)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear),
                in: RoundedRectangle(cornerRadius: 9, style: .continuous)
            )
            .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
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
                    HStack(spacing: 4) {
                        FilterPillButton(title: "Overview", isSelected: model.pullTab == .overview) {
                            model.pullTab = .overview
                        }
                        FilterPillButton(title: "Files \(d.files.count)", isSelected: model.pullTab == .files) {
                            model.pullTab = .files
                        }
                        FilterPillButton(title: "Commits \(d.commits.count)", isSelected: model.pullTab == .commits) {
                            model.pullTab = .commits
                        }
                        Spacer()
                    }
                    .padding(.horizontal, 28)
                    .padding(.bottom, 14)

                    switch model.pullTab {
                    case .overview: PullOverview(detail: d)
                    case .files: PullFiles(detail: d)
                    case .commits: PullCommits(detail: d)
                    }
                }
            } else if let error = model.pullError {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't show this pull request", message: error)
            } else {
                EmptyState(icon: "arrow.triangle.pull", title: "No pull request selected", message: "Pick one from the list, or open a new one.")
            }
        }
        .background(N.ground)
    }
}

struct PullHeader: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail
    @State private var editing = false

    var body: some View {
        let p = detail.pull
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(p.title)
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(N.text)
                    .lineLimit(2)
                Text("#\(p.number)")
                    .font(N.mono(16))
                    .foregroundStyle(N.faint)
                Spacer(minLength: 8)
                if p.state != .merged {
                    ActionPillButton(title: "Edit", systemImage: "pencil", tint: N.muted, background: N.surfacePill) {
                        editing = true
                    }
                }
            }
            HStack(spacing: 10) {
                StateChip(state: p.state)
                BranchFlow(source: p.source, target: p.target)
                Text("·").foregroundStyle(N.faint)
                Text(p.versions == 1 ? "1 version" : "\(p.versions) versions")
                    .help("Each push to \(p.source) while it's open is a new version.")
                Text("·").foregroundStyle(N.faint)
                Text("opened \(Format.ago(p.createdAt))")
            }
            .font(.system(size: 12))
            .foregroundStyle(N.muted)

            if !detail.stack.isEmpty {
                stackChainBar(current: p, members: detail.stack)
            }
        }
        .padding(.horizontal, 28)
        .padding(.top, 22)
        .padding(.bottom, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .sheet(isPresented: $editing) { EditPullSheet(pull: p) }
    }

    private func stackChainBar(current: PullRequest, members: [StackMember]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                Text("STACK")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(N.faint)
                    .padding(.trailing, 2)
                ForEach(Array(members.enumerated()), id: \.element.id) { idx, m in
                    if idx > 0 {
                        Image(systemName: "arrow.right")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(N.faint)
                    }
                    let isHere = m.number == current.number
                    Button {
                        if !isHere { Task { await model.select(pull: m.number) } }
                    } label: {
                        HStack(spacing: 5) {
                            Text("#\(m.number)")
                                .font(N.mono(11, weight: .semibold))
                            Text(m.source)
                                .font(.system(size: 11, weight: isHere ? .semibold : .regular))
                                .lineLimit(1)
                            if m.stackConflict != nil {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.system(size: 10))
                                    .foregroundStyle(N.rose)
                            }
                        }
                        .foregroundStyle(isHere ? N.text : N.muted)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(
                            isHere ? N.indigo.opacity(0.20) : N.surfaceSubtle,
                            in: Capsule()
                        )
                    }
                    .buttonStyle(.plain)
                    .help(m.stackConflict ?? m.title)
                }
                if members.contains(where: { $0.state == .open }) {
                    Button {
                        Task { await model.syncStack() }
                    } label: {
                        Label(model.pullBusy == "Syncing stack" ? "Syncing…" : "Sync stack", systemImage: "arrow.triangle.2.circlepath")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(N.sky)
                            .padding(.horizontal, 9)
                            .padding(.vertical, 4)
                            .background(N.sky.opacity(0.12), in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .disabled(model.pullBusy != nil)
                }
            }
        }
    }
}

// MARK: Overview

struct PullOverview: View {
    @Environment(AppModel.self) private var model
    var detail: PullDetail

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(alignment: .leading, spacing: 22) {
                if detail.pull.body.isEmpty {
                    Text("No description.")
                        .font(.system(size: 13))
                        .foregroundStyle(N.faint)
                        .italic()
                } else {
                    Text(detail.pull.body)
                        .font(.system(size: 13.5))
                        .foregroundStyle(N.body)
                        .lineSpacing(4)
                        .textSelection(.enabled)
                }
                if let receipt = detail.receipt {
                    VerificationReceiptCard(receipt: receipt)
                }
                if let receipt = detail.receipt {
                    VerificationReceiptCard(receipt: receipt)
                }
                MergeBox(detail: detail)
                NotesList(detail: detail)
                VersionList(detail: detail)
            }
            .padding(.horizontal, 28)
            .padding(.top, 6)
            .padding(.bottom, 36)
            .frame(maxWidth: 780, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct VerificationReceiptCard: View {
    var receipt: VerificationReceipt

    private var riskColor: Color {
        switch receipt.riskLevel {
        case "high": N.rose
        case "medium": N.amber
        default: N.mint
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: "checkmark.shield.fill")
                    .font(.system(size: 13))
                    .foregroundStyle(riskColor)
                Text("Verification Receipt")
                    .font(.system(size: 13.5, weight: .semibold))
                    .foregroundStyle(N.text)
                TagPill(text: "\(receipt.riskLevel.capitalized) blast radius", color: riskColor, textColor: riskColor, background: riskColor.opacity(0.14))
                Spacer()
                if receipt.provenanceCount > 0 {
                    let suffix = receipt.deviceSignedCount > 0 ? " · \(receipt.deviceSignedCount) Secure Enclave" : ""
                    TagPill(text: "SLSA L3 (\(receipt.provenanceCount)\(suffix))", color: N.indigo, textColor: N.indigo, background: N.indigo.opacity(0.14))
                }
            }

            if receipt.publicSymbolDeltas.isEmpty && receipt.dependencyFiles.isEmpty && receipt.securityPaths.isEmpty && receipt.verifiedChecks.isEmpty {
                Text("No exported API changes, dependency bumps, or security-sensitive path modifications detected.")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
            } else {
                FlowWrapLayout(horizontalSpacing: 6, verticalSpacing: 6) {
                    ForEach(receipt.securityPaths, id: \.self) { path in
                        TagPill(text: "Security: \(path)", color: N.rose, textColor: N.rose, background: N.rose.opacity(0.12))
                    }
                    ForEach(receipt.dependencyFiles, id: \.self) { dep in
                        TagPill(text: "Deps: \(dep)", color: N.amber, textColor: N.amber, background: N.amber.opacity(0.12))
                    }
                    ForEach(receipt.publicSymbolDeltas.prefix(10)) { d in
                        let prefix = switch d.change {
                        case .added: "+"
                        case .removed: "-"
                        case .signatureChanged: "~"
                        }
                        let col: Color = d.change == .added ? N.mint : (d.change == .removed ? N.rose : N.amber)
                        TagPill(text: "\(prefix)\(d.symbol) (\(d.kind))", color: col, textColor: col, background: col.opacity(0.12))
                    }
                    ForEach(receipt.verifiedChecks, id: \.self) { chk in
                        TagPill(text: "✓ \(chk)", color: N.mint, textColor: N.mint, background: N.mint.opacity(0.12))
                    }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
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
        VStack(alignment: .leading, spacing: 14) {
            switch p.state {
            case .merged:
                status(icon: "checkmark.circle.fill", color: N.violet, title: "Merged into \(p.target)",
                       detail: [p.mergeSha.map { "as \($0.prefix(7))" }, p.mergedAt.map { Format.ago($0) }].compactMap { $0 }.joined(separator: ", "))
            case .closed:
                status(icon: "xmark.circle.fill", color: N.rose, title: "Closed without merging", detail: "Reopen it to carry on.")
                ActionPillButton(title: "Reopen", systemImage: "arrow.uturn.backward", tint: N.text, background: N.surfacePill) {
                    Task { await model.setPullState(.open) }
                }
                .disabled(model.pullBusy != nil)
            case .open:
                checks
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
                        .foregroundStyle(N.text)
                        .padding(.horizontal, 11)
                        .frame(height: 30)
                        .background(N.surfacePill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .fixedSize()
                    .help(strategy.detail)

                    Toggle("Delete \(p.source) afterwards", isOn: $deleteSource)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 12))
                        .foregroundStyle(N.muted)

                    Spacer()

                    Button("Close") { Task { await model.setPullState(.closed) } }
                        .buttonStyle(PillButtonStyle(kind: .quiet, height: 30))
                        .disabled(model.pullBusy != nil)

                    ActionPillButton(
                        title: model.pullBusy == "Merging" ? "Merging…" : "Merge",
                        systemImage: "arrow.triangle.merge",
                        tint: canMerge ? N.onAccent : N.muted,
                        background: canMerge ? N.mint : N.surfacePill,
                        isLoading: model.pullBusy == "Merging"
                    ) {
                        Task { await model.merge(strategy: strategy, deleteSource: deleteSource) }
                    }
                    .disabled(!canMerge || model.pullBusy != nil)
                    .help(mergeHelp)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var canMerge: Bool {
        guard let m = detail.mergeability else { return false }
        if strategy == .fastForward { return m.fastForward }
        return m.clean && missing.isEmpty
    }

    /// Required checks that haven't passed on the latest version.
    private var missing: [String] { missingChecks(detail.rule?.requiredChecks ?? [], in: detail.checks) }

    private var mergeHelp: String {
        guard let m = detail.mergeability else { return "One of the branches is gone." }
        if strategy == .fastForward, !m.fastForward { return "\(detail.pull.target) has moved on, so it can't fast-forward." }
        if !m.clean { return "Resolve the conflicts first." }
        if !missing.isEmpty { return "\(missing.joined(separator: ", ")) must pass first." }
        return "\(strategy.label) \(detail.pull.source) into \(detail.pull.target). \(strategy.detail)"
    }

    @ViewBuilder private var checks: some View {
        let p = detail.pull
        if let m = detail.mergeability {
            if !m.clean {
                status(icon: "exclamationmark.triangle.fill", color: N.rose, title: "Conflicts with \(p.target)",
                       detail: "In \(m.conflicts.prefix(4).joined(separator: ", "))\(m.conflicts.count > 4 ? " and \(m.conflicts.count - 4) more" : ""). Merge \(p.target) into \(p.source), resolve them and push.")
            } else if m.fastForward {
                status(icon: "checkmark.circle.fill", color: N.mint, title: "Ready to merge", detail: "\(p.target) hasn't moved, so this can fast-forward.")
            } else {
                status(icon: "checkmark.circle.fill", color: N.mint, title: "Merges cleanly",
                       detail: "\(p.target) is \(m.behind) commit\(m.behind == 1 ? "" : "s") ahead of where \(p.source) started.")
            }
        } else {
            status(icon: "questionmark.circle", color: N.amber, title: "Can't check", detail: "\(p.source) or \(p.target) is gone from the server.")
        }
        if p.openNotes > 0 {
            status(icon: "text.bubble", color: N.amber, title: "\(p.openNotes) unresolved note\(p.openNotes == 1 ? "" : "s")", detail: "Notes don't block merging.")
        }
        if let r = detail.rule {
            status(icon: "lock.shield", color: N.sky, title: "\(p.target) is guarded by \(r.pattern)",
                   detail: r.requiredChecks.isEmpty ? r.summary : "\(r.summary); \(r.requiredChecks.joined(separator: ", ")) must pass")
        }
        if !detail.checks.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(detail.checks) { run in
                    Button {
                        Task { await model.show(check: run) }
                    } label: {
                        HStack(spacing: 8) {
                            RunStateIcon(state: run.state, size: 12)
                            Text(run.name).font(.system(size: 12.5, weight: .medium)).foregroundStyle(N.text)
                            if detail.rule?.requiredChecks.contains(where: { $0 == run.name || $0 == run.workflow || $0 + ".yml" == run.workflow }) == true {
                                TagPill(text: "required", textColor: N.sky, background: N.sky.opacity(0.14))
                            }
                            Text(run.state.label).font(.system(size: 11.5)).foregroundStyle(N.muted)
                            Spacer()
                            Text(formatDuration(run.duration())).font(N.mono(11)).foregroundStyle(N.faint)
                        }
                        .padding(.horizontal, 10)
                        .frame(height: 30)
                        .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .help("Open run \(run.number)")
                }
            }
            .padding(.leading, 28)
        }
        ForEach(missing.filter { name in !detail.checks.contains { $0.name == name || $0.workflow.hasPrefix(name) } }, id: \.self) { name in
            status(icon: "circle.dashed", color: N.amber, title: "\(name) hasn't run", detail: "It's required, but no workflow by that name ran on this version.")
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
                .foregroundStyle(note.resolved ? N.faint : N.amber)
                .font(.system(size: 13))
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    if showPlace {
                        Button {
                            model.pullTab = .files
                            Task { await model.select(pullFile: note.path) }
                        } label: {
                            Text("\(note.path):\(note.currentLine ?? note.line)")
                                .font(N.mono(11.5, weight: .medium))
                                .foregroundStyle(N.sky)
                        }
                        .buttonStyle(.plain)
                        .help("Show it in Files")
                    }
                    if note.outdated {
                        TagPill(text: "outdated", textColor: N.faint)
                            .help("The line changed after version \(note.version), where this was written.")
                    }
                    Text("v\(note.version) · \(Format.ago(note.createdAt))")
                        .font(N.mono(10.5))
                        .foregroundStyle(N.faint)
                }
                Text(note.text)
                    .font(.system(size: 12.5))
                    .foregroundStyle(note.resolved ? N.muted : N.body)
                    .textSelection(.enabled)
            }
            Spacer(minLength: 8)
            Button(note.resolved ? "Reopen" : "Resolve") { Task { await model.resolve(note, !note.resolved) } }
                .buttonStyle(PillButtonStyle(kind: .secondary, height: 24))
                .font(.system(size: 11))
                .disabled(model.pullBusy != nil)
        }
        .padding(12)
        .background(N.surfaceSubtle.opacity(note.resolved ? 0.65 : 1), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

struct VersionList: View {
    var detail: PullDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("Versions")
            VStack(spacing: 4) {
                ForEach(detail.versions.reversed()) { v in
                    HStack(spacing: 10) {
                        Text("v\(v.version)")
                            .font(N.mono(11.5, weight: .semibold))
                            .foregroundStyle(v.version == detail.versions.last?.version ? N.indigo : N.muted)
                            .frame(width: 30, alignment: .leading)
                        Text(String(v.headSha.prefix(7)))
                            .font(N.mono(11.5))
                            .foregroundStyle(N.text)
                        Text("from \(v.baseSha.prefix(7))")
                            .font(N.mono(11))
                            .foregroundStyle(N.faint)
                        Spacer()
                        Text(Format.ago(v.createdAt))
                            .font(N.mono(11))
                            .foregroundStyle(N.faint)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
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
        let shownFiles = model.orderedFiles(for: detail)
        let activeMoves = model.pullInterdiff?.moves ?? detail.moves
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                if !(model.pullInterdiff?.tour ?? detail.tour).isEmpty {
                    HStack(spacing: 2) {
                        FilterPillButton(title: "Tour", isSelected: model.pullOrder == .tour) {
                            model.pullOrder = .tour
                        }
                        FilterPillButton(title: "Path", isSelected: model.pullOrder == .path) {
                            model.pullOrder = .path
                        }
                    }
                    .padding(2)
                    .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .padding(.horizontal, 10)
                    .padding(.top, 10)
                    .padding(.bottom, 4)
                }
                ScrollView(.vertical, showsIndicators: false) {
                    VStack(spacing: 2) {
                        ForEach(shownFiles) { f in
                            let notes = (model.pullInterdiff?.notes ?? detail.notes).filter { $0.path == f.path && !$0.resolved }.count
                            let isSelected = model.pullFile == f.path
                            let tour = model.pullOrder == .tour ? model.tourStep(for: f.path, detail: detail) : nil
                            Button {
                                composing = nil
                                Task { await model.select(pullFile: f.path) }
                            } label: {
                                HStack(spacing: 7) {
                                    if let (stepNum, step) = tour {
                                        Text("\(stepNum)")
                                            .font(N.mono(10, weight: .semibold))
                                            .foregroundStyle(step.tier == "core" ? N.indigo : N.muted)
                                            .frame(width: 16, height: 16)
                                            .background(N.surfaceSubtle, in: Circle())
                                    }
                                    Text(f.status.letter)
                                        .font(N.mono(10.5, weight: .semibold))
                                        .foregroundStyle(N.status(f.status))
                                        .frame(width: 12)
                                    Text((f.path as NSString).lastPathComponent)
                                        .font(.system(size: 12.5, weight: isSelected ? .medium : .regular))
                                        .foregroundStyle(isSelected ? N.text : N.body)
                                        .lineLimit(1)
                                    Spacer(minLength: 4)
                                    if notes > 0 {
                                        Image(systemName: "text.bubble")
                                            .font(.system(size: 10))
                                            .foregroundStyle(N.amber)
                                    }
                                    if !f.binary { StatCounts(additions: f.additions, deletions: f.deletions) }
                                }
                                .padding(.horizontal, 10)
                                .frame(height: 30)
                                .background(isSelected ? N.surfaceActive : Color.clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                                .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                            }
                            .buttonStyle(.plain)
                            .help([f.oldPath.map { "\($0) → \(f.path)" } ?? f.path, tour?.1.reason].compactMap { $0 }.joined(separator: " · "))
                        }
                    }
                    .padding(10)
                }
            }
            .frame(width: 245)
            .background(N.inspector)

            VStack(spacing: 0) {
                if !activeMoves.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            Image(systemName: "arrow.left.and.right.square")
                                .font(.system(size: 11))
                                .foregroundStyle(N.sky)
                            Text("Moved across PR:")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(N.muted)
                            ForEach(activeMoves.prefix(6), id: \.id) { m in
                                TagPill(
                                    text: "\(m.symbol) (\((m.fromPath as NSString).lastPathComponent)→\((m.toPath as NSString).lastPathComponent))",
                                    textColor: N.sky,
                                    background: N.sky.opacity(0.12),
                                    monospaced: true
                                )
                            }
                        }
                        .padding(.horizontal, 18)
                        .padding(.vertical, 6)
                    }
                    .background(N.surfaceSubtle)
                }
                diff
            }
        }
    }

    @ViewBuilder private var diff: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                if let d = model.pullDiff {
                    Text(d.path)
                        .font(N.mono(12, weight: .medium))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                        .truncationMode(.head)
                    StatCounts(additions: d.additions, deletions: d.deletions)
                    let noiseCount = d.hunks.filter { $0.noise != nil }.count
                    if noiseCount > 0 {
                        FilterPillButton(
                            title: model.hideFormatting ? "Show formatting (\(noiseCount))" : "Hide formatting (\(noiseCount))",
                            isSelected: model.hideFormatting
                        ) {
                            model.hideFormatting.toggle()
                        }
                    }
                }
                if detail.versions.count > 1, let latest = detail.versions.last {
                    Menu {
                        Button("Latest (vs \(detail.pull.target))") {
                            Task { await model.selectInterdiff(from: nil, to: nil) }
                        }
                        Divider()
                        ForEach(detail.versions.dropLast()) { v in
                            Button("Interdiff v\(v.version) → v\(latest.version)") {
                                Task { await model.selectInterdiff(from: v.version, to: latest.version) }
                            }
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "clock.arrow.2.circlepath")
                                .font(.system(size: 11))
                            Text(model.pullInterdiffFrom.map { "v\($0) → v\(model.pullInterdiffTo ?? latest.version)" } ?? "v\(latest.version)")
                                .font(N.mono(11, weight: .medium))
                        }
                        .foregroundStyle(model.pullInterdiffFrom != nil ? N.indigo : N.muted)
                        .padding(.horizontal, 9)
                        .padding(.vertical, 4)
                        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                }
                ViewThatFits(in: .horizontal) {
                    Text("Click a line to leave a note")
                        .font(.system(size: 11))
                        .foregroundStyle(N.faint)
                        .fixedSize()
                        .frame(maxWidth: .infinity, alignment: .trailing)
                    Spacer(minLength: 0)
                }
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
                .help("Click a line to leave a note on it")
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 10)
            .background(N.ground)

            Group {
                if let d = model.pullDiff {
                    if d.binary {
                        EmptyState(icon: "doc.zipper", title: "Binary file", message: "Neutrino doesn't show binary changes yet.")
                    } else if d.tooLarge {
                        EmptyState(icon: "doc.text.magnifyingglass", title: "Too large to show", message: "One side of this file is over 8 MB.")
                    } else if d.hunks.isEmpty {
                        EmptyState(icon: "doc.text", title: "No line changes", message: d.oldPath != nil ? "Renamed without changes." : "Only the file's mode changed.")
                    } else {
                        DiffBody(diff: d, layout: model.layout, hideFormatting: model.hideFormatting, onPick: model.pullInterdiffFrom == nil ? { line in
                            composing = line
                            draft = ""
                        } : nil, pickContext: true, footer: { line in footer(for: line, path: d.path) })
                    }
                } else if let error = model.pullDiffError {
                    EmptyState(icon: "exclamationmark.triangle", title: "Can't show this diff", message: error)
                } else if detail.files.isEmpty {
                    EmptyState(icon: "doc", title: "No changes", message: "\(detail.pull.source) has nothing \(detail.pull.target) doesn't.")
                } else {
                    Color.clear
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.ground)
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
                .foregroundStyle(N.text)
                .scrollContentBackground(.hidden)
                .focused($focused)
                .frame(minHeight: 54, maxHeight: 120)
                .padding(8)
                .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            HStack {
                Text("⌘↩ to save").font(N.mono(10.5)).foregroundStyle(N.faint)
                Spacer()
                Button("Cancel", action: cancel)
                    .buttonStyle(PillButtonStyle(kind: .quiet, height: 26))
                    .keyboardShortcut(.cancelAction)
                Button(saving ? "Saving…" : "Leave note", action: save)
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 26))
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || saving)
            }
        }
        .padding(12)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .onAppear { focused = true }
    }
}

// MARK: Commits

struct PullCommits: View {
    var detail: PullDetail

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            LazyVStack(alignment: .leading, spacing: 2) {
                ForEach(detail.commits) { c in
                    HStack(spacing: 12) {
                        Text(c.shortSHA)
                            .font(N.mono(12, weight: .medium))
                            .foregroundStyle(N.sky)
                        Text(c.subject)
                            .font(.system(size: 13))
                            .foregroundStyle(N.text)
                            .lineLimit(1)
                        Spacer(minLength: 8)
                        Text(c.author.name)
                            .font(.system(size: 12))
                            .foregroundStyle(N.muted)
                            .lineLimit(1)
                        Text(Format.ago(c.time))
                            .font(N.mono(11))
                            .foregroundStyle(N.faint)
                            .frame(width: 80, alignment: .trailing)
                    }
                    .padding(.horizontal, 14)
                    .frame(height: 36)
                    .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .textSelection(.enabled)
                }
                if detail.commits.count >= 250 {
                    Text("Showing the first 250 commits.")
                        .font(.system(size: 12))
                        .foregroundStyle(N.faint)
                        .padding(14)
                }
            }
            .padding(.horizontal, 28)
            .padding(.vertical, 8)
        }
    }
}

// MARK: - Sheets

private struct Field<Content: View>: View {
    var label: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
            content
        }
    }
}

private extension View {
    func sheetField() -> some View {
        textFieldStyle(.plain)
            .foregroundStyle(N.text)
            .padding(.horizontal, 12)
            .frame(height: 34)
            .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

struct NewPullSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// Start with this branch (on the server) to merge, e.g. from Branches.
    var preferredSource: String? = nil
    @State private var branches: [Branch] = []
    @State private var source = ""
    @State private var target = ""
    @State private var parentPull: UInt32? = nil
    @State private var title = ""
    @State private var bodyText = ""
    @State private var working = false
    @State private var error: String?

    private var openParentCandidates: [PullRequest] {
        model.pullList.filter { $0.state == .open && $0.source != source }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("New pull request")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(N.text)
            HStack(spacing: 10) {
                Field(label: "Merge") { picker($source) }
                Image(systemName: "arrow.right").foregroundStyle(N.faint).padding(.top, 16)
                Field(label: "Into") { picker($target) }
            }
            if !openParentCandidates.isEmpty {
                Field(label: "Stack on PR (optional)") {
                    Picker("", selection: $parentPull) {
                        Text("None (standalone PR)").tag(UInt32?.none)
                        ForEach(openParentCandidates) { p in
                            Text("#\(p.number) \(p.source) — \(p.title)").tag(UInt32?.some(p.number))
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: parentPull) { _, new in
                    if let new, let p = openParentCandidates.first(where: { $0.number == new }) {
                        target = p.source
                    } else if new == nil {
                        let main = model.hostedDefaultBranch ?? "main"
                        target = branches.first { $0.name == main }?.name ?? branches.first?.name ?? ""
                    }
                }
            }
            Field(label: "Title") {
                TextField("", text: $title, prompt: Text("What this changes").foregroundStyle(N.faint))
                    .font(.system(size: 13))
                    .sheetField()
            }
            Field(label: "Description") {
                TextEditor(text: $bodyText)
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.text)
                    .scrollContentBackground(.hidden)
                    .frame(height: 110)
                    .padding(8)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            if let error {
                Text(error).font(.system(size: 12)).foregroundStyle(N.rose).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Text("Branches on \(model.serverLink?.name ?? "the server"). Push yours first.")
                    .font(.system(size: 11))
                    .foregroundStyle(N.faint)
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
                Button(working ? "Opening…" : "Open pull request", action: create)
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                    .keyboardShortcut(.defaultAction)
                    .disabled(working || source.isEmpty || target.isEmpty || source == target || title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(24)
        .frame(width: 520)
        .background(N.sheet)
        .task {
            branches = await model.hostedBranches()
            let main = model.hostedDefaultBranch ?? "main"
            target = branches.first { $0.name == main }?.name ?? branches.first?.name ?? ""
            let candidates = branches.filter { $0.name != target }.sorted { $0.time > $1.time }
            let current = model.source == .local ? model.repo?.head?.label : nil
            source = candidates.first { $0.name == preferredSource }?.name ?? candidates.first { $0.name == current }?.name ?? candidates.first?.name ?? ""
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
                try await model.createPull(title: title, body: bodyText, source: source, target: target, parentPull: parentPull)
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
        VStack(alignment: .leading, spacing: 16) {
            Text("Edit #\(pull.number)")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(N.text)
            Field(label: "Title") { TextField("Title", text: $title).font(.system(size: 13)).sheetField() }
            Field(label: "Description") {
                TextEditor(text: $bodyText)
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.text)
                    .scrollContentBackground(.hidden)
                    .frame(height: 140)
                    .padding(8)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
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
        .padding(24)
        .frame(width: 520)
        .background(N.sheet)
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
        VStack(alignment: .leading, spacing: 16) {
            Text("Branch rules")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(N.text)
            Text("Rules guard branches on \(model.serverLink?.name ?? "the server") when people push. A push can still override one with `git push -o neutrino-override=\"why\"`; overrides are recorded.")
                .font(.system(size: 12))
                .foregroundStyle(N.muted)
                .fixedSize(horizontal: false, vertical: true)
            VStack(spacing: 6) {
                if rules.isEmpty {
                    Text("No rules yet.")
                        .font(.system(size: 12))
                        .foregroundStyle(N.faint)
                        .frame(maxWidth: .infinity, alignment: .leading)
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
                    .padding(.horizontal, 12)
                    .frame(height: 36)
                    .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
            }
            Field(label: "Branch or pattern") {
                TextField("", text: $draft.pattern, prompt: Text("main, release/*").foregroundStyle(N.faint))
                    .font(N.mono(13))
                    .sheetField()
            }
            HStack(spacing: 16) {
                Toggle("Pull requests only", isOn: $draft.requirePull)
                Toggle("No force-push", isOn: $draft.blockForce)
                Toggle("No delete", isOn: $draft.blockDelete)
            }
            .toggleStyle(.checkbox)
            .font(.system(size: 12.5))
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.rose) }
            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
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
        .padding(24)
        .frame(width: 560)
        .background(N.sheet)
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

extension PullList {
    /// Open New pull request when the palette asked for it.
    func takeSheetRequest() {
        guard model.sheetRequest == .newPull else { return }
        model.sheetRequest = nil
        creating = true
    }
}
