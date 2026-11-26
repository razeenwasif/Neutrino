import NeutrinoKit
import SwiftUI
import WidgetKit

@main
struct NeutrinoWidgetsBundle: WidgetBundle {
    var body: some Widget {
        NeutrinoStatusWidget()
    }
}

struct NeutrinoEntry: TimelineEntry {
    let date: Date
    let snapshot: NeutrinoWidgetSnapshot
    let isPlaceholder: Bool
}

struct NeutrinoProvider: TimelineProvider {
    private var defaultSnapshot: NeutrinoWidgetSnapshot {
        #if os(iOS)
        .phonePlaceholder
        #else
        .placeholder
        #endif
    }

    func placeholder(in context: Context) -> NeutrinoEntry {
        NeutrinoEntry(date: .now, snapshot: defaultSnapshot, isPlaceholder: true)
    }

    func getSnapshot(in context: Context, completion: @escaping (NeutrinoEntry) -> Void) {
        let loaded = NeutrinoWidgetStore.load()
        let snap = loaded ?? defaultSnapshot
        completion(NeutrinoEntry(date: .now, snapshot: snap, isPlaceholder: context.isPreview && loaded == nil))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<NeutrinoEntry>) -> Void) {
        #if os(iOS)
        ServerStore.shared = ("group.dev.neutrino", "YOUR_TEAM_ID.dev.neutrino.shared")
        if let link = ServerStore.load() {
            let service = ServerService(link: link)
            let serverName = link.name
            let cached = NeutrinoWidgetStore.load()
            let done = WidgetCompletionBox(completion)
            Task {
                let fresh: NeutrinoWidgetSnapshot
                if let inbox = try? await service.inbox() {
                    fresh = NeutrinoWidgetSnapshot.fromInbox(inbox, serverName: serverName, online: true)
                    NeutrinoWidgetStore.save(fresh, reload: false)
                } else if let cached {
                    fresh = NeutrinoWidgetSnapshot(
                        updatedMs: cached.updatedMs,
                        serverName: cached.serverName ?? serverName,
                        serverOnline: false,
                        daemonOnline: false,
                        localRepoCount: cached.localRepoCount,
                        hostedRepoCount: cached.hostedRepoCount,
                        dirtyRepoCount: cached.dirtyRepoCount,
                        openPullCount: cached.openPullCount,
                        activeRepoID: cached.activeRepoID,
                        activeRepoName: cached.activeRepoName,
                        activeBranch: cached.activeBranch,
                        activeDirtyFiles: cached.activeDirtyFiles,
                        waitingApprovalCount: cached.waitingApprovalCount,
                        readyPullCount: cached.readyPullCount,
                        failingRunCount: cached.failingRunCount,
                        isPhoneInbox: true,
                        repos: cached.repos,
                        waitingApprovals: cached.waitingApprovals,
                        openPulls: cached.openPulls,
                        recentRuns: cached.recentRuns
                    )
                } else {
                    fresh = NeutrinoWidgetSnapshot(
                        serverName: serverName,
                        serverOnline: false,
                        daemonOnline: false,
                        localRepoCount: 0,
                        hostedRepoCount: 0,
                        dirtyRepoCount: 0,
                        openPullCount: 0,
                        isPhoneInbox: true
                    )
                }
                let entry = NeutrinoEntry(date: .now, snapshot: fresh, isPlaceholder: false)
                let next = Calendar.current.date(byAdding: .minute, value: 15, to: .now) ?? Date(timeIntervalSinceNow: 900)
                done.finish(Timeline(entries: [entry], policy: .after(next)))
            }
            return
        }
        let fallback = NeutrinoWidgetStore.load() ?? defaultSnapshot
        let entry = NeutrinoEntry(date: .now, snapshot: fallback, isPlaceholder: false)
        let next = Calendar.current.date(byAdding: .minute, value: 15, to: .now) ?? Date(timeIntervalSinceNow: 900)
        completion(Timeline(entries: [entry], policy: .after(next)))
        #else
        let snap = NeutrinoWidgetStore.load() ?? defaultSnapshot
        let entry = NeutrinoEntry(date: .now, snapshot: snap, isPlaceholder: false)
        let next = Calendar.current.date(byAdding: .minute, value: 5, to: .now) ?? Date(timeIntervalSinceNow: 300)
        completion(Timeline(entries: [entry], policy: .after(next)))
        #endif
    }
}

