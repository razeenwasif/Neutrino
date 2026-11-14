import NeutrinoKit
import SwiftUI

struct DiffPane: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                if let d = model.diff {
                    Text(d.status.label)
                        .font(N.mono(10.5, weight: .semibold))
                        .foregroundStyle(N.status(d.status))
                        .padding(.horizontal, 7)
                        .frame(height: 20)
                        .background(N.status(d.status).opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
                    Text(d.path).font(N.mono(12.5, weight: .medium)).foregroundStyle(N.text).lineLimit(1).truncationMode(.head)
                    StatCounts(additions: d.additions, deletions: d.deletions)
                } else if model.loadingDiff {
                    ProgressView().controlSize(.small)
                }
                Spacer(minLength: 8)
                Segmented(options: [(DiffLayout.unified, "Unified"), (.split, "Split")], selection: $model.layout)
                Button {
                    model.wideDiff.toggle()
                } label: {
                    Image(systemName: model.wideDiff ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                        .font(.system(size: 11, weight: .semibold))
                }
                .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                .help(model.wideDiff ? "Show the commit list again (⌘⇧D)" : "Give the diff the whole width (⌘⇧D)")
                .accessibilityLabel(model.wideDiff ? "Narrow diff" : "Widen diff")
                .keyboardShortcut("d", modifiers: [.command, .shift])
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)

            Group {
                if let d = model.diff {
                    if d.binary {
                        EmptyState(title: "Binary file", message: "Neutrino doesn't show binary changes yet.")
                    } else if d.tooLarge {
                        EmptyState(title: "Too large to show", message: "One side of this file is over 8 MB.")
                    } else if d.hunks.isEmpty {
                        EmptyState(title: "No line changes", message: d.oldPath != nil ? "Renamed without changes." : "Only the file's mode changed.")
                    } else {
                        DiffBody(diff: d, layout: model.layout)
                    }
                } else if let error = model.diffError {
                    EmptyState(title: "Can't show this diff", message: error)
                } else if model.detail?.files.isEmpty == true {
                    EmptyState(title: "No files changed", message: "This commit changes nothing (an empty commit).")
                } else {
                    Color.clear
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.well)
        }
    }
}

/// A flat list of what the diff draws, so long diffs render lazily.
private enum Row: Identifiable {
    case hunk(Int, String)
    case line(Int, DiffLine)
    case pair(Int, DiffLine?, DiffLine?)

    var id: Int {
        switch self {
        case let .hunk(i, _), let .line(i, _), let .pair(i, _, _): i
        }
    }
}

struct DiffBody: View {
    var diff: FileDiff
    var layout: DiffLayout

    private var rows: [Row] {
        var out: [Row] = []
        var n = 0
        for h in diff.hunks {
            out.append(.hunk(n, h.header)); n += 1
            if layout == .unified {
                for l in h.lines { out.append(.line(n, l)); n += 1 }
                continue
            }
            // Split: removed lines pair with the added lines that follow them.
            var i = 0
            let lines = h.lines
            while i < lines.count {
                if lines[i].kind == .context {
                    out.append(.pair(n, lines[i], lines[i])); n += 1; i += 1
                    continue
                }
                var removed: [DiffLine] = [], added: [DiffLine] = []
                while i < lines.count, lines[i].kind == .removed { removed.append(lines[i]); i += 1 }
                while i < lines.count, lines[i].kind == .added { added.append(lines[i]); i += 1 }
                for k in 0 ..< max(removed.count, added.count) {
                    out.append(.pair(n, k < removed.count ? removed[k] : nil, k < added.count ? added[k] : nil)); n += 1
                }
            }
        }
        return out
    }

    var body: some View {
        GeometryReader { geo in
        ScrollView(layout == .unified ? [.vertical, .horizontal] : .vertical) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(rows) { row in
                    switch row {
                    case let .hunk(_, header):
                        Text(header)
                            .font(N.mono(12))
                            .foregroundStyle(N.muted)
                            .padding(.horizontal, 14)
                            .frame(maxWidth: .infinity, minHeight: 24, alignment: .leading)
                            .background(N.accent.opacity(0.07))
                            .padding(.top, 6)
                    case let .line(_, l):
                        UnifiedLine(line: l)
                    case let .pair(_, a, b):
                        HStack(spacing: 0) {
                            SideLine(line: a, number: a?.oldLine, isOld: true)
                            Rectangle().fill(N.hairline).frame(width: 1)
                            SideLine(line: b, number: b?.newLine, isOld: false)
                        }
                    }
                }
            }
            // Rows span at least the visible width, so line colours reach the edge.
            .frame(minWidth: geo.size.width, alignment: .leading)
            .padding(.bottom, 14)
            .textSelection(.enabled)
        }
        }
    }
}

private func colors(_ kind: LineKind) -> (bg: Color, sign: Color, text: Color, emph: Color) {
    switch kind {
    case .added: (N.ok.opacity(0.09), N.ok, N.text, N.ok.opacity(0.26))
    case .removed: (N.danger.opacity(0.09), N.danger, N.text, N.danger.opacity(0.28))
    case .context: (.clear, N.faint, Color(hex: 0xB9B2C6), .clear)
    }
}

private func styled(_ line: DiffLine) -> AttributedString {
    let c = colors(line.kind)
    if line.segments.isEmpty {
        var s = AttributedString(line.text.isEmpty ? " " : line.text)
        s.foregroundColor = c.text
        return s
    }
    var out = AttributedString()
    for seg in line.segments {
        var part = AttributedString(seg.text)
        part.foregroundColor = c.text
        if seg.changed { part.backgroundColor = c.emph }
        out += part
    }
    return out
}

private struct UnifiedLine: View {
    var line: DiffLine

    var body: some View {
        let c = colors(line.kind)
        HStack(spacing: 0) {
            Text(line.oldLine.map(String.init) ?? "").frame(width: 46, alignment: .trailing).foregroundStyle(N.faint)
            Text(line.newLine.map(String.init) ?? "").frame(width: 44, alignment: .trailing).foregroundStyle(N.faint)
            Text(line.kind == .added ? "+" : line.kind == .removed ? "−" : " ").frame(width: 22).foregroundStyle(c.sign)
            Text(styled(line)).fixedSize(horizontal: true, vertical: false)
            if line.noNewline {
                Text("  no newline at end").foregroundStyle(N.faint).italic()
            }
            Spacer(minLength: 16)
        }
        .font(N.mono(12))
        .frame(maxWidth: .infinity, minHeight: 20, alignment: .leading)
        .background(c.bg)
    }
}

private struct SideLine: View {
    var line: DiffLine?
    var number: UInt32?
    var isOld: Bool

    var body: some View {
        let c = line.map { colors($0.kind) } ?? (Color.white.opacity(0.015), N.faint, N.faint, .clear)
        HStack(spacing: 0) {
            Text(number.map(String.init) ?? "").frame(width: 46, alignment: .trailing).foregroundStyle(N.faint)
            Text(line.map { $0.kind == .added ? "+" : $0.kind == .removed ? "−" : " " } ?? "").frame(width: 22).foregroundStyle(c.sign)
            if let line { Text(styled(line)).lineLimit(1).truncationMode(.tail) }
            Spacer(minLength: 0)
        }
        .font(N.mono(12))
        .frame(maxWidth: .infinity, minHeight: 20, alignment: .leading)
        .background(c.bg)
        .clipped()
    }
}
