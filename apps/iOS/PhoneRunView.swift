import NeutrinoKit
import SwiftUI

/// A run: its jobs and steps, a job's log, cancelling, approving.
struct PhoneRunView: View {
    @Environment(PhoneModel.self) private var model
    var ref: RunRef
    @State private var detail: RunDetail?
    @State private var error: String?

    var body: some View {
        List {
            if let d = detail {
                let r = d.run
                Section {
                    HStack(spacing: 10) {
                        PhoneStateIcon(state: r.state).font(.title2)
                        VStack(alignment: .leading, spacing: 2) {
                            Text("\(r.name) run \(r.number)").font(.headline)
                            Text("\(r.eventLabel) · \(r.repo) · \(r.pull.map { "#\($0)" } ?? r.branch ?? "") · \(r.shortSHA)").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if let e = r.error { Text(e).font(.footnote).foregroundStyle(N.danger) }
                    if !r.state.finished {
                        Button(role: .destructive) {
                            Task { if let d = await model.cancel(repo: ref.repo, run: r) { detail = d } }
                        } label: {
                            Label("Cancel run", systemImage: "faceid")
                        }
                        .disabled(model.busy != nil)
                    }
                }
                ForEach(d.jobs) { j in
                    Section {
                        if j.state == .waiting {
                            VStack(alignment: .leading, spacing: 8) {
                                Label("Waiting to deploy to \(j.environment ?? "?")", systemImage: "hand.raised.fill").foregroundStyle(N.warn)
                                ApprovalButtons { yes in if let d = await model.approve(repo: ref.repo, job: j, yes) { detail = d } }
                                .disabled(model.busy != nil)
                            }
                        }
                        if let by = j.approvedBy { Text("Approved by \(by)").font(.caption).foregroundStyle(.secondary) }
                        if let e = j.error { Text(e).font(.footnote).foregroundStyle(N.danger) }
                        ForEach(j.steps) { s in
                            HStack(spacing: 8) {
                                PhoneStateIcon(state: s.state).font(.caption)
                                Text(s.name).font(.subheadline).foregroundStyle(s.state == .skipped ? .secondary : .primary).lineLimit(1)
                                Spacer()
                                if let code = s.exitCode, code != 0 { Text("exit \(code)").font(.caption.monospaced()).foregroundStyle(N.danger) }
                            }
                        }
                        NavigationLink("Log") { PhoneLogView(repo: ref.repo, job: j) }
                    } header: {
                        HStack {
                            PhoneStateIcon(state: j.state)
                            Text(j.name)
                        }
                    }
                }
            } else if let error {
                ContentUnavailableView("Can't load it", systemImage: "exclamationmark.triangle", description: Text(error))
            } else {
                ProgressView()
            }
        }
        .navigationTitle("Run")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable { await load() }
        .task {
            await load()
            // Keep it current while it runs.
            while !Task.isCancelled, detail?.run.state.finished == false {
                try? await Task.sleep(for: .seconds(3))
                await load()
            }
        }
    }

    private func load() async {
        guard let service = model.service else { return }
        do {
            detail = try await service.run(repo: ref.repo, id: ref.id)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// A job's log, followed while the job runs.
struct PhoneLogView: View {
    @Environment(PhoneModel.self) private var model
    var repo: String
    var job: RunJob
    @State private var text = ""
    @State private var next: UInt64 = 0

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                Text(text.isEmpty ? "No output yet." : text)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                Color.clear.frame(height: 1).id("end")
            }
            .onChange(of: next) { proxy.scrollTo("end") }
        }
        .background(N.well)
        .navigationTitle(job.name)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            guard let service = model.service else { return }
            while !Task.isCancelled {
                guard let chunk = try? await service.jobLog(repo: repo, job: job.id, offset: next) else { return }
                text += chunk.text.replacingOccurrences(of: "\u{1B}\\[[0-9;?]*[A-Za-z]", with: "", options: .regularExpression)
                let grew = chunk.next > next
                next = chunk.next
                if chunk.done { return }
                if !grew { try? await Task.sleep(for: .seconds(1)) }
            }
        }
    }
}