private struct WidgetCompletionBox: @unchecked Sendable {
    let finish: (Timeline<NeutrinoEntry>) -> Void
    init(_ finish: @escaping (Timeline<NeutrinoEntry>) -> Void) { self.finish = finish }
}

private func neutrinoURL(_ host: String, _ parts: String...) -> URL {
    let segAllowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
    let encoded = parts.map { $0.addingPercentEncoding(withAllowedCharacters: segAllowed) ?? $0 }
    let path = encoded.isEmpty ? "" : "/" + encoded.joined(separator: "/")
    return URL(string: "neutrino://\(host)\(path)") ?? URL(string: "neutrino://inbox")!
}

struct NeutrinoStatusWidget: Widget {
    let kind = "dev.neutrino.status"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: NeutrinoProvider()) { entry in
            NeutrinoWidgetRoot(entry: entry)
        }
        #if os(iOS)
        .configurationDisplayName("Neutrino Inbox")
        .description("Deploy approvals, open pull requests, and CI runs from your paired Mac.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge, .accessoryCircular, .accessoryRectangular, .accessoryInline])
        #else
        .configurationDisplayName("Neutrino Workspace")
        .description("Repositories, dirty changes, pull requests, and CI status.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
        #endif
    }
}

// MARK: - Root & Obsidian Background

private struct NeutrinoWidgetRoot: View {
    @Environment(\.widgetFamily) private var family
    let entry: NeutrinoEntry

    private var usePhoneInbox: Bool {
        #if os(iOS)
        true
        #else
        entry.snapshot.isPhoneInbox
        #endif
    }

    var body: some View {
        let snap = entry.snapshot
        Group {
            switch family {
            case .systemSmall:
                if usePhoneInbox { PhoneSmallInboxWidget(snap: snap) } else { SmallWorkspaceWidget(snap: snap) }
            case .systemMedium:
                if usePhoneInbox { PhoneMediumInboxWidget(snap: snap) } else { MediumWorkspaceWidget(snap: snap) }
            case .systemLarge:
                if usePhoneInbox { PhoneLargeInboxWidget(snap: snap) } else { LargeWorkspaceWidget(snap: snap) }
            #if os(iOS)
            case .accessoryCircular:
                LockCircularWidget(snap: snap)
            case .accessoryRectangular:
                LockRectangularWidget(snap: snap)
            case .accessoryInline:
                LockInlineWidget(snap: snap)
            #endif
            default:
                if usePhoneInbox { PhoneSmallInboxWidget(snap: snap) } else { SmallWorkspaceWidget(snap: snap) }
            }
        }
        .containerBackground(for: .widget) {
            #if os(iOS)
            if family == .accessoryCircular || family == .accessoryRectangular || family == .accessoryInline {
                Color.clear
            } else {
                obsidianBackground
            }
            #else
            obsidianBackground
            #endif
        }
    }

    private var obsidianBackground: some View {
        ZStack {
            LinearGradient(
                colors: [Color(hex: 0x06070C), Color(hex: 0x000000)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            RadialGradient(
                colors: [N.indigo.opacity(0.16), .clear],
                center: .topLeading,
                startRadius: 2,
                endRadius: 190
            )
            RadialGradient(
                colors: [N.violet.opacity(0.10), .clear],
                center: .bottomTrailing,
                startRadius: 4,
                endRadius: 170
            )
        }
    }
}

private struct WidgetNeutrinoMark: View {
    var size: CGFloat = 22

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
                .fill(Color(hex: 0x090B14))
            RadialGradient(
                colors: [N.indigo.opacity(0.45), .clear],
                center: .center,
                startRadius: 0,
                endRadius: size * 0.52
            )
            Circle()
                .stroke(N.indigo.opacity(0.55), lineWidth: max(1.1, size * 0.06))
                .padding(size * 0.19)
            Circle()
                .fill(.white)
                .frame(width: size * 0.26, height: size * 0.26)
                .shadow(color: N.sky.opacity(0.85), radius: 3)
        }
        .frame(width: size, height: size)
    }
}

// MARK: - iOS Phone Inbox Widgets

private struct PhoneSmallInboxWidget: View {
    let snap: NeutrinoWidgetSnapshot

    private var attentionCount: Int {
        snap.waitingApprovalCount > 0 ? snap.waitingApprovalCount : snap.openPullCount
    }

    private var heroColor: Color {
        if snap.waitingApprovalCount > 0 { return N.amber }
        if snap.failingRunCount > 0 { return N.rose }
        return snap.openPullCount > 0 ? N.indigo : N.mint
    }

