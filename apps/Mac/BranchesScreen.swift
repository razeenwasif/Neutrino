import NeutrinoKit
import SwiftUI

struct BranchesScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            BranchList().frame(minWidth: 520, maxWidth: .infinity, maxHeight: .infinity)
            BranchPanel().frame(minWidth: 320, idealWidth: 380, maxWidth: 420, maxHeight: .infinity)
        }
    }
}

struct BranchList: View {
    @Environment(AppModel.self) private var model
    @State private var pruning = false

    /// Local branches already in the default branch, other than the one checked out.
    private var mergedLocal: [Branch] {
        model.source == .local ? model.branches.filter { $0.kind == .local && $0.merged == true && !$0.isHead } : []
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text("Branches").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
                if model.source == .local {
                    Text("ahead and behind their upstream").font(.system(size: 12)).foregroundStyle(N.muted)
                }
                Spacer()
                if !mergedLocal.isEmpty {
                    Button("Delete merged (\(mergedLocal.count))…") { pruning = true }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 26))
                        .disabled(model.writing || !model.isLive)
                        .help("Delete the local branches whose work is already in the default branch")
                }
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 8)

            if let error = model.branchesError {
                EmptyState(title: "Can't read the branches", message: error)
            } else if model.branches.isEmpty {
                EmptyState(title: "No branches yet", message: "Make a first commit to create one.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        let local = model.branches.filter { $0.kind == .local }
                        let remote = model.branches.filter { $0.kind == .remote }
                        let scale = max(1, model.branches.compactMap { max($0.ahead ?? 0, $0.behind ?? 0) }.max() ?? 1)
                        group(model.source == .server ? "On \(model.serverLink?.name ?? "the server")" : "On this Mac", local, scale: scale)
                        if !remote.isEmpty { group("Remote", remote, scale: scale) }
                    }
                    .padding(.horizontal, 6)
                    .padding(.bottom, 8)
                }
            }
        }
        .island()
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") {
            await model.loadRunList()
            await model.refreshGlancePulls()
        }
        .confirmationDialog("Delete \(mergedLocal.count) merged branch\(mergedLocal.count == 1 ? "" : "es")?", isPresented: $pruning) {
            Button("Delete", role: .destructive) {
                Task {
                    var refused: [String] = []
                    for b in mergedLocal {
                        if await model.deleteBranch(b.name, force: false) != nil { refused.append(b.name) }
                    }
                    if !refused.isEmpty { model.notice = "Git kept \(refused.joined(separator: ", ")): it doesn't count as merged from here." }
                }
            }
        } message: {
            Text(mergedLocal.map(\.name).joined(separator: ", ") + "\n\nTheir commits are already in the default branch. Branches on the server aren't touched.")
        }
    }

    @ViewBuilder
    private func group(_ title: String, _ list: [Branch], scale: UInt32) -> some View {
        Text(title).font(.system(size: 12)).foregroundStyle(N.muted).padding(.horizontal, 10).padding(.top, 12).padding(.bottom, 4)
        ForEach(list) { b in
            BranchRow(branch: b, scale: scale, selected: b.id == model.selectedBranch, hosted: model.source == .server)
                .onTapGesture { model.selectedBranch = b.id }
                .onTapGesture(count: 2) { Task { await model.showHistory(of: b) } }
        }
    }
}

struct BranchRow: View {
    var branch: Branch
    var scale: UInt32
    var selected: Bool
    /// A branch of a hosted repo: on the server itself, so there's no upstream to compare with.
    var hosted = false
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 12) {
            Dot(color: branch.isHead ? N.accent : .clear, glow: false, size: 7)
                .help(branch.isHead ? "Checked out" : "")
            HStack(spacing: 6) {
                Text(branch.name)
                    .font(N.mono(12.5, weight: .medium))
                    .foregroundStyle(branch.kind == .remote ? N.second : N.text)
                    .lineLimit(1)
                if branch.merged == true {
                    Text("merged").font(.system(size: 10.5, weight: .medium)).foregroundStyle(N.muted)
                        .padding(.horizontal, 6).frame(height: 17)
                        .background(Color.white.opacity(0.06), in: Capsule())
                        .fixedSize()
                        .help("Its tip is already in the default branch")
                }
            }
            .frame(width: 220, alignment: .leading)
            CheckMark(sha: branch.sha).frame(width: 14)
            Text(branch.subject).font(.system(size: 12.5)).foregroundStyle(N.muted).lineLimit(1)
            Spacer(minLength: 8)
            if !hosted {
            Group {
                if branch.kind == .remote {
                    Color.clear
                } else if branch.ahead == 0, branch.behind == 0 {
                    Text("up to date").font(.system(size: 12)).foregroundStyle(N.faint).frame(maxWidth: .infinity)
                } else {
                    AheadBehind(ahead: branch.ahead, behind: branch.behind, scale: scale)
                }
            }
            .frame(width: 150)
            }
            Text(Format.ago(branch.time)).font(.system(size: 12)).foregroundStyle(N.faint).frame(width: 80, alignment: .trailing)
        }
        .padding(.horizontal, 10)
        .frame(height: 40)
        .background(selected ? N.selected : (hovering ? Color.white.opacity(0.035) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .help("Double-click to show its history")
    }
}

