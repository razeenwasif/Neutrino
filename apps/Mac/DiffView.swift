import NeutrinoKit
import SwiftUI

struct DiffPane: View {
    @Environment(AppModel.self) private var model
    @State private var wideHovered = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                if let d = model.diff {
                    TagPill(
                        text: d.status.label,
                        textColor: N.status(d.status),
                        background: N.status(d.status).opacity(0.14),
                        monospaced: true
                    )
                    Text(d.path)
                        .font(N.mono(12, weight: .medium))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                        .truncationMode(.head)
                    StatCounts(additions: d.additions, deletions: d.deletions)
                } else if model.loadingDiff {
                    ProgressView().controlSize(.small)
                }
                Spacer(minLength: 8)
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

                Button {
                    model.wideDiff.toggle()
                } label: {
                    Image(systemName: model.wideDiff ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(wideHovered ? N.text : N.muted)
                        .frame(width: 28, height: 28)
                        .background(wideHovered ? N.surfaceActive : N.surfacePill, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                }
                .buttonStyle(.plain)
                .onHover { wideHovered = $0 }
                .help(model.wideDiff ? "Show the commit list again (⌘⇧D)" : "Give the diff the whole width (⌘⇧D)")
                .accessibilityLabel(model.wideDiff ? "Narrow diff" : "Widen diff")
                .keyboardShortcut("d", modifiers: [.command, .shift])
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 10)
            .background(N.inspector)

            Group {
                if let d = model.diff {
                    if d.binary {
                        EmptyState(icon: "doc.zipper", title: "Binary file", message: "Neutrino doesn't show binary changes yet.")
                    } else if d.tooLarge {
                        EmptyState(icon: "doc.text.magnifyingglass", title: "Too large to show", message: "One side of this file is over 8 MB.")
                    } else if d.hunks.isEmpty {
                        EmptyState(icon: "doc.text", title: "No line changes", message: d.oldPath != nil ? "Renamed without changes." : "Only the file's mode changed.")
                    } else {
                        DiffBody(diff: d, layout: model.layout)
                    }
                } else if let error = model.diffError {
                    EmptyState(icon: "exclamationmark.triangle", title: "Can't show this diff", message: error)
                } else if model.detail?.files.isEmpty == true {
                    EmptyState(icon: "doc", title: "No files changed", message: "This commit changes nothing (an empty commit).")
                } else {
                    Color.clear
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(N.ground)
        }
    }
}

/// A flat list of what the diff draws, so long diffs render lazily.
private enum Row: Identifiable {
    case hunk(Int, Hunk)
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
    /// Lines picked for staging; empty and unused in read-only diffs.
    var picks: Set<LinePick> = []
    /// Set to make changed lines pickable.
    var onPick: ((DiffLine) -> Void)? = nil
    /// A button on each hunk header, e.g. "Stage hunk".
    var hunkAction: String? = nil
    var onHunk: ((Hunk) -> Void)? = nil
    /// Unchanged lines can be clicked too (to leave a note on them).
    var pickContext = false
    /// Drawn under a line: its notes, say.
    var footer: ((DiffLine) -> AnyView?)? = nil

    private func picked(_ l: DiffLine?) -> Bool {
        guard let l, let p = LinePick(l) else { return false }
        return picks.contains(p)
    }

    private var rows: [Row] {
        var out: [Row] = []
        var n = 0
        for h in diff.hunks {
            out.append(.hunk(n, h)); n += 1
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
                        case let .hunk(_, h):
                            HStack(spacing: 10) {
                                Rectangle()
                                    .fill(N.indigo)
                                    .frame(width: 2, height: 14)
                                Text(h.header)
                                    .font(N.mono(11.5))
                                    .foregroundStyle(N.sky)
                                if let hunkAction, let onHunk {
                                    Button(hunkAction) { onHunk(h) }
                                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 22))
                                        .font(.system(size: 11))
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(.horizontal, 14)
                            .frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                            .background(N.surfaceSubtle)
                            .padding(.top, 8)
                        case let .line(_, l):
                            UnifiedLine(line: l, picked: picked(l), onPick: onPick, pickContext: pickContext)
                            if let view = footer?(l) { view }
                        case let .pair(_, a, b):
                            HStack(spacing: 0) {
                                SideLine(line: a, number: a?.oldLine, isOld: true, picked: picked(a), onPick: onPick, pickContext: pickContext)
                                Rectangle().fill(N.surfaceSubtle).frame(width: 1)
                                SideLine(line: b, number: b?.newLine, isOld: false, picked: picked(b), onPick: onPick, pickContext: pickContext)
                            }
                            if let a, a.kind == .removed, let view = footer?(a) { view }
                            if let b, let view = footer?(b) { view }
                        }
                    }
                }
                .frame(minWidth: geo.size.width, alignment: .leading)
                .padding(.bottom, 16)
                .modifier(SelectableText(on: onPick == nil))
            }
        }
        .background(N.ground)
    }
}

