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

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text("Branches").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
                Text("ahead and behind their upstream").font(.system(size: 12)).foregroundStyle(N.muted)
                Spacer()
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
                        group("On this Mac", local, scale: scale)
                        if !remote.isEmpty { group("Remote", remote, scale: scale) }
                    }
                    .padding(.horizontal, 6)
                    .padding(.bottom, 8)
                }
            }
        }
        .island()
    }

    @ViewBuilder
    private func group(_ title: String, _ list: [Branch], scale: UInt32) -> some View {
        Text(title).font(.system(size: 12)).foregroundStyle(N.muted).padding(.horizontal, 10).padding(.top, 12).padding(.bottom, 4)
        ForEach(list) { b in
            BranchRow(branch: b, scale: scale, selected: b.id == model.selectedBranch)
                .onTapGesture { model.selectedBranch = b.id }
                .onTapGesture(count: 2) { Task { await model.showHistory(of: b) } }
        }
    }
}

struct BranchRow: View {
    var branch: Branch
    var scale: UInt32
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 12) {
            Dot(color: branch.isHead ? N.accent : .clear, glow: false, size: 7)
                .help(branch.isHead ? "Checked out" : "")
            Text(branch.name)
                .font(N.mono(12.5, weight: .medium))
                .foregroundStyle(branch.kind == .remote ? N.second : N.text)
                .lineLimit(1)
                .frame(width: 220, alignment: .leading)
            Text(branch.subject).font(.system(size: 12.5)).foregroundStyle(N.muted).lineLimit(1)
            Spacer(minLength: 8)
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
                    if b.kind == .local {
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
                Button("Show history") { Task { await model.showHistory(of: b) } }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                Spacer()
            } else {
                EmptyState(title: "No branch selected", message: "Pick a branch to see where it stands.")
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .island()
    }
}
