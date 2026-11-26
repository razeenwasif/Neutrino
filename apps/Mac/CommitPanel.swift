import NeutrinoKit
import SwiftUI

struct CommitPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            if let d = model.detail {
                if !model.wideDiff {
                    CommitHeader(detail: d)
                    FileList(detail: d)
                        .frame(height: min(220, CGFloat(d.files.count) * 31 + 38))
                }
                DiffPane()
            } else if let error = model.detailError {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't read this commit", message: error)
            } else {
                EmptyState(icon: "clock.arrow.circlepath", title: "No commit selected", message: "Pick a commit to see what it changed.")
            }
        }
        .background(N.inspector)
    }
}

struct CommitHeader: View {
    @Environment(AppModel.self) private var model
    var detail: CommitDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(detail.subject)
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(N.text)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            if !detail.body.isEmpty {
                ScrollView(.vertical, showsIndicators: false) {
                    Text(detail.body)
                        .font(.system(size: 12.5))
                        .foregroundStyle(N.muted)
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 96)
                .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Text("Commit")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.faint)
                        .frame(width: 56, alignment: .leading)
                    Text(detail.sha)
                        .font(N.mono(11.5))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                    Spacer(minLength: 4)
                    Button {
                        model.copy(detail.sha, label: "commit SHA")
                    } label: {
                        Image(systemName: "doc.on.doc")
                            .font(.system(size: 10.5))
                            .foregroundStyle(N.sky)
                    }
                    .buttonStyle(.plain)
                    .help("Copy the full hash")
                    .accessibilityLabel("Copy hash")
                }

                HStack(spacing: 8) {
                    Text(detail.parents.count > 1 ? "Parents" : "Parent")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.faint)
                        .frame(width: 56, alignment: .leading)
                    if detail.parents.isEmpty {
                        Text("Root commit")
                            .font(.system(size: 12))
                            .foregroundStyle(N.muted)
                    } else {
                        ForEach(detail.parents, id: \.self) { p in
                            Button(String(p.prefix(7))) { Task { await model.select(commit: p) } }
                                .buttonStyle(.plain)
                                .font(N.mono(11.5, weight: .medium))
                                .foregroundStyle(N.sky)
                                .help("Go to \(p)")
                        }
                        if detail.parents.count > 1 {
                            Text("vs first parent")
                                .font(.system(size: 11))
                                .foregroundStyle(N.faint)
                        }
                    }
                    Spacer()
                }

                HStack(spacing: 8) {
                    Text("Author")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.faint)
                        .frame(width: 56, alignment: .leading)
                    Text("\(detail.author.name) · \(detail.author.email)")
                        .font(.system(size: 12))
                        .foregroundStyle(N.body)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                    Spacer()
                }

                HStack(spacing: 8) {
                    Text("Date")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.faint)
                        .frame(width: 56, alignment: .leading)
                    Text(Format.full(detail.author.time))
                        .font(N.mono(11.5))
                        .foregroundStyle(N.body)
                    Spacer()
                }

                if !detail.refs.isEmpty {
                    HStack(spacing: 8) {
                        Text("Refs")
                            .font(.system(size: 11.5))
                            .foregroundStyle(N.faint)
                            .frame(width: 56, alignment: .leading)
                        HStack(spacing: 6) {
                            ForEach(detail.refs, id: \.name) { RefChip(label: $0) }
                        }
                        Spacer()
                    }
                }

                let checks = model.checkRuns(for: detail.sha)
                if !checks.isEmpty {
                    HStack(alignment: .top, spacing: 8) {
                        Text("Checks")
                            .font(.system(size: 11.5))
                            .foregroundStyle(N.faint)
                            .frame(width: 56, alignment: .leading)
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(checks) { r in
                                Button {
                                    Task { await model.openRun(r.id) }
                                } label: {
                                    HStack(spacing: 6) {
                                        RunStateIcon(state: r.state, size: 11)
                                        Text(r.name)
                                            .font(.system(size: 12, weight: .medium))
                                            .foregroundStyle(N.text)
                                        Text("\(r.state.label) · #\(r.number)")
                                            .font(N.mono(11))
                                            .foregroundStyle(N.faint)
                                    }
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .help("Open run \(r.number)")
                            }
                        }
                        Spacer()
                    }
                }
            }
            .padding(12)
            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        }
        .padding(.horizontal, 18)
        .padding(.top, 18)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct FileList: View {
    @Environment(AppModel.self) private var model
    var detail: CommitDetail

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                SectionLabel("Files changed · \(detail.files.count)\(detail.truncated ? "+" : "")")
                Spacer()
                let add = detail.files.reduce(UInt32(0)) { $0 + $1.additions }
                let del = detail.files.reduce(UInt32(0)) { $0 + $1.deletions }
                StatCounts(additions: add, deletions: del)
            }
            .padding(.horizontal, 18)

            ScrollView(.vertical, showsIndicators: false) {
                LazyVStack(spacing: 1) {
                    ForEach(detail.files) { f in
                        FileRow(file: f, selected: f.path == model.selectedPath)
                            .onTapGesture { Task { await model.select(file: f.path) } }
                    }
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 8)
            }
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
                    Text(old).foregroundStyle(N.faint) + Text("  →  ").foregroundStyle(N.faint) + Text(file.path).foregroundStyle(N.text)
                } else {
                    Text(dirname(file.path)).foregroundStyle(N.faint) + Text(basename(file.path)).foregroundStyle(N.text)
                }
            }
            .font(N.mono(12))
            .lineLimit(1)
            .truncationMode(.head)
            Spacer(minLength: 8)
            if file.binary {
                Text("binary")
                    .font(N.mono(10.5))
                    .foregroundStyle(N.faint)
            } else {
                StatCounts(additions: file.additions, deletions: file.deletions)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(
            selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
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
