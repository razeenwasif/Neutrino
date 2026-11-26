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
    func placeholder(in context: Context) -> NeutrinoEntry {
        NeutrinoEntry(date: .now, snapshot: .placeholder, isPlaceholder: true)
    }

    func getSnapshot(in context: Context, completion: @escaping (NeutrinoEntry) -> Void) {
        let snap = NeutrinoWidgetStore.load() ?? .placeholder
        completion(NeutrinoEntry(date: .now, snapshot: snap, isPlaceholder: context.isPreview && NeutrinoWidgetStore.load() == nil))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<NeutrinoEntry>) -> Void) {
        let snap = NeutrinoWidgetStore.load() ?? .placeholder
        let entry = NeutrinoEntry(date: .now, snapshot: snap, isPlaceholder: false)
        let next = Calendar.current.date(byAdding: .minute, value: 5, to: .now) ?? Date(timeIntervalSinceNow: 300)
        completion(Timeline(entries: [entry], policy: .after(next)))
    }
}

struct NeutrinoStatusWidget: Widget {
    let kind = "dev.neutrino.status"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: NeutrinoProvider()) { entry in
            NeutrinoWidgetRoot(entry: entry)
        }
        .configurationDisplayName("Neutrino Workspace")
        .description("Repositories, dirty changes, pull requests, and CI status.")
        #if os(iOS)
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge, .accessoryCircular, .accessoryRectangular])
        #else
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge])
        #endif
    }
}

// MARK: - Root & Obsidian Background

private struct NeutrinoWidgetRoot: View {
    @Environment(\.widgetFamily) private var family
    let entry: NeutrinoEntry

    var body: some View {
        let snap = entry.snapshot
        Group {
            switch family {
            case .systemSmall:
                SmallWorkspaceWidget(snap: snap)
            case .systemMedium:
                MediumWorkspaceWidget(snap: snap)
            case .systemLarge:
                LargeWorkspaceWidget(snap: snap)
            #if os(iOS)
            case .accessoryCircular:
                LockCircularWidget(snap: snap)
            case .accessoryRectangular:
                LockRectangularWidget(snap: snap)
            #endif
            default:
                SmallWorkspaceWidget(snap: snap)
            }
        }
        .containerBackground(for: .widget) {
            #if os(iOS)
            if family == .accessoryCircular || family == .accessoryRectangular {
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

// MARK: - Small Widget

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

// MARK: - Medium Widget

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

// MARK: - Large Widget

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

    private func runRow(_ run: WidgetRunItem) -> some View {
        let color: Color = switch run.state {
        case .success: N.mint
        case .failure: N.rose
        case .running, .queued, .waiting: N.amber
        case .cancelled, .skipped: N.slate
        }
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
}

#if os(iOS)
private struct LockCircularWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        VStack(spacing: 1) {
            Image(systemName: "arrow.triangle.branch").font(.system(size: 11, weight: .bold))
            Text("\(snap.openPullCount)")
                .font(.system(size: 14, weight: .bold).monospacedDigit())
        }
        .widgetURL(URL(string: "neutrino://pulls"))
    }
}

private struct LockRectangularWidget: View {
    let snap: NeutrinoWidgetSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                Image(systemName: "arrow.triangle.branch").font(.system(size: 10))
                Text("NEUTRINO · \(snap.activeRepoName?.uppercased() ?? "REPOS")")
                    .font(.system(size: 10, weight: .bold))
                    .lineLimit(1)
            }
            Text("\(snap.activeDirtyFiles) changes · \(snap.openPullCount) open PRs")
                .font(.system(size: 12, weight: .semibold).monospacedDigit())
                .lineLimit(1)
            if let branch = snap.activeBranch {
                Text("on \(branch)")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .widgetURL(URL(string: "neutrino://changes"))
    }
}
#endif
