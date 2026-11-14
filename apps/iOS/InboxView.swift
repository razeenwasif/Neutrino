import NeutrinoKit
import SwiftUI

/// What needs you: deploys to approve, pull requests ready to merge, runs.
struct InboxView: View {
    @Environment(PhoneModel.self) private var model
    @State private var settings = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            List {
                if case let .offline(why) = model.status {
                    Section {
                        Label(why, systemImage: "wifi.exclamationmark").font(.footnote).foregroundStyle(N.warn)
                    }
                }
                if !model.inbox.waiting.isEmpty {
                    Section("Waiting for you") {
                        ForEach(model.inbox.waiting) { w in ApprovalRow(waiting: w) }
                    }
                }
                if !model.inbox.ready.isEmpty {
                    Section("Ready to merge") {
                        ForEach(model.inbox.ready) { p in
                            NavigationLink(value: PullRef(repo: p.repo, number: p.number)) { PullRow(pull: p, ready: true) }
                        }
                    }
                }
                if !model.inbox.open.isEmpty {
                    Section("Open pull requests") {
                        ForEach(model.inbox.open) { p in
                            NavigationLink(value: PullRef(repo: p.repo, number: p.number)) { PullRow(pull: p, ready: false) }
                        }
                    }
                }
                if !model.inbox.runs.isEmpty {
                    Section("Runs") {
                        ForEach(model.inbox.runs) { r in
                            NavigationLink(value: RunRef(repo: r.repo, id: r.id)) { PhoneRunRow(run: r) }
                        }
                    }
                }
                if !model.inbox.events.isEmpty {
                    Section("Lately") {
                        ForEach(model.inbox.events.prefix(10)) { e in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(e.title).font(.subheadline.weight(.medium))
                                Text(e.body).font(.caption).foregroundStyle(.secondary)
                                Text(Format.ago(e.at)).font(.caption2).foregroundStyle(.tertiary)
                            }
                        }
                    }
                }
                if model.status == .online, model.inbox.waiting.isEmpty, model.inbox.ready.isEmpty, model.inbox.open.isEmpty, model.inbox.runs.isEmpty {
                    ContentUnavailableView("Nothing needs you", systemImage: "checkmark.circle", description: Text("Deploys waiting for approval, pull requests and runs show up here."))
                }
            }
            .navigationTitle("Inbox")
            .refreshable { await model.refresh() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { settings = true } label: { Image(systemName: "gearshape") }.accessibilityLabel("Settings")
                }
            }
            .navigationDestination(for: PullRef.self) { PhonePullView(ref: $0) }
            .navigationDestination(for: RunRef.self) { PhoneRunView(ref: $0) }
            .navigationDestination(item: $model.openRun) { PhoneRunView(ref: $0) }
            .navigationDestination(item: $model.openPull) { PhonePullView(ref: $0) }
            .sheet(isPresented: $settings) { PhoneSettings() }
            .alert("Something went wrong", isPresented: Binding(get: { model.notice != nil }, set: { if !$0 { model.notice = nil } })) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(model.notice ?? "")
            }
            .overlay(alignment: .bottom) {
                if let busy = model.busy {
                    Label(busy + "…", systemImage: "hourglass").font(.footnote.weight(.medium))
                        .padding(.horizontal, 14).padding(.vertical, 8)
                        .background(.ultraThinMaterial, in: Capsule())
                        .padding(.bottom, 12)
                }
            }
        }
    }
}

struct ApprovalRow: View {
    @Environment(PhoneModel.self) private var model
    var waiting: Waiting

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "hand.raised.fill").foregroundStyle(N.warn)
                Text("Deploy to \(waiting.environment ?? "?")").font(.headline)
            }
            Text("\(waiting.runName) run \(waiting.number) · \(waiting.jobName)").font(.subheadline)
            Text("\(waiting.repo) · \(waiting.branch ?? "") · \(Format.ago(waiting.since))").font(.caption).foregroundStyle(.secondary)
            HStack {
                ApprovalButtons { yes in await model.approve(waiting, yes) }
            }
            .disabled(model.busy != nil)
        }
        .padding(.vertical, 4)
    }
}

struct PullRow: View {
    var pull: PullRequest
    var ready: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Text("#\(pull.number)").font(.caption.monospaced()).foregroundStyle(.secondary)
                Text(pull.title).font(.subheadline.weight(.medium)).lineLimit(2)
            }
            HStack(spacing: 6) {
                Text("\(pull.repo) · \(pull.source) → \(pull.target)").font(.caption.monospaced()).foregroundStyle(N.accent).lineLimit(1)
                Spacer()
                if pull.openNotes > 0 { Label("\(pull.openNotes)", systemImage: "text.bubble").font(.caption).foregroundStyle(N.warn) }
                if ready { Image(systemName: "checkmark.circle.fill").foregroundStyle(N.ok).accessibilityLabel("Ready to merge") }
            }
        }
    }
}

struct PhoneStateIcon: View {
    var state: RunState

    var body: some View {
        let (symbol, color): (String, Color) = switch state {
        case .queued: ("circle.dashed", N.faint)
        case .running: ("circle.dotted.circle", N.warn)
        case .success: ("checkmark.circle.fill", N.ok)
        case .failure: ("xmark.circle.fill", N.danger)
        case .cancelled: ("slash.circle", N.faint)
        case .skipped: ("minus.circle", N.faint)
        case .waiting: ("hand.raised.circle", N.warn)
        }
        Image(systemName: symbol).foregroundStyle(color).symbolEffect(.pulse, isActive: state == .running).accessibilityLabel(state.label)
    }
}

struct PhoneRunRow: View {
    var run: Run

    var body: some View {
        HStack(spacing: 10) {
            PhoneStateIcon(state: run.state)
            VStack(alignment: .leading, spacing: 2) {
                Text("\(run.name) \(run.number)").font(.subheadline.weight(.medium))
                Text("\(run.repo) · \(run.pull.map { "#\($0)" } ?? run.branch ?? run.shortSHA)").font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Text(Format.ago(run.createdAt)).font(.caption2).foregroundStyle(.tertiary)
        }
    }
}

/// Turn down and Approve, side by side in a List row. Borderless, so each
/// takes only its own taps: with bordered styles a List row hands a tap
/// anywhere in it to the first button.
struct ApprovalButtons: View {
    var act: (Bool) async -> Void

    var body: some View {
        HStack {
            Button(role: .destructive) { Task { await act(false) } } label: {
                Text("Turn down").pill(fill: N.danger.opacity(0.16), text: N.danger)
            }
            Spacer()
            Button { Task { await act(true) } } label: {
                Label("Approve", systemImage: "faceid").pill(fill: N.accent, text: N.onAccent)
            }
        }
        .buttonStyle(.borderless)
    }
}

extension View {
    func pill(fill: Color, text: Color) -> some View {
        font(.body.weight(.medium))
            .foregroundStyle(text)
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .background(Capsule().fill(fill))
            .contentShape(Capsule())
    }
}
