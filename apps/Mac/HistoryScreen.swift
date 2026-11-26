import NeutrinoKit
import SwiftUI

struct HistoryScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 0) {
            if !model.wideDiff {
                CommitList()
                    .frame(minWidth: 400, maxWidth: .infinity, maxHeight: .infinity)
            }
            CommitPanel()
                .frame(minWidth: 460, idealWidth: 580, maxWidth: model.wideDiff ? .infinity : 660, maxHeight: .infinity)
        }
    }
}

// MARK: - Commit list

struct CommitList: View {
    @Environment(AppModel.self) private var model
    @FocusState private var listFocused: Bool
    @FocusState private var searchFocused: Bool

    /// Lanes drawn before the graph is cut off; very wide histories scroll out of view.
    private let maxLanes: UInt16 = 10

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("History")
                        .font(N.pageTitle())
                        .foregroundStyle(N.text)
                    HStack(spacing: 8) {
                        TagPill(text: model.historyTitle, color: N.sky, textColor: N.text, monospaced: true)
                        if !model.commits.isEmpty {
                            Text("\(model.commits.count)\(model.hasMore ? "+" : "") commits")
                                .font(N.mono(11))
                                .foregroundStyle(N.faint)
                            if let ms = model.historyMillis {
                                Text(ms < 1 ? "<1 ms" : "\(ms) ms")
                                    .font(N.mono(10.5))
                                    .foregroundStyle(N.faint)
                            }
                        }
                        if model.rev != nil, model.scope == .current {
                            ActionPillButton(title: "Back to HEAD", systemImage: "arrow.uturn.backward", tint: N.sky) {
                                model.rev = nil
                                Task { await model.reloadHistory() }
                            }
                        }
                    }
                }

                Spacer(minLength: 8)

                HStack(spacing: 2) {
                    FilterPillButton(title: "Branch", isSelected: model.scope == .current) {
                        model.scope = .current
                    }
                    FilterPillButton(title: "All", isSelected: model.scope == .all) {
                        model.scope = .all
                    }
                }
                .padding(2)
                .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .help("This branch, or every branch and tag")

                BorderlessSearchField(
                    text: $model.query,
                    placeholder: "Message, author, SHA...",
                    width: 190,
                    focused: $searchFocused
                )
            }
            .padding(.horizontal, 24)
            .padding(.top, 18)
            .padding(.bottom, 14)

            if model.repo == nil {
                EmptyState(icon: "folder", title: "No repository selected", message: "Pick one from the sidebar.")
            } else if let error = model.historyError {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't read the history", message: error)
            } else if model.commits.isEmpty, !model.loadingHistory {
                EmptyState(
                    icon: "clock.arrow.circlepath",
                    title: model.query.isEmpty ? "No commits yet" : "Nothing matches",
                    message: model.query.isEmpty ? "This branch has no commits." : "No commit's message, author or hash contains \"\(model.query)\"."
                )
            } else {
                list
            }
        }
        .background(N.ground)
        .onChange(of: model.scope) { Task { await model.reloadHistory() } }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") {
            await model.loadRunList()
        }
        .task(id: model.query) {
            // Wait for typing to pause.
            try? await Task.sleep(for: .milliseconds(220))
            guard !Task.isCancelled else { return }
            await model.reloadHistory()
        }
        .background {
            Button("") { searchFocused = true }.keyboardShortcut("f", modifiers: .command).hidden()
        }
    }

    private var list: some View {
        let lanes = min(model.lanes, maxLanes)
        return GeometryReader { geo in
            ScrollViewReader { proxy in
                ScrollView(.vertical, showsIndicators: false) {
                    LazyVStack(spacing: 1) {
                        ForEach(model.commits) { c in
                            CommitRow(commit: c, lanes: lanes, selected: c.sha == model.selectedSHA, showAuthor: geo.size.width > 740)
                                .id(c.sha)
                                .onTapGesture {
                                    listFocused = true
                                    Task { await model.select(commit: c.sha) }
                                }
                                .onAppear {
                                    if c.sha == model.commits.last?.sha { Task { await model.loadMore() } }
                                }
                        }
                        if model.hasMore {
                            ProgressView().controlSize(.small).padding(14)
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 24)
                }
                .focusable()
                .focused($listFocused)
                .focusEffectDisabled()
                .onKeyPress(.downArrow) {
                    Task {
                        await model.moveSelection(by: 1)
                        if let s = model.selectedSHA { proxy.scrollTo(s) }
                    }
                    return .handled
                }
                .onKeyPress(.upArrow) {
                    Task {
                        await model.moveSelection(by: -1)
                        if let s = model.selectedSHA { proxy.scrollTo(s) }
                    }
                    return .handled
                }
            }
        }
    }
}