/// A changed line that can be picked reads as a toggle to VoiceOver and other
/// assistive tools, with the same action a click has.
private struct PickableLine: ViewModifier {
    var line: DiffLine?
    var picked: Bool
    var onPick: ((DiffLine) -> Void)?

    func body(content: Content) -> some View {
        if let line, let onPick {
            let side = switch line.kind {
            case .added: "Added line \(line.newLine ?? 0)"
            case .removed: "Removed line \(line.oldLine ?? 0)"
            case .context: "Line \(line.newLine ?? 0)"
            }
            content
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(side): \(line.text)")
                .accessibilityValue(picked ? "picked" : "not picked")
                .accessibilityAddTraits(picked ? [.isButton, .isSelected] : .isButton)
                .accessibilityAction { onPick(line) }
        } else {
            content
        }
    }
}

/// Text selection, off where clicks pick lines instead.
private struct SelectableText: ViewModifier {
    var on: Bool

    func body(content: Content) -> some View {
        if on { content.textSelection(.enabled) } else { content.textSelection(.disabled) }
    }
}

private func colors(_ kind: LineKind) -> (bg: Color, sign: Color, text: Color, emph: Color) {
    switch kind {
    case .added: (N.mint.opacity(0.08), N.mint, N.text, N.mint.opacity(0.24))
    case .removed: (N.rose.opacity(0.08), N.rose, N.text, N.rose.opacity(0.24))
    case .context: (.clear, N.faint, N.body, .clear)
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
    var picked = false
    var onPick: ((DiffLine) -> Void)? = nil
    var pickContext = false

    var body: some View {
        let c = colors(line.kind)
        let pickable = onPick != nil && (line.kind != .context || pickContext)
        HStack(spacing: 0) {
            Rectangle().fill(picked ? N.indigo : .clear).frame(width: 3)
            Text(line.oldLine.map(String.init) ?? "")
                .frame(width: 46, alignment: .trailing)
                .foregroundStyle(N.faint)
            Text(line.newLine.map(String.init) ?? "")
                .frame(width: 44, alignment: .trailing)
                .foregroundStyle(N.faint)
            Text(line.kind == .added ? "+" : line.kind == .removed ? "−" : " ")
                .frame(width: 22)
                .foregroundStyle(c.sign)
            Text(styled(line)).fixedSize(horizontal: true, vertical: false)
            if line.noNewline {
                Text("  no newline at end")
                    .foregroundStyle(N.faint)
                    .italic()
            }
            Spacer(minLength: 16)
        }
        .font(N.mono(12).monospacedDigit())
        .frame(maxWidth: .infinity, minHeight: 20, alignment: .leading)
        .background(picked ? N.indigo.opacity(0.16) : c.bg)
        .contentShape(Rectangle())
        .onTapGesture { if pickable { onPick?(line) } }
        .help(pickable ? (pickContext ? "Click to leave a note on this line" : (picked ? "Picked. Click to drop it." : "Click to pick this line")) : "")
        .modifier(PickableLine(line: line, picked: picked, onPick: pickable ? onPick : nil))
    }
}

private struct SideLine: View {
    var line: DiffLine?
    var number: UInt32?
    var isOld: Bool
    var picked = false
    var onPick: ((DiffLine) -> Void)? = nil
    var pickContext = false

    var body: some View {
        let c = line.map { colors($0.kind) } ?? (N.sidebar, N.faint, N.faint, .clear)
        HStack(spacing: 0) {
            Text(number.map(String.init) ?? "")
                .frame(width: 46, alignment: .trailing)
                .foregroundStyle(N.faint)
            Text(line.map { $0.kind == .added ? "+" : $0.kind == .removed ? "−" : " " } ?? "")
                .frame(width: 22)
                .foregroundStyle(c.sign)
            if let line {
                Text(styled(line))
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 0)
        }
        .font(N.mono(12).monospacedDigit())
        .frame(maxWidth: .infinity, minHeight: 20, alignment: .leading)
        .background(picked ? N.indigo.opacity(0.16) : c.bg)
        .overlay(alignment: .leading) {
            if picked { Rectangle().fill(N.indigo).frame(width: 3) }
        }
        .clipped()
        .contentShape(Rectangle())
        .onTapGesture { if let line, line.kind != .context || pickContext { onPick?(line) } }
        .modifier(PickableLine(line: line, picked: picked, onPick: line?.kind == .context && !pickContext ? nil : onPick))
    }
}
