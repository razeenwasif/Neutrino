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
    var run: @MainActor () async -> Void
}

extension AppModel {
    /// Everything the palette can do right now, in the order it lists them
    /// before anything is typed.
    func paletteItems() -> [PaletteItem] {
        var items: [PaletteItem] = []
        let screens: [(Section, String, String, Bool)] = [
            (.code, "Code", "chevron.left.forwardslash.chevron.right", true),
            (.changes, "Changes", "doc.badge.plus", source == .local),
            (.history, "History", "clock.arrow.circlepath", true),
            (.branches, "Branches", "arrow.triangle.branch", true),
            (.pulls, "Pull requests", "arrow.triangle.pull", serverLink != nil),
            (.runs, "Runs", "play.circle", serverLink != nil),
            (.secrets, "Secrets", "key", serverLink != nil),
        ]
        for (section, title, icon, available) in screens where available {
            items.append(PaletteItem(id: "screen:\(title)", group: "Screens", title: title, detail: repo?.name ?? "", icon: icon) { self.section = section })
        }

        if source == .local, isLive {
            items.append(PaletteItem(id: "act:fetch", group: "Actions", title: "Fetch", detail: "from origin", icon: "arrow.down.circle") { await self.fetch() })
            items.append(PaletteItem(id: "act:push", group: "Actions", title: "Push", detail: repo?.head?.label ?? "", icon: "arrow.up.circle") { await self.push() })
            items.append(PaletteItem(id: "act:stage", group: "Actions", title: "Stage all", icon: "plus.square.on.square") {
                self.section = .changes
                await self.stageAll()
            })
        }
        items.append(PaletteItem(id: "act:reload", group: "Actions", title: "Reload", icon: "arrow.clockwise") { await self.reload() })
        if isServerOnline {
            items.append(PaletteItem(id: "act:pull", group: "Actions", title: "New pull request…", icon: "arrow.triangle.pull") {
                self.section = .pulls
                self.sheetRequest = .newPull
            })
            items.append(PaletteItem(id: "act:dispatch", group: "Actions", title: "Run workflow…", icon: "play.circle") {
                self.section = .runs
                self.sheetRequest = .runWorkflow
            })
        }

        for p in glancePulls {
            items.append(PaletteItem(id: "pull:\(p.number)", group: "Pull requests", title: "#\(p.number) \(p.title)", detail: "\(p.source) → \(p.target)", icon: "arrow.triangle.pull") {
                self.section = .pulls
                await self.select(pull: p.number)
            })
        }
        for r in runList.prefix(15) {
            items.append(PaletteItem(id: "run:\(r.id)", group: "Runs", title: "Run \(r.number) \(r.name)", detail: "\(r.state.label) · \(r.branch ?? r.shortSHA)", icon: "play.circle") {
                await self.openRun(r.id)
            })
        }
        for b in branches {
            items.append(PaletteItem(id: "branch:\(b.id)", group: "Branches", title: b.name, detail: b.subject, icon: "arrow.triangle.branch") {
                self.section = .branches
                self.selectedBranch = b.id
            })
        }

        let server = serverLink?.name ?? "the server"
        for r in repos where r.error == nil {
            items.append(PaletteItem(id: "local:\(r.id)", group: "Repositories", title: r.name, detail: "This Mac · \(r.head?.label ?? "")", icon: "laptopcomputer") {
                await self.select(repo: r.id)
            })
        }
        for r in hostedRepos {
            items.append(PaletteItem(id: "server:\(r.id)", group: "Repositories", title: r.name, detail: "\(server) · \(r.head?.label ?? "")", icon: "server.rack") {
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
    // Letters in order, with gaps.
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
    @FocusState private var focused: Bool

    private var results: [PaletteItem] {
        let all = model.paletteItems()
        if query.trimmingCharacters(in: .whitespaces).isEmpty {
            // Before typing: screens and actions, then the rest, short.
            return Array(all.prefix(40))
        }
        return all.enumerated()
            .compactMap { pair -> (Int, Int, PaletteItem)? in
                let item = pair.element
                // A match in the title counts fully, in the detail a third.
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
            HStack(spacing: 10) {
                Image(systemName: "magnifyingglass").font(.system(size: 14)).foregroundStyle(N.faint)
                TextField("Go to a screen, repository, branch, pull request or run…", text: $query)
                    .textFieldStyle(.plain)
                    .font(.system(size: 15))
                    .focused($focused)
                    .onSubmit { run(items) }
                    .onKeyPress(.downArrow) { move(1, in: items); return .handled }
                    .onKeyPress(.upArrow) { move(-1, in: items); return .handled }
                    .onKeyPress(.escape) { close(); return .handled }
                Text("esc").font(N.mono(11)).foregroundStyle(N.faint)
            }
            .padding(.horizontal, 16)
            .frame(height: 50)
            Divider().overlay(Color.white.opacity(0.06))
            if items.isEmpty {
                Text("Nothing matches \"\(query)\".").font(.system(size: 12.5)).foregroundStyle(N.muted).padding(20)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 1) {
                            ForEach(Array(items.enumerated()), id: \.element.id) { i, item in
                                if i == 0 || items[i - 1].group != item.group {
                                    SectionLabel(item.group).padding(.horizontal, 12).padding(.top, i == 0 ? 8 : 12).padding(.bottom, 4)
                                }
                                row(item, selected: i == index)
                                    .id(item.id)
                                    .onTapGesture {
                                        index = i
                                        run(items)
                                    }
                            }
                        }
                        .padding(6)
                    }
                    // As tall as the results, up to 420: rows of 33, group labels of ~30.
                    .frame(height: min(420, CGFloat(items.count) * 33 + CGFloat(groupCount(items)) * 30 + 14))
                    .onChange(of: index) { _, i in
                        if items.indices.contains(i) { proxy.scrollTo(items[i].id) }
                    }
                }
            }
        }
        .frame(width: 640)
        .background(N.island, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color.white.opacity(0.08)))
        .shadow(color: .black.opacity(0.5), radius: 30, y: 12)
        .onAppear { focused = true }
        .onChange(of: query) { index = 0 }
        .task {
            // So pull requests and runs are there to find.
            await model.refreshGlancePulls()
            await model.loadRunList()
        }
    }

    private func row(_ item: PaletteItem, selected: Bool) -> some View {
        HStack(spacing: 10) {
            Image(systemName: item.icon).font(.system(size: 12)).foregroundStyle(selected ? N.accent : N.faint).frame(width: 18)
            Text(item.title).font(.system(size: 13)).foregroundStyle(N.text).lineLimit(1)
            Text(item.detail).font(.system(size: 12)).foregroundStyle(N.faint).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 0)
            if selected { Image(systemName: "return").font(.system(size: 11)).foregroundStyle(N.faint) }
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(selected ? N.selected : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        .contentShape(Rectangle())
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
