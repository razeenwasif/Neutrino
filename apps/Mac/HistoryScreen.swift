import NeutrinoKit
import SwiftUI

struct HistoryScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            if !model.wideDiff {
                CommitList()
                    .frame(minWidth: 380, maxWidth: .infinity, maxHeight: .infinity)
            }
            CommitPanel()
                .frame(minWidth: 440, idealWidth: 560, maxWidth: model.wideDiff ? .infinity : 620, maxHeight: .infinity)
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
            HStack(spacing: 10) {
                Text("History").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
                Text(model.historyTitle).font(N.mono(12)).foregroundStyle(N.muted).lineLimit(1)
                if model.rev != nil, model.scope == .current {
                    Button("Back to HEAD") {
                        model.rev = nil
                        Task { await model.reloadHistory() }
                    }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 24))
                }
                Spacer(minLength: 8)
                Segmented(options: [(HistoryScope.current, "Branch"), (.all, "All")], selection: $model.scope)
                    .help("This branch, or every branch and tag")
                SearchField(text: $model.query, focused: $searchFocused)
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 10)

            if let error = model.historyError {
                EmptyState(title: "Can't read the history", message: error)
            } else if model.commits.isEmpty, !model.loadingHistory {
                EmptyState(title: model.query.isEmpty ? "No commits yet" : "Nothing matches", message: model.query.isEmpty ? "This branch has no commits." : "No commit's message, author or hash contains \"\(model.query)\".")
            } else {
                list
            }
        }
        .island()
        .onChange(of: model.scope) { Task { await model.reloadHistory() } }
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
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(model.commits) { c in
                        CommitRow(commit: c, lanes: lanes, selected: c.sha == model.selectedSHA, showAuthor: geo.size.width > 760)
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
                        ProgressView().controlSize(.small).padding(12)
                    }
                }
                .padding(.horizontal, 6)
                .padding(.bottom, 8)
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

struct SearchField: View {
    @Binding var text: String
    var focused: FocusState<Bool>.Binding

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(N.faint)
            TextField("Message, author or hash", text: $text)
                .textFieldStyle(.plain)
                .font(.system(size: 12.5))
                .focused(focused)
                .frame(width: 180)
                .onKeyPress(.escape) {
                    text = ""
                    return .handled
                }
            if !text.isEmpty {
                Button {
                    text = ""
                } label: {
                    Image(systemName: "xmark.circle.fill").font(.system(size: 11)).foregroundStyle(N.faint)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .well(radius: 14)
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
                GraphCell(row: row, lanes: lanes, isMerge: commit.isMerge, isHead: commit.refs.contains(where: \.isHead))
            }
            // The subject gives way first; refs, hash and time keep their size.
            Text(commit.subject)
                .font(.system(size: 13))
                .foregroundStyle(N.text)
                .lineLimit(1)
            ForEach(commit.refs.prefix(3), id: \.name) { RefChip(label: $0).fixedSize() }
            if commit.refs.count > 3 {
                Text("+\(commit.refs.count - 3)").font(N.mono(11)).foregroundStyle(N.faint).fixedSize()
                    .help(commit.refs.dropFirst(3).map(\.name).joined(separator: ", "))
            }
            Spacer(minLength: 8)
            if showAuthor {
                Text(commit.author.name).font(.system(size: 12)).foregroundStyle(N.muted).lineLimit(1).frame(width: 120, alignment: .trailing)
            }
            Text(commit.shortSHA).font(N.mono(12)).foregroundStyle(N.muted).fixedSize()
            Text(Format.ago(commit.time)).font(.system(size: 12)).foregroundStyle(N.faint).lineLimit(1).frame(width: 78, alignment: .trailing)
                .help(Format.full(commit.time))
        }
        .padding(.leading, 6)
        .padding(.trailing, 10)
        .frame(height: GraphCell.rowHeight)
        .background(selected ? N.selected : (hovering ? Color.white.opacity(0.035) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .contentShape(Rectangle())
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

    var body: some View {
        let width = CGFloat(max(lanes, 1)) * Self.laneWidth + 6
        Canvas { ctx, size in
            let h = size.height
            let x = { (lane: UInt16) in CGFloat(lane) * Self.laneWidth + Self.laneWidth / 2 + 3 }
            func stroke(_ s: GraphSegment, top: CGFloat, bottom: CGFloat) {
                // Lanes past the visible width are left out.
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
            ctx.fill(Path(ellipseIn: dot), with: .color(isMerge ? color : N.island))
            ctx.stroke(Path(ellipseIn: dot), with: .color(color), lineWidth: 1.8)
        }
        .frame(width: width, height: Self.rowHeight)
        .accessibilityHidden(true)
    }
}
