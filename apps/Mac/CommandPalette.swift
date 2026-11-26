import NeutrinoKit
import SwiftUI

/// ⌘K: type to jump to a screen, a repository (on this Mac or the server), a
/// branch, a pull request or a run, or to run Fetch, Push and the rest.
struct PaletteItem: Identifiable {
    var id: String
    var group: String
    var title: String
    var detail: String = ""
    var icon: String
    var tint: Color = N.indigo
    var run: @MainActor () async -> Void
}

extension AppModel {
    /// Everything the palette can do right now, in the order it lists them
    /// before anything is typed.
    func paletteItems() -> [PaletteItem] {
        var items: [PaletteItem] = []
        let screens: [(Section, Bool)] = [
            (.code, true),
            (.changes, source == .local),
            (.history, true),
            (.branches, true),
            (.pulls, serverLink != nil),
            (.runs, serverLink != nil),
            (.secrets, serverLink != nil),
        ]
        for (section, available) in screens where available {
            items.append(PaletteItem(
                id: "screen:\(section.title)",
                group: "Screens",
                title: section.title,
                detail: repo?.name ?? "",
                icon: section.icon,
                tint: section.accentColor
            ) {
                self.section = section
            })
        }

        if source == .local, isLive {
            items.append(PaletteItem(id: "act:fetch", group: "Actions", title: "Fetch", detail: "from origin", icon: "arrow.down.circle", tint: N.sky) {
                await self.fetch()
            })
            items.append(PaletteItem(id: "act:push", group: "Actions", title: "Push", detail: repo?.head?.label ?? "", icon: "arrow.up.circle", tint: N.indigo) {
                await self.push()
            })
            items.append(PaletteItem(id: "act:stage", group: "Actions", title: "Stage all", icon: "plus.square.on.square", tint: N.mint) {
                self.section = .changes
                await self.stageAll()
            })
        }
        items.append(PaletteItem(id: "act:reload", group: "Actions", title: "Reload", icon: "arrow.clockwise", tint: N.muted) {
            await self.reload()
        })
        if isServerOnline {
            items.append(PaletteItem(id: "act:pull", group: "Actions", title: "New pull request…", icon: "arrow.triangle.pull", tint: N.mint) {
                self.section = .pulls
                self.sheetRequest = .newPull
            })
            items.append(PaletteItem(id: "act:dispatch", group: "Actions", title: "Run workflow…", icon: "play.circle", tint: N.amber) {
                self.section = .runs
                self.sheetRequest = .runWorkflow
            })
        }

        for p in glancePulls {
            items.append(PaletteItem(id: "pull:\(p.number)", group: "Pull requests", title: "#\(p.number) \(p.title)", detail: "\(p.source) → \(p.target)", icon: "arrow.triangle.pull", tint: N.mint) {
                self.section = .pulls
                await self.select(pull: p.number)
            })
        }
        for r in runList.prefix(15) {
            items.append(PaletteItem(id: "run:\(r.id)", group: "Runs", title: "Run \(r.number) \(r.name)", detail: "\(r.state.label) · \(r.branch ?? r.shortSHA)", icon: "play.circle", tint: N.amber) {
                await self.openRun(r.id)
            })
        }
        for b in branches {
            items.append(PaletteItem(id: "branch:\(b.id)", group: "Branches", title: b.name, detail: b.subject, icon: "arrow.triangle.branch", tint: N.violet) {
                self.section = .branches
                self.selectedBranch = b.id
            })
        }

        let server = serverLink?.name ?? "the server"
        for r in repos where r.error == nil {
            items.append(PaletteItem(id: "local:\(r.id)", group: "Repositories", title: r.name, detail: "This Mac · \(r.head?.label ?? "")", icon: "laptopcomputer", tint: N.indigo) {
                await self.select(repo: r.id)
            })
        }
        for r in hostedRepos {
            items.append(PaletteItem(id: "server:\(r.id)", group: "Repositories", title: r.name, detail: "\(server) · \(r.head?.label ?? "")", icon: "server.rack", tint: N.violet) {
                await self.selectHosted(r.id)
            })
        }
        return items
    }
}

/// How well `query` matches `text`: higher is better, nil for no match.
/// A plain substring beats letters found in order; earlier and at a word start beats later.
func paletteScore(_ query: String, _ text: String) -> Int? {
    let q = query.lowercased().trimmingCharacters(in: .whitespaces)
    if q.isEmpty { return 0 }
    let t = text.lowercased()
    if let r = t.range(of: q) {
        let at = t.distance(from: t.startIndex, to: r.lowerBound)
        let wordStart = at == 0 || !t[t.index(before: r.lowerBound)].isLetter
        return 1000 - at * 4 + (wordStart ? 200 : 0) + (t.count == q.count ? 300 : 0)
    }
    var score = 400
    var i = t.startIndex
    for ch in q {
        guard let found = t[i...].firstIndex(of: ch) else { return nil }
        score -= t.distance(from: i, to: found)
        i = t.index(after: found)
    }
    return max(score, 1)
}