/// Behind on the left, ahead on the right, against the upstream.
struct AheadBehind: View {
    var ahead: UInt32?
    var behind: UInt32?
    var scale: UInt32

    var body: some View {
        if let ahead, let behind {
            HStack(spacing: 5) {
                Text("\(behind)").frame(width: 24, alignment: .trailing).foregroundStyle(N.muted)
                GeometryReader { g in
                    Capsule().fill(Color(hex: 0x6F6880))
                        .frame(width: max(behind > 0 ? 5 : 0, g.size.width * CGFloat(behind) / CGFloat(scale)), height: 6)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .trailing)
                }
                Rectangle().fill(Color(hex: 0x4A4458)).frame(width: 1, height: 14)
                GeometryReader { g in
                    Capsule().fill(N.accent)
                        .frame(width: max(ahead > 0 ? 5 : 0, g.size.width * CGFloat(ahead) / CGFloat(scale)), height: 6)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                }
                Text("\(ahead)").frame(width: 24, alignment: .leading).foregroundStyle(N.text)
            }
            .font(N.mono(11))
            .frame(height: 16)
            .accessibilityElement()
            .accessibilityLabel("\(ahead) ahead, \(behind) behind")
        } else {
            Text("no upstream").font(.system(size: 12)).foregroundStyle(N.faint).frame(maxWidth: .infinity)
        }
    }
}

struct BranchPanel: View {
    @Environment(AppModel.self) private var model
    @State private var opening: String?
    @State private var naming: NameSheet.Purpose?
    @State private var deleting: Branch?
    @State private var unmerged: (branch: Branch, message: String)?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let b = model.branches.first(where: { $0.id == model.selectedBranch }) {
                HStack(spacing: 8) {
                    Text(b.name).font(N.mono(15, weight: .semibold)).foregroundStyle(N.text).textSelection(.enabled)
                    if b.isHead {
                        Text("checked out").font(.system(size: 11, weight: .medium)).foregroundStyle(N.text)
                            .padding(.horizontal, 9).frame(height: 20).background(Color.white.opacity(0.07), in: Capsule())
                    }
                }
                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 8) {
                    GridRow {
                        Text("Tip").foregroundStyle(N.faint)
                        (Text(String(b.sha.prefix(7))).font(N.mono(12)).foregroundStyle(N.muted) + Text("  \(b.subject)"))
                            .foregroundStyle(N.body).lineLimit(2)
                    }
                    GridRow {
                        Text("Updated").foregroundStyle(N.faint)
                        Text(Format.full(b.time))
                    }
                    if b.kind == .local, model.source == .local {
                        GridRow {
                            Text("Upstream").foregroundStyle(N.faint)
                            if let up = b.upstream {
                                if let a = b.ahead, let behind = b.behind {
                                    Text("\(up): \(a) to push, \(behind) to pull")
                                } else {
                                    Text("\(up), which no longer exists")
                                }
                            } else {
                                Text("None. This branch is only on this Mac.").foregroundStyle(N.muted)
                            }
                        }
                    }
                }
                .font(.system(size: 12.5))
                .foregroundStyle(N.body)
                HStack(spacing: 6) {
                    // A hosted repo has no working copy here to switch.
                    if !b.isHead, model.source == .local {
                        Button(b.kind == .remote ? "Check out as local" : "Check out") { Task { await model.checkout(b) } }
                            .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                            .disabled(model.writing || !model.isLive)
                            .help("Switches with git, which refuses if it would overwrite your uncommitted changes.")
                    }
                    Button("Show history") { Task { await model.showHistory(of: b) } }
                        .buttonStyle(PillButtonStyle(kind: b.isHead ? .primary : .secondary, height: 30))
                }
                pullActions(for: b)
                if model.source == .local {
                HStack(spacing: 6) {
                    Button("New branch from here…") { naming = .create(start: b.name) }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                    if b.kind == .local {
                        Button("Rename…") { naming = .rename(b.name) }
                            .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                    }
                    if b.kind == .local, !b.isHead {
                        Button("Delete…") { deleting = b }
                            .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                            .foregroundStyle(N.danger)
                    }
                }
                .disabled(model.writing || !model.isLive)
                }
                Spacer()
            } else {
                EmptyState(title: "No branch selected", message: "Pick a branch to see where it stands.")
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .island()
        .sheet(item: $naming) { purpose in NameSheet(purpose: purpose) }
        .sheet(item: Binding(get: { opening.map(IdentifiedName.init) }, set: { opening = $0?.name })) { n in NewPullSheet(preferredSource: n.name) }
        .confirmationDialog(deleting.map { "Delete \($0.name)?" } ?? "", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), presenting: deleting) { b in
            Button("Delete branch", role: .destructive) {
                Task {
                    if let refusal = await model.deleteBranch(b.name, force: false) {
                        unmerged = (b, refusal)
                    }
                }
            }
        } message: { _ in
            Text("Only a branch that's merged is deleted; git refuses otherwise.")
        }
        .confirmationDialog(unmerged.map { "\($0.branch.name) isn't merged" } ?? "", isPresented: Binding(get: { unmerged != nil }, set: { if !$0 { unmerged = nil } }), presenting: unmerged) { u in
            Button("Delete anyway", role: .destructive) {
                Task {
                    if let refusal = await model.deleteBranch(u.branch.name, force: true) { model.notice = refusal }
                }
            }
        } message: { u in
            Text("\(u.message)\n\nIts commits stay reachable through the reflog for a while, but no branch will point at them.")
        }
    }
}