struct CommitRow: View {
    var commit: LogEntry
    var lanes: UInt16
    var selected: Bool
    /// Left out when the list is narrow.
    var showAuthor = true
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 10) {
            if let row = commit.graph {
                GraphCell(row: row, lanes: lanes, isMerge: commit.isMerge, isHead: commit.refs.contains(where: \.isHead), selected: selected)
            }
            Text(commit.subject)
                .font(.system(size: 13, weight: selected ? .medium : .regular))
                .foregroundStyle(selected ? N.text : N.body)
                .lineLimit(1)
            ForEach(commit.refs.prefix(3), id: \.name) { RefChip(label: $0).fixedSize() }
            if commit.refs.count > 3 {
                Text("+\(commit.refs.count - 3)")
                    .font(N.mono(11))
                    .foregroundStyle(N.faint)
                    .fixedSize()
                    .help(commit.refs.dropFirst(3).map(\.name).joined(separator: ", "))
            }
            Spacer(minLength: 8)
            if showAuthor {
                Text(commit.author.name)
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
                    .lineLimit(1)
                    .frame(width: 116, alignment: .trailing)
            }
            CheckMark(sha: commit.sha).frame(width: 14)
            Text(commit.shortSHA)
                .font(N.mono(11.5))
                .foregroundStyle(selected ? N.sky : N.muted)
                .fixedSize()
            Text(Format.ago(commit.time))
                .font(N.mono(11))
                .foregroundStyle(N.faint)
                .lineLimit(1)
                .frame(width: 74, alignment: .trailing)
                .help(Format.full(commit.time))
        }
        .padding(.leading, 8)
        .padding(.trailing, 12)
        .frame(height: GraphCell.rowHeight)
        .background(
            selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .onHover { hovering = $0 }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isSelected, .isButton] : .isButton)
    }
}

/// Draws one row of the commit graph from neutrinod's lane layout.
struct GraphCell: View {
    static let rowHeight: CGFloat = 36
    static let laneWidth: CGFloat = 14

    var row: GraphRow
    var lanes: UInt16
    var isMerge: Bool
    var isHead: Bool
    var selected = false

    var body: some View {
        let width = CGFloat(max(lanes, 1)) * Self.laneWidth + 6
        Canvas { ctx, size in
            let h = size.height
            let x = { (lane: UInt16) in CGFloat(lane) * Self.laneWidth + Self.laneWidth / 2 + 3 }
            func stroke(_ s: GraphSegment, top: CGFloat, bottom: CGFloat) {
                guard s.from < lanes || s.to < lanes else { return }
                var p = Path()
                p.move(to: CGPoint(x: x(s.from), y: top))
                if s.from == s.to {
                    p.addLine(to: CGPoint(x: x(s.to), y: bottom))
                } else {
                    let mid = (top + bottom) / 2
                    p.addCurve(to: CGPoint(x: x(s.to), y: bottom), control1: CGPoint(x: x(s.from), y: mid), control2: CGPoint(x: x(s.to), y: mid))
                }
                ctx.stroke(p, with: .color(N.lane(s.color)), style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
            }
            for s in row.upper { stroke(s, top: 0, bottom: h / 2) }
            for s in row.lower { stroke(s, top: h / 2, bottom: h) }
            guard row.lane < lanes else { return }
            let r: CGFloat = isHead ? 5 : 4
            let dot = CGRect(x: x(row.lane) - r, y: h / 2 - r, width: r * 2, height: r * 2)
            let color = N.lane(row.color)
            ctx.fill(Path(ellipseIn: dot), with: .color(isMerge ? color : (selected ? N.surfaceActive : N.ground)))
            ctx.stroke(Path(ellipseIn: dot), with: .color(color), lineWidth: 1.8)
        }
        .frame(width: width, height: Self.rowHeight)
        .accessibilityHidden(true)
    }
}