struct CommandPalette: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @State private var index = 0
    @State private var hoveredID: String?
    @FocusState private var focused: Bool

    private var results: [PaletteItem] {
        let all = model.paletteItems()
        if query.trimmingCharacters(in: .whitespaces).isEmpty {
            return Array(all.prefix(40))
        }
        return all.enumerated()
            .compactMap { pair -> (Int, Int, PaletteItem)? in
                let item = pair.element
                let s = max(paletteScore(query, item.title) ?? -1, (paletteScore(query, item.detail) ?? -1) / 3)
                guard s > 0 else { return nil }
                return (s, pair.offset, item)
            }
            .sorted { ($0.0, -$0.1) > ($1.0, -$1.1) }
            .prefix(50)
            .map(\.2)
    }

    var body: some View {
        let items = results
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(N.indigo)
                TextField("", text: $query, prompt: Text("Go to a screen, repository, branch, pull request or run…").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(.system(size: 14))
                    .foregroundStyle(N.text)
                    .focused($focused)
                    .onSubmit { run(items) }
                    .onKeyPress(.downArrow) { move(1, in: items); return .handled }
                    .onKeyPress(.upArrow) { move(-1, in: items); return .handled }
                    .onKeyPress(.escape) { close(); return .handled }
                TagPill(text: "ESC", textColor: N.faint, background: N.surfacePill, monospaced: true)
            }
            .padding(.horizontal, 16)
            .frame(height: 50)

            if items.isEmpty {
                Text("Nothing matches \"\(query)\".")
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.muted)
                    .padding(24)
            } else {
                ScrollViewReader { proxy in
                    ScrollView(.vertical, showsIndicators: false) {
                        LazyVStack(alignment: .leading, spacing: 2) {
                            ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                                if i == 0 || items[i - 1].group != item.group {
                                    SectionLabel(item.group)
                                        .padding(.horizontal, 12)
                                        .padding(.top, i == 0 ? 6 : 12)
                                        .padding(.bottom, 4)
                                }
                                row(item, selected: i == index)
                                    .id(item.id)
                                    .onHover { hoveredID = $0 ? item.id : nil }
                                    .onTapGesture {
                                        index = i
                                        run(items)
                                    }
                            }
                        }
                        .padding(.horizontal, 8)
                        .padding(.bottom, 8)
                    }
                    .frame(height: min(400, CGFloat(items.count) * 34 + CGFloat(groupCount(items)) * 28 + 16))
                    .onChange(of: index) { _, i in
                        if items.indices.contains(i) { proxy.scrollTo(items[i].id) }
                    }
                }
            }
        }
        .frame(width: 580)
        .background(N.palette, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .shadow(color: .black.opacity(0.75), radius: 40, y: 16)
        .onAppear { focused = true }
        .onChange(of: query) { index = 0 }
        .task {
            await model.refreshGlancePulls()
            await model.loadRunList()
        }
    }

    private func row(_ item: PaletteItem, selected: Bool) -> some View {
        let hovered = hoveredID == item.id
        return HStack(spacing: 10) {
            Image(systemName: item.icon)
                .font(.system(size: 12, weight: selected ? .semibold : .regular))
                .foregroundStyle(selected ? item.tint : N.muted)
                .frame(width: 18)
            Text(item.title)
                .font(.system(size: 13, weight: selected ? .medium : .regular))
                .foregroundStyle(N.text)
                .lineLimit(1)
            if !item.detail.isEmpty {
                Text(item.detail)
                    .font(.system(size: 12))
                    .foregroundStyle(N.faint)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            Spacer(minLength: 8)
            if selected {
                Image(systemName: "return")
                    .font(.system(size: 10.5))
                    .foregroundStyle(N.muted)
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(
            selected ? N.surfaceActive : (hovered ? N.surfaceHover : Color.clear),
            in: RoundedRectangle(cornerRadius: 8, style: .continuous)
        )
        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func groupCount(_ items: [PaletteItem]) -> Int {
        items.indices.filter { $0 == 0 || items[$0 - 1].group != items[$0].group }.count
    }

    private func move(_ by: Int, in items: [PaletteItem]) {
        guard !items.isEmpty else { return }
        index = (index + by + items.count) % items.count
    }

    private func run(_ items: [PaletteItem]) {
        guard items.indices.contains(index) else { return }
        let item = items[index]
        close()
        Task { await item.run() }
    }

    private func close() {
        model.paletteOpen = false
    }
}
