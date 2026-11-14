import NeutrinoKit
import SwiftUI

struct CommitPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            if let d = model.detail {
                if !model.wideDiff {
                    CommitHeader(detail: d)
                    // Tall enough for the files, at most about seven rows; the diff gets the rest.
                    FileList(detail: d)
                        .frame(height: min(240, CGFloat(d.files.count) * 31 + 36))
                }
                DiffPane()
            } else if let error = model.detailError {
                EmptyState(title: "Can't read this commit", message: error)
            } else {
                EmptyState(title: "No commit selected", message: "Pick a commit to see what it changed.")
            }
        }
        .island()
    }
}

struct CommitHeader: View {
    @Environment(AppModel.self) private var model
    var detail: CommitDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(detail.subject)
                .font(.system(size: 16, weight: .semibold))
                .foregroundStyle(N.text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if !detail.body.isEmpty {
                ScrollView {
                    Text(detail.body)
                        .font(.system(size: 12.5))
                        .foregroundStyle(N.muted)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 110)
                .fixedSize(horizontal: false, vertical: true)
            }
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 6) {
                GridRow {
                    label("Commit")
                    HStack(spacing: 6) {
                        Text(detail.sha).font(N.mono(12)).foregroundStyle(N.body).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                        Button {
                            model.copy(detail.sha)
                        } label: {
                            Image(systemName: "doc.on.doc").font(.system(size: 10))
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(N.faint)
                        .help("Copy the full hash")
                        .accessibilityLabel("Copy hash")
                    }
                }
                GridRow {
                    label(detail.parents.count > 1 ? "Parents" : "Parent")
                    HStack(spacing: 8) {
                        if detail.parents.isEmpty { Text("None, this is the first commit").foregroundStyle(N.muted) }
                        ForEach(detail.parents, id: \.self) { p in
                            Button(String(p.prefix(7))) { Task { await model.select(commit: p) } }
                                .buttonStyle(.plain)
                                .font(N.mono(12))
                                .foregroundStyle(N.accent)
                                .help("Go to \(p)")
                        }
                        if detail.parents.count > 1 {
                            Text("files compared with the first").font(.system(size: 11.5)).foregroundStyle(N.faint)
                        }
                    }
                }
                GridRow {
                    label("Author")
                    Text("\(detail.author.name)  \(detail.author.email)").foregroundStyle(N.body).lineLimit(1).textSelection(.enabled)
                }
                GridRow {
                    label("Date")
                    Text(Format.full(detail.author.time)).foregroundStyle(N.body)
                }
                if !detail.refs.isEmpty {
                    GridRow {
                        label("Refs")
                        HStack(spacing: 6) { ForEach(detail.refs, id: \.name) { RefChip(label: $0) } }
                    }
                }
            }
            .font(.system(size: 12.5))
        }
        .padding(.horizontal, 16)
        .padding(.top, 16)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func label(_ text: String) -> some View {
        Text(text).foregroundStyle(N.faint).gridColumnAlignment(.leading)
    }
}

struct FileList: View {
    @Environment(AppModel.self) private var model
    var detail: CommitDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                SectionLabel("Files changed · \(detail.files.count)\(detail.truncated ? "+" : "")")
                Spacer()
                let add = detail.files.reduce(UInt32(0)) { $0 + $1.additions }
                let del = detail.files.reduce(UInt32(0)) { $0 + $1.deletions }
                StatCounts(additions: add, deletions: del)
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 6)
            ScrollView {
                LazyVStack(spacing: 1) {
                    ForEach(detail.files) { f in
                        FileRow(file: f, selected: f.path == model.selectedPath)
                            .onTapGesture { Task { await model.select(file: f.path) } }
                    }
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
            .scrollIndicators(.automatic)
        }
    }
}

struct FileRow: View {
    var file: FileChange
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 8) {
            Text(file.status.letter)
                .font(N.mono(10.5, weight: .semibold))
                .foregroundStyle(N.status(file.status))
                .frame(width: 12)
                .help(file.status.label)
            Group {
                if let old = file.oldPath {
                    Text(old).foregroundStyle(N.faint) + Text("  →  ").foregroundStyle(N.faint) + Text(file.path).foregroundStyle(N.body)
                } else {
                    Text(dirname(file.path)).foregroundStyle(N.faint) + Text(basename(file.path)).foregroundStyle(N.body)
                }
            }
            .font(N.mono(12))
            .lineLimit(1)
            .truncationMode(.head)
            Spacer(minLength: 8)
            if file.binary {
                Text("binary").font(.system(size: 11)).foregroundStyle(N.faint)
            } else {
                StatCounts(additions: file.additions, deletions: file.deletions)
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 30)
        .background(selected ? N.selected : (hovering ? Color.white.opacity(0.035) : .clear), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .help(file.oldPath.map { "\($0) → \(file.path)" } ?? file.path)
    }

    private func dirname(_ p: String) -> String {
        guard let i = p.lastIndex(of: "/") else { return "" }
        return String(p[...i])
    }

    private func basename(_ p: String) -> String {
        guard let i = p.lastIndex(of: "/") else { return p }
        return String(p[p.index(after: i)...])
    }
}