    private var statusColor: Color {
        snap.serverOnline ? N.mint : (snap.serverName == nil ? N.amber : N.rose)
    }

    private var statusLabel: String {
        if snap.serverOnline { return snap.serverName?.uppercased() ?? "LIVE" }
        return snap.serverName == nil ? "DEMO" : "OFFLINE"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                WidgetNeutrinoMark(size: 24)
                Spacer()
                HStack(spacing: 4) {
                    Circle().fill(statusColor).frame(width: 5.5, height: 5.5)
                    Text(statusLabel)
                        .font(N.mono(9, weight: .bold))
                        .foregroundStyle(statusColor)
                        .lineLimit(1)
                }
                .padding(.horizontal, 7)
                .frame(height: 18)
                .background(statusColor.opacity(0.14), in: Capsule())
            }

            Spacer(minLength: 6)

            if snap.waitingApprovalCount == 0 && snap.openPullCount == 0 && snap.failingRunCount == 0 {
                Text("All clear")
                    .font(.system(size: 20, weight: .bold))
                    .foregroundStyle(N.mint)
                Text("No approvals or PRs")
                    .font(.system(size: 11))
                    .foregroundStyle(N.muted)
                    .lineLimit(1)
            } else {
                Text("\(attentionCount)")
                    .font(N.heroKPI())
                    .foregroundStyle(heroColor)
                Text(snap.waitingApprovalCount > 0
                     ? (snap.waitingApprovalCount == 1 ? "deploy approval" : "deploy approvals")
                     : (snap.openPullCount == 1 ? "open pull request" : "open pull requests"))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(N.muted)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("\(snap.openPullCount)")
                        .font(N.mono(15, weight: .bold))
                        .foregroundStyle(snap.openPullCount > 0 ? N.sky : N.text)
                    Text("open PRs")
                        .font(.system(size: 9.5))
                        .foregroundStyle(N.subtle)
                }
                Spacer()
                VStack(alignment: .trailing, spacing: 1) {
                    let count = snap.failingRunCount > 0 ? snap.failingRunCount : snap.recentRuns.count
                    Text("\(count)")
                        .font(N.mono(15, weight: .bold))
                        .foregroundStyle(snap.failingRunCount > 0 ? N.rose : N.mint)
                    Text(snap.failingRunCount > 0 ? "failing" : "CI runs")
                        .font(.system(size: 9.5))
                        .foregroundStyle(N.subtle)
                }
            }
        }
        .widgetURL(neutrinoURL("inbox"))
    }
}

private struct PhoneMediumInboxWidget: View {
    let snap: NeutrinoWidgetSnapshot

    private enum InboxFeedItem: Identifiable {
        case waiting(WidgetWaitingItem)
        case pull(WidgetPullItem)
        case run(WidgetRunItem)

        var id: String {
            switch self {
            case .waiting(let w): "w-\(w.id)"
            case .pull(let p): "p-\(p.id)"
            case .run(let r): "r-\(r.id)"
            }
        }

        var url: URL {
            switch self {
            case .waiting(let w): neutrinoURL("run", w.repo, "\(w.runID)")
            case .pull(let p): neutrinoURL("pull", p.repoName, "\(p.number)")
            case .run(let r):
                if let repo = r.repoName { neutrinoURL("run", repo, "\(r.id)") } else { neutrinoURL("inbox") }
            }
        }
    }