extension BranchPanel {
    /// The branch's name on the server: origin/x and a local x tracking it are both x.
    func serverName(of b: Branch) -> String? {
        if model.source == .server { return b.kind == .local ? b.name : nil }
        if b.kind == .remote { return b.name.split(separator: "/", maxSplits: 1).last.map(String.init) }
        guard let up = b.upstream, b.ahead != nil else { return nil }
        return up.split(separator: "/", maxSplits: 1).last.map(String.init)
    }

    /// Show the pull request from this branch, or open one.
    @ViewBuilder
    func pullActions(for b: Branch) -> some View {
        let isDefault = b.merged == nil && (b.name == "main" || b.name == "master" || b.name.hasSuffix("/main") || b.name.hasSuffix("/master"))
        if model.serverLink != nil, !isDefault {
            let name = serverName(of: b)
            if let name, let pr = model.glancePulls.first(where: { $0.source == name }) {
                Button("Show pull request #\(pr.number)") {
                    Task {
                        model.section = .pulls
                        await model.select(pull: pr.number)
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                .help(pr.title)
            } else if b.merged == true {
                Text("Merged: its work is already in the default branch.").font(.system(size: 12)).foregroundStyle(N.muted)
            } else {
                Button("Open pull request…") { if let name { opening = name } }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                    .disabled(name == nil || !model.isServerOnline)
                    .help(name == nil ? "Push this branch first; pull requests are made from branches on the server." : "Propose merging \(name ?? "") on \(model.serverLink?.name ?? "the server")")
            }
        }
    }
}

/// Asks for a branch name, to create or rename one.
struct NameSheet: View {
    enum Purpose: Identifiable, Hashable {
        case create(start: String?)
        case rename(String)

        var id: String {
            switch self {
            case let .create(s): "create:\(s ?? "")"
            case let .rename(n): "rename:\(n)"
            }
        }
    }

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    var purpose: Purpose
    @State private var name = ""
    @State private var switchTo = true
    @State private var working = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("feature/thing", text: $name)
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .onSubmit(save)
            }
            if case .create = purpose {
                Toggle("Switch to it", isOn: $switchTo).toggleStyle(.checkbox).font(.system(size: 12.5))
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button(action: save) { Text(working ? "Working…" : (isRename ? "Rename" : "Create")) }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                    .keyboardShortcut(.defaultAction)
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || working)
            }
        }
        .padding(20)
        .frame(width: 380)
        .background(N.island)
        .onAppear { if case let .rename(old) = purpose { name = old } }
    }

    private var isRename: Bool { if case .rename = purpose { true } else { false } }

    private var title: String {
        switch purpose {
        case let .create(start): "New branch from \(start ?? "HEAD")"
        case let .rename(old): "Rename \(old)"
        }
    }

    private func save() {
        let n = name.trimmingCharacters(in: .whitespaces)
        guard !n.isEmpty, !working else { return }
        working = true
        Task {
            let ok: Bool = switch purpose {
            case let .create(start): await model.createBranch(name: n, start: start, checkout: switchTo)
            case let .rename(old): await model.renameBranch(from: old, to: n)
            }
            working = false
            if ok { dismiss() }
        }
    }
}
