import NeutrinoKit
import SwiftUI

/// A pull request: what it changes, its checks, and merging it.
struct PhonePullView: View {
    @Environment(PhoneModel.self) private var model
    var ref: PullRef
    @State private var detail: PullDetail?
    @State private var error: String?
    @State private var strategy: MergeStrategy = .merge
    @State private var deleteSource = true

    var body: some View {
        List {
            if let d = detail {
                let p = d.pull
                Section {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(p.title).font(.title3.weight(.semibold))
                        Text("\(p.source) → \(p.target)").font(.caption.monospaced()).foregroundStyle(N.accent)
                        Text("\(p.state.label) · \(p.versions) version\(p.versions == 1 ? "" : "s") · opened \(Format.ago(p.createdAt))").font(.caption).foregroundStyle(.secondary)
                        if !p.body.isEmpty { Text(p.body).font(.subheadline).padding(.top, 4) }
                    }
                }
                if p.state == .open {
                    Section("Merge") {
                        if let m = d.mergeability {
                            Label(m.clean ? (m.fastForward ? "Can fast-forward" : "Merges cleanly") : "Conflicts in \(m.conflicts.joined(separator: ", "))",
                                  systemImage: m.clean ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                                .foregroundStyle(m.clean ? N.ok : N.danger)
                        }
                        let missing = missingChecks(d.rule?.requiredChecks ?? [], in: d.checks)
                        if !missing.isEmpty {
                            Label("Needs \(missing.joined(separator: ", "))", systemImage: "lock.shield").foregroundStyle(N.warn)
                        }
                        Picker("Strategy", selection: $strategy) {
                            ForEach(MergeStrategy.allCases) { Text($0.label).tag($0) }
                        }
                        Toggle("Delete \(p.source) afterwards", isOn: $deleteSource)
                        Button {
                            Task {
                                if await model.merge(p, strategy: strategy, deleteSource: deleteSource) { await load() }
                            }
                        } label: {
                            Label(model.busy == "Merging" ? "Merging…" : "Merge", systemImage: "faceid")
                        }
                        .disabled(d.mergeability?.clean != true || !missing.isEmpty || model.busy != nil)
                    }
                } else if let sha = p.mergeSha {
                    Section { Label("Merged as \(sha.prefix(7))", systemImage: "checkmark.seal.fill").foregroundStyle(N.accent) }
                }
                if !d.checks.isEmpty {
                    Section("Checks") {
                        ForEach(d.checks) { r in
                            NavigationLink(value: RunRef(repo: r.repo, id: r.id)) { PhoneRunRow(run: r) }
                        }
                    }
                }
                Section("Files (\(d.files.count))") {
                    ForEach(d.files) { f in
                        NavigationLink {
                            PhoneDiffView(repo: ref.repo, base: p.state == .open ? p.target : (d.latest?.baseSha ?? p.target), head: d.latest?.headSha ?? p.headSha,
                                          path: f.path, notes: d.notes.filter { $0.path == f.path })
                        } label: {
                            HStack {
                                Text(f.status.letter).font(.caption.monospaced().weight(.semibold)).foregroundStyle(N.status(f.status))
                                Text(f.path).font(.caption.monospaced()).lineLimit(1).truncationMode(.head)
                                Spacer()
                                if d.notes.contains(where: { $0.path == f.path && !$0.resolved }) {
                                    Image(systemName: "text.bubble").font(.caption2).foregroundStyle(N.warn)
                                }
                                Text("+\(f.additions) −\(f.deletions)").font(.caption2.monospaced()).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
                if !d.commits.isEmpty {
                    Section("Commits") {
                        ForEach(d.commits) { c in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(c.subject).font(.subheadline).lineLimit(2)
                                Text("\(c.shortSHA) · \(c.author.name)").font(.caption.monospaced()).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            } else if let error {
                ContentUnavailableView("Can't load it", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView()
            }
        }
        .navigationTitle("#\(ref.number)")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        guard let service = model.service else { return }
        do {
            detail = try await service.pull(repo: ref.repo, number: ref.number)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// One file's diff, unified, with its notes under their lines.
struct PhoneDiffView: View {
    @Environment(PhoneModel.self) private var model
    var repo: String
    var base: String
    var head: String
    var path: String
    var notes: [Note]
    @State private var diff: FileDiff?
    @State private var error: String?

    var body: some View {
        ScrollView([.vertical, .horizontal]) {
            LazyVStack(alignment: .leading, spacing: 0) {
                if let d = diff {
                    if d.binary || d.tooLarge {
                        Text(d.binary ? "Binary file" : "Too large to show").foregroundStyle(.secondary).padding()
                    }
                    ForEach(Array(d.hunks.enumerated()), id: \.offset) { _, h in
                        Text(h.header).font(.caption.monospaced()).foregroundStyle(.secondary).padding(.horizontal, 8).padding(.vertical, 4)
                        ForEach(Array(h.lines.enumerated()), id: \.offset) { _, l in
                            line(l)
                            ForEach(notesAt(l)) { n in
                                Text(n.text).font(.caption).padding(8)
                                    .background(N.raised, in: RoundedRectangle(cornerRadius: 8))
                                    .padding(.leading, 40).padding(.vertical, 2)
                            }
                        }
                    }
                } else if let error {
                    Text(error).foregroundStyle(N.danger).padding()
                } else {
                    ProgressView().padding()
                }
            }
        }
        .background(N.well)
        .navigationTitle((path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            guard let service = model.service else { return }
            do {
                diff = try await service.compareDiff(repo: repo, base: base, head: head, path: path)
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func notesAt(_ l: DiffLine) -> [Note] {
        let side: Side = l.kind == .removed ? .old : .new
        let n = l.kind == .removed ? l.oldLine : l.newLine
        return notes.filter { $0.side == side && $0.currentLine == n }
    }

    private func line(_ l: DiffLine) -> some View {
        let (sign, bg): (String, Color) = switch l.kind {
        case .added: ("+", N.ok.opacity(0.12))
        case .removed: ("−", N.danger.opacity(0.12))
        case .context: (" ", .clear)
        }
        return HStack(spacing: 6) {
            Text((l.kind == .removed ? l.oldLine : l.newLine).map(String.init) ?? "").frame(width: 32, alignment: .trailing).foregroundStyle(.tertiary)
            Text(sign).foregroundStyle(l.kind == .added ? N.ok : N.danger)
            Text(l.text.isEmpty ? " " : l.text)
        }
        .font(.system(size: 11, design: .monospaced))
        .padding(.horizontal, 6)
        .frame(minHeight: 18, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(bg)
    }
}