    private var feed: [InboxFeedItem] {
        var items: [InboxFeedItem] = []
        items.append(contentsOf: snap.waitingApprovals.map(InboxFeedItem.waiting))
        items.append(contentsOf: snap.openPulls.map(InboxFeedItem.pull))
        items.append(contentsOf: snap.recentRuns.map(InboxFeedItem.run))
        return Array(items.prefix(3))
    }

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    WidgetNeutrinoMark(size: 20)
                    VStack(alignment: .leading, spacing: 0) {
                        Text("Neutrino")
                            .font(.system(size: 11.5, weight: .semibold))
                            .foregroundStyle(N.text)
                        HStack(spacing: 4) {
                            Circle()
                                .fill(snap.serverOnline ? N.mint : N.amber)
                                .frame(width: 5, height: 5)
                            Text(snap.serverName ?? "Inbox")
                                .font(.system(size: 9.5))
                                .foregroundStyle(N.muted)
                                .lineLimit(1)
                        }
                    }
                }

                Spacer(minLength: 4)

                if snap.waitingApprovalCount > 0 {
                    Text("\(snap.waitingApprovalCount)")
                        .font(N.mono(26, weight: .bold))
                        .foregroundStyle(N.amber)
                    Text(snap.waitingApprovalCount == 1 ? "approval" : "approvals")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(N.amber.opacity(0.9))
                } else {
                    Text("\(snap.openPullCount)")
                        .font(N.mono(26, weight: .bold))
                        .foregroundStyle(snap.openPullCount > 0 ? N.indigo : N.mint)
                    Text(snap.openPullCount == 1 ? "open PR" : "open PRs")
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(N.muted)
                }

                Spacer(minLength: 4)

                HStack(spacing: 10) {
                    statChip("PRs", "\(snap.openPullCount)", N.sky)
                    statChip(snap.failingRunCount > 0 ? "FAIL" : "RUNS",
                             "\(snap.failingRunCount > 0 ? snap.failingRunCount : snap.recentRuns.count)",
                             snap.failingRunCount > 0 ? N.rose : N.mint)
                }
            }
            .frame(width: 112, alignment: .leading)

            VStack(alignment: .leading, spacing: 5) {
                if feed.isEmpty {
                    VStack(spacing: 5) {
                        Image(systemName: "checkmark.circle")
                            .font(.system(size: 16))
                            .foregroundStyle(N.mint)
                        Text("Nothing needs you")
                            .font(.system(size: 11.5, weight: .medium))
                            .foregroundStyle(N.muted)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color.white.opacity(0.03), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                } else {
                    ForEach(feed) { item in
                        Link(destination: item.url) {
                            feedRow(item)
                        }
                    }
                    Spacer(minLength: 0)
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }

    private func statChip(_ label: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label)
                .font(.system(size: 8.5, weight: .semibold))
                .tracking(0.5)
                .foregroundStyle(N.faint)
            Text(value)
                .font(N.mono(12, weight: .bold))
                .foregroundStyle(color)
                .lineLimit(1)
        }
    }

    @ViewBuilder
    private func feedRow(_ item: InboxFeedItem) -> some View {
        switch item {
        case .waiting(let w):
            HStack(spacing: 7) {
                Image(systemName: "hand.raised.fill")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(N.amber)
                VStack(alignment: .leading, spacing: 1) {
                    Text(w.environment.map { "Deploy \($0)" } ?? w.jobName)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                    Text("\(w.repo) · \(w.runName)")
                        .font(N.mono(9))
                        .foregroundStyle(N.muted)
                        .lineLimit(1)
                }
                Spacer(minLength: 2)
                Text("APPROVE")
                    .font(N.mono(8.5, weight: .bold))
                    .foregroundStyle(N.amber)
            }
            .padding(.horizontal, 8)
            .frame(height: 36)
            .background(N.amber.opacity(0.09), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        case .pull(let p):
            HStack(spacing: 7) {
                Circle().fill(N.sky).frame(width: 5.5, height: 5.5)
                Text("#\(p.number)")
                    .font(N.mono(10, weight: .semibold))
                    .foregroundStyle(N.sky)
                Text(p.title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(N.text)
                    .lineLimit(1)
                Spacer(minLength: 2)
                Text(p.repoName)
                    .font(N.mono(9))
                    .foregroundStyle(N.subtle)
                    .lineLimit(1)
            }
            .padding(.horizontal, 8)
            .frame(height: 34)
            .background(Color.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        case .run(let r):
            let color = runColor(r.state)
            HStack(spacing: 7) {
                Circle().fill(color).frame(width: 5.5, height: 5.5)
                Text(r.workflow)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(N.text)
                    .lineLimit(1)
                Spacer(minLength: 2)
                Text(r.state.label)
                    .font(N.mono(9.5, weight: .semibold))
                    .foregroundStyle(color)
            }
            .padding(.horizontal, 8)
            .frame(height: 34)
            .background(Color.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
    }
}

private struct PhoneLargeInboxWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 9) {
                WidgetNeutrinoMark(size: 24)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Neutrino Inbox")
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundStyle(N.text)
                    Text(snap.serverName.map { "Paired with \($0)" } ?? "Not paired")
                        .font(.system(size: 11))
                        .foregroundStyle(N.muted)
                }
                Spacer()
                let statusColor = snap.serverOnline ? N.mint : N.amber
                HStack(spacing: 5) {
                    Circle().fill(statusColor).frame(width: 6, height: 6)
                    Text(snap.serverOnline ? "ONLINE" : (snap.serverName == nil ? "DEMO" : "OFFLINE"))
                        .font(N.mono(9.5, weight: .bold))
                        .foregroundStyle(statusColor)
                }
                .padding(.horizontal, 8)
                .frame(height: 20)
                .background(statusColor.opacity(0.13), in: Capsule())
            }

            HStack(spacing: 8) {
                phoneKPITile("APPROVALS", "\(snap.waitingApprovalCount)", snap.waitingApprovalCount > 0 ? N.amber : N.mint)
                phoneKPITile("OPEN PRS", "\(snap.openPullCount)", N.sky)
                phoneKPITile("CI RUNS", "\(snap.recentRuns.count)", snap.failingRunCount > 0 ? N.rose : N.indigo)
            }

            if !snap.waitingApprovals.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Text("WAITING FOR APPROVAL")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(0.6)
                        .foregroundStyle(N.amber)
                    ForEach(snap.waitingApprovals.prefix(2)) { w in
                        Link(destination: neutrinoURL("run", w.repo, "\(w.runID)")) {
                            HStack(spacing: 8) {
                                Image(systemName: "hand.raised.fill")
                                    .font(.system(size: 11))
                                    .foregroundStyle(N.amber)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text("Deploy \(w.environment ?? w.jobName)")
                                        .font(.system(size: 12, weight: .semibold))
                                        .foregroundStyle(N.text)
                                        .lineLimit(1)
                                    Text("\(w.repo) · \(w.runName) #\(w.number)")
                                        .font(N.mono(9.5))
                                        .foregroundStyle(N.muted)
                                        .lineLimit(1)
                                }
                                Spacer()
                                Text("Review")
                                    .font(N.mono(9.5, weight: .bold))
                                    .foregroundStyle(N.onAccent)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 3)
                                    .background(N.amber, in: Capsule())
                            }
                            .padding(.horizontal, 10)
                            .frame(height: 36)
                            .background(N.amber.opacity(0.09), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                }
            }

            if !snap.openPulls.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Text("PULL REQUESTS")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(0.6)
                        .foregroundStyle(N.faint)
                    ForEach(snap.openPulls.prefix(snap.waitingApprovals.isEmpty ? 3 : 2)) { pr in
                        Link(destination: neutrinoURL("pull", pr.repoName, "\(pr.number)")) {
                            pullRow(pr)
                        }
                    }
                }
            }

            if !snap.recentRuns.isEmpty {
                let runSlots = max(1, 4 - snap.waitingApprovals.prefix(2).count - snap.openPulls.prefix(2).count)
                VStack(alignment: .leading, spacing: 5) {
                    Text("RECENT CI RUNS")
                        .font(.system(size: 9, weight: .semibold))
                        .tracking(0.6)
                        .foregroundStyle(N.faint)
                    ForEach(snap.recentRuns.prefix(runSlots)) { run in
                        let dest = run.repoName.map { neutrinoURL("run", $0, "\(run.id)") } ?? neutrinoURL("inbox")
                        Link(destination: dest) {
                            runRow(run)
                        }
                    }
                }
            }

            if snap.waitingApprovals.isEmpty && snap.openPulls.isEmpty && snap.recentRuns.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "checkmark.circle")
                        .font(.system(size: 20))
                        .foregroundStyle(N.mint)
                    Text("Nothing needs your review")
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundStyle(N.muted)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.white.opacity(0.03), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }

            Spacer(minLength: 0)
        }
    }

    private func phoneKPITile(_ title: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.system(size: 9, weight: .semibold))
                .tracking(0.55)
                .foregroundStyle(N.faint)
            Text(value)
                .font(N.mono(15, weight: .bold))
                .foregroundStyle(color)
                .lineLimit(1)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
    }
}

// MARK: - macOS Small / Medium / Large Workspace Widgets

private struct SmallWorkspaceWidget: View {
    let snap: NeutrinoWidgetSnapshot

    private var statusColor: Color {
        if snap.serverOnline { return N.mint }
        return snap.daemonOnline ? N.sky : N.amber
    }

    private var statusLabel: String {
        if snap.serverOnline { return snap.serverName?.uppercased() ?? "SERVER" }
        return snap.daemonOnline ? "LOCAL" : "DEMO"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                WidgetNeutrinoMark(size: 24)
                Spacer()
                HStack(spacing: 4) {
                    Circle().fill(statusColor).frame(width: 5.5, height: 5.5)
                    Text(statusLabel)
                        .font(N.mono(9, weight: .bold))
                        .foregroundStyle(statusColor)
                        .lineLimit(1)
                }
                .padding(.horizontal, 7)
                .frame(height: 18)
                .background(statusColor.opacity(0.14), in: Capsule())
            }

            Spacer(minLength: 6)

            Text(snap.activeRepoName ?? "Neutrino")
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundStyle(N.text)
                .lineLimit(1)

            if let branch = snap.activeBranch {
                HStack(spacing: 4) {
                    Image(systemName: "arrow.triangle.branch")
                        .font(.system(size: 9.5, weight: .semibold))
                        .foregroundStyle(N.violet)
                    Text(branch)
                        .font(N.mono(10.5, weight: .medium))
                        .foregroundStyle(N.muted)
                        .lineLimit(1)
                }
                .padding(.top, 2)
            }

            Spacer(minLength: 8)

            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("\(snap.activeDirtyFiles)")
                        .font(N.mono(18, weight: .bold))
                        .foregroundStyle(snap.activeDirtyFiles > 0 ? N.coral : N.mint)
                    Text(snap.activeDirtyFiles == 1 ? "change" : "changes")
                        .font(.system(size: 9.5))
                        .foregroundStyle(N.subtle)
                }
                Spacer()
                VStack(alignment: .trailing, spacing: 1) {
                    Text("\(snap.openPullCount)")
                        .font(N.mono(18, weight: .bold))
                        .foregroundStyle(snap.openPullCount > 0 ? N.mint : N.text)
                    Text(snap.openPullCount == 1 ? "open PR" : "open PRs")
                        .font(.system(size: 9.5))
                        .foregroundStyle(N.subtle)
                }
            }
        }
        .widgetURL(URL(string: "neutrino://changes"))
    }
}

private struct MediumWorkspaceWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        HStack(spacing: 14) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 7) {
                    WidgetNeutrinoMark(size: 22)
                    VStack(alignment: .leading, spacing: 0) {
                        Text("Neutrino")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(N.text)
                        HStack(spacing: 4) {
                            Circle()
                                .fill(snap.serverOnline ? N.mint : N.indigo)
                                .frame(width: 5, height: 5)
                            Text(snap.serverOnline ? (snap.serverName ?? "Server") : "Local Git")
                                .font(.system(size: 10))
                                .foregroundStyle(N.muted)
                                .lineLimit(1)
                        }
                    }
                }

                Spacer(minLength: 6)

                HStack(alignment: .firstTextBaseline, spacing: 3) {
                    Text("\(snap.localRepoCount)")
                        .font(N.mono(26, weight: .bold))
                        .foregroundStyle(N.text)
                    Text("repos")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(N.muted)
                }

                Spacer(minLength: 6)

                HStack(spacing: 10) {
                    statChip("DIRTY", "\(snap.dirtyRepoCount)", snap.dirtyRepoCount > 0 ? N.coral : N.mint)
                    statChip("PRs", "\(snap.openPullCount)", N.indigo)
                }
            }
            .frame(width: 128, alignment: .leading)

            VStack(alignment: .leading, spacing: 5) {
                if snap.repos.isEmpty {
                    VStack(spacing: 6) {
                        Image(systemName: "folder")
                            .font(.system(size: 16))
                            .foregroundStyle(N.faint)
                        Text("No repositories")
                            .font(.system(size: 11.5))
                            .foregroundStyle(N.muted)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color.white.opacity(0.03), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                } else {
                    ForEach(snap.repos.prefix(4)) { r in
                        Link(destination: URL(string: "neutrino://repo/\(r.id)") ?? URL(string: "neutrino://code")!) {
                            repoRow(r)
                        }
                    }
                    Spacer(minLength: 0)
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }

    private func statChip(_ label: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label)
                .font(.system(size: 8.5, weight: .semibold))
                .tracking(0.5)
                .foregroundStyle(N.faint)
            Text(value)
                .font(N.mono(12, weight: .bold))
                .foregroundStyle(color)
                .lineLimit(1)
        }
    }

    private func repoRow(_ r: WidgetRepoItem) -> some View {
        HStack(spacing: 7) {
            Circle()
                .fill(r.dirtyCount > 0 ? N.coral : N.mint)
                .frame(width: 5.5, height: 5.5)
            Text(r.name)
                .font(.system(size: 11.5, weight: .semibold))
                .foregroundStyle(N.text)
                .lineLimit(1)
            Text(r.branch)
                .font(N.mono(9.5))
                .foregroundStyle(N.muted)
                .lineLimit(1)
            Spacer(minLength: 4)
            if r.ahead > 0 {
                Text("↑\(r.ahead)")
                    .font(N.mono(9.5, weight: .semibold))
                    .foregroundStyle(N.sky)
            }
            if r.dirtyCount > 0 {
                Text("\(r.dirtyCount)")
                    .font(N.mono(10, weight: .bold))
                    .foregroundStyle(N.coral)
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(N.coral.opacity(0.14), in: Capsule())
            } else {
                Text("clean")
                    .font(.system(size: 9.5))
                    .foregroundStyle(N.subtle)
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 28)
        .background(Color.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }
}

private struct LargeWorkspaceWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            HStack(spacing: 9) {
                WidgetNeutrinoMark(size: 24)
                VStack(alignment: .leading, spacing: 1) {
                    Text(snap.activeRepoName ?? "Neutrino")
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundStyle(N.text)
                    Text(snap.activeBranch.map { "Branch \($0)" } ?? "Local & Forge Repositories")
                        .font(.system(size: 11))
                        .foregroundStyle(N.muted)
                }
                Spacer()
                HStack(spacing: 5) {
                    Circle()
                        .fill(snap.serverOnline ? N.mint : N.indigo)
                        .frame(width: 6, height: 6)
                    Text(snap.serverOnline ? (snap.serverName?.uppercased() ?? "ONLINE") : "LOCAL")
                        .font(N.mono(9.5, weight: .bold))
                        .foregroundStyle(snap.serverOnline ? N.mint : N.indigo)
                }
                .padding(.horizontal, 8)
                .frame(height: 20)
                .background((snap.serverOnline ? N.mint : N.indigo).opacity(0.13), in: Capsule())
            }

            HStack(spacing: 10) {
                kpiTile("REPOS", "\(snap.localRepoCount)", N.indigo)
                kpiTile("DIRTY", "\(snap.dirtyRepoCount)", snap.dirtyRepoCount > 0 ? N.coral : N.mint)
                kpiTile("OPEN PRS", "\(snap.openPullCount)", N.sky)
            }

            VStack(alignment: .leading, spacing: 5) {
                Text("REPOSITORIES")
                    .font(.system(size: 9.5, weight: .semibold))
                    .tracking(0.6)
                    .foregroundStyle(N.faint)

                ForEach(snap.repos.prefix(3)) { r in
                    Link(destination: URL(string: "neutrino://repo/\(r.id)") ?? URL(string: "neutrino://code")!) {
                        largeRepoRow(r)
                    }
                }
            }

            if !snap.openPulls.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Text("OPEN PULL REQUESTS")
                        .font(.system(size: 9.5, weight: .semibold))
                        .tracking(0.6)
                        .foregroundStyle(N.faint)

                    ForEach(snap.openPulls.prefix(2)) { pr in
                        Link(destination: URL(string: "neutrino://pulls")!) {
                            pullRow(pr)
                        }
                    }
                }
            } else if !snap.recentRuns.isEmpty {
                VStack(alignment: .leading, spacing: 5) {
                    Text("RECENT CI RUNS")
                        .font(.system(size: 9.5, weight: .semibold))
                        .tracking(0.6)
                        .foregroundStyle(N.faint)

                    ForEach(snap.recentRuns.prefix(2)) { run in
                        Link(destination: URL(string: "neutrino://runs")!) {
                            runRow(run)
                        }
                    }
                }
            }

            Spacer(minLength: 0)
        }
    }

    private func kpiTile(_ title: String, _ value: String, _ color: Color) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title)
                .font(.system(size: 9, weight: .semibold))
                .tracking(0.55)
                .foregroundStyle(N.faint)
            Text(value)
                .font(N.mono(15, weight: .bold))
                .foregroundStyle(color)
                .lineLimit(1)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white.opacity(0.045), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
    }

    private func largeRepoRow(_ r: WidgetRepoItem) -> some View {
        HStack(spacing: 8) {
            Circle()
                .fill(r.dirtyCount > 0 ? N.coral : N.mint)
                .frame(width: 6, height: 6)
            Text(r.name)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(N.text)
                .lineLimit(1)
            Spacer(minLength: 6)
            Text(r.branch)
                .font(N.mono(10))
                .foregroundStyle(N.violet)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(N.violet.opacity(0.12), in: Capsule())
            if r.ahead > 0 {
                Text("↑\(r.ahead)")
                    .font(N.mono(10, weight: .semibold))
                    .foregroundStyle(N.sky)
            }
            Text(r.dirtyCount > 0 ? "\(r.dirtyCount) modified" : "clean")
                .font(N.mono(10.5))
                .foregroundStyle(r.dirtyCount > 0 ? N.coral : N.subtle)
                .frame(width: 72, alignment: .trailing)
        }
        .padding(.horizontal, 10)
        .frame(height: 32)
        .background(Color.white.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

// MARK: - Shared Row Helpers

private func pullRow(_ pr: WidgetPullItem) -> some View {
    HStack(spacing: 8) {
        Image(systemName: "arrow.triangle.pull")
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundStyle(N.mint)
        Text("#\(pr.number)")
            .font(N.mono(10.5, weight: .semibold))
            .foregroundStyle(N.sky)
        Text(pr.title)
            .font(.system(size: 11.5, weight: .medium))
            .foregroundStyle(N.text)
            .lineLimit(1)
        Spacer(minLength: 4)
        Text("\(pr.source) → \(pr.target)")
            .font(N.mono(9.5))
            .foregroundStyle(N.subtle)
            .lineLimit(1)
    }
    .padding(.horizontal, 10)
    .frame(height: 30)
    .background(Color.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
}

private func runColor(_ state: RunState) -> Color {
    switch state {
    case .success: N.mint
    case .failure: N.rose
    case .running, .queued, .waiting: N.amber
    case .cancelled, .skipped: N.slate
    }
}

private func runRow(_ run: WidgetRunItem) -> some View {
    let color = runColor(run.state)
    return HStack(spacing: 8) {
        Circle().fill(color).frame(width: 6, height: 6)
        Text(run.workflow)
            .font(.system(size: 11.5, weight: .semibold))
            .foregroundStyle(N.text)
            .lineLimit(1)
        Text("#\(run.number)")
            .font(N.mono(10))
            .foregroundStyle(N.muted)
        Spacer()
        Text(run.state.label)
            .font(.system(size: 10.5, weight: .medium))
            .foregroundStyle(color)
    }
    .padding(.horizontal, 10)
    .frame(height: 30)
    .background(Color.white.opacity(0.035), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
}

#if os(iOS)
private struct LockCircularWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        let count = snap.waitingApprovalCount > 0 ? snap.waitingApprovalCount : snap.openPullCount
        VStack(spacing: 1) {
            Image(systemName: snap.waitingApprovalCount > 0 ? "hand.raised.fill" : "arrow.triangle.branch")
                .font(.system(size: 11, weight: .bold))
            Text("\(count)")
                .font(.system(size: 14, weight: .bold).monospacedDigit())
        }
        .widgetURL(neutrinoURL("inbox"))
    }
}

private struct LockRectangularWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Image(systemName: snap.waitingApprovalCount > 0 ? "hand.raised.fill" : "arrow.triangle.branch")
                    .font(.system(size: 10))
                Text("NEUTRINO · \(snap.serverName?.uppercased() ?? "INBOX")")
                    .font(.system(size: 10, weight: .bold))
                    .lineLimit(1)
            }
            if snap.waitingApprovalCount > 0 {
                Text("\(snap.waitingApprovalCount) approval\(snap.waitingApprovalCount == 1 ? "" : "s") · \(snap.openPullCount) PRs")
                    .font(.system(size: 12, weight: .semibold).monospacedDigit())
                    .lineLimit(1)
                if let w = snap.waitingApprovals.first {
                    Text("\(w.repo) · \(w.environment ?? w.jobName)")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            } else {
                Text("\(snap.openPullCount) open PRs · \(snap.recentRuns.count) runs")
                    .font(.system(size: 12, weight: .semibold).monospacedDigit())
                    .lineLimit(1)
                if let pr = snap.openPulls.first {
                    Text("#\(pr.number) \(pr.title)")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
            }
        }
        .widgetURL(neutrinoURL("inbox"))
    }
}

private struct LockInlineWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        let label: String = {
            if snap.waitingApprovalCount > 0 {
                return "\(snap.waitingApprovalCount) deploy · \(snap.openPullCount) PRs"
            }
            if snap.openPullCount > 0 {
                return "Neutrino · \(snap.openPullCount) PR\(snap.openPullCount == 1 ? "" : "s")"
            }
            return "Neutrino · All clear"
        }()
        Label(label, systemImage: snap.waitingApprovalCount > 0 ? "hand.raised.fill" : "arrow.triangle.branch")
            .widgetURL(neutrinoURL("inbox"))
    }
}
#endif
