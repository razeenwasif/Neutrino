import NeutrinoKit
import SwiftUI

struct RunsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let problem = model.pullRepoProblem {
                EmptyState(title: "No runs here", message: problem).island()
            } else {
                HStack(alignment: .top, spacing: 10) {
                    RunList().frame(width: 340).frame(maxHeight: .infinity)
                    RunDetailPanel().frame(maxWidth: .infinity, maxHeight: .infinity)
                }
            }
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") { await model.loadRuns() }
        .onDisappear { model.runsTask?.cancel() }
    }
}

struct RunStateIcon: View {
    var state: RunState
    var size: CGFloat = 13

    var body: some View {
        let (symbol, color): (String, Color) = switch state {
        case .queued: ("circle.dashed", N.faint)
        case .running: ("circle.dotted.circle", N.warn)
        case .success: ("checkmark.circle.fill", N.ok)
        case .failure: ("xmark.circle.fill", N.danger)
        case .cancelled: ("slash.circle", N.faint)
        case .skipped: ("minus.circle", N.faint)
        }
        Image(systemName: symbol)
            .font(.system(size: size, weight: .semibold))
            .foregroundStyle(color)
            .symbolEffect(.pulse, isActive: state == .running)
            .accessibilityLabel(state.label)
    }
}

func formatDuration(_ seconds: Int64?) -> String {
    guard let s = seconds, s >= 0 else { return "" }
    if s < 60 { return "\(s)s" }
    if s < 3600 { return "\(s / 60)m \(s % 60)s" }
    return "\(s / 3600)h \(s % 3600 / 60)m"
}

// MARK: - List

struct RunList: View {
    @Environment(AppModel.self) private var model
    @State private var starting = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("Runs").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
                Spacer(minLength: 6)
                Button("Run workflow…") { starting = true }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 26))
                    .help("Start a workflow that has workflow_dispatch")
            }
            .padding(.horizontal, 14)
            .padding(.top, 12)
            .padding(.bottom, 10)

            if let error = model.runError, model.runList.isEmpty {
                EmptyState(title: "Can't load runs", message: error)
            } else if model.runList.isEmpty {
                EmptyState(title: "No runs yet", message: "Push a workflow file to .neutrino/workflows/ and its runs show up here.")
            } else {
                ScrollView {
                    LazyVStack(spacing: 2) {
                        ForEach(model.runList) { r in
                            RunRow(run: r, selected: r.id == model.runID)
                        }
                    }
                    .padding(.horizontal, 8)
                    .padding(.bottom, 10)
                }
            }
            CiFooter()
        }
        .island()
        .sheet(isPresented: $starting) { DispatchSheet() }
    }
}

struct CiFooter: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let info = model.ciInfo {
            HStack(spacing: 8) {
                Dot(color: info.graviton ? N.ok : N.danger, glow: info.graviton, size: 6)
                Text(info.graviton ? "Graviton" : "Graviton unreachable")
                Spacer()
                Text("\(info.usedCpus)/\(info.budgetCpus) CPUs · \(info.usedMemoryMib / 1024)/\(info.budgetMemoryMib / 1024) GB")
                if info.queued > 0 { Text("· \(info.queued) waiting") }
            }
            .font(.system(size: 11))
            .foregroundStyle(N.faint)
            .padding(.horizontal, 14)
            .frame(height: 30)
            .help(info.gravitonError ?? "CI's budget on the server, and what running jobs use of it")
        }
    }
}

struct RunRow: View {
    @Environment(AppModel.self) private var model
    var run: Run
    var selected: Bool
    @State private var hovering = false

    var body: some View {
        Button {
            Task { await model.select(run: run.id) }
        } label: {
            HStack(alignment: .top, spacing: 10) {
                RunStateIcon(state: run.state).padding(.top, 2)
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        Text(run.name).font(.system(size: 13, weight: .medium)).foregroundStyle(N.text).lineLimit(1)
                        Text("\(run.number)").font(N.mono(11)).foregroundStyle(N.faint)
                        Spacer(minLength: 0)
                    }
                    HStack(spacing: 8) {
                        if let n = run.pull {
                            Text("#\(n)").foregroundStyle(N.accent)
                        } else if let b = run.branch {
                            Text(b).foregroundStyle(N.accent).lineLimit(1)
                        }
                        Text(run.shortSHA).foregroundStyle(N.faint)
                        Spacer(minLength: 4)
                        Text(formatDuration(run.duration())).foregroundStyle(N.faint)
                        Text(Format.ago(run.createdAt)).font(.system(size: 11)).foregroundStyle(N.faint)
                    }
                    .font(N.mono(11))
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? N.selected : (hovering ? Color.white.opacity(0.04) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("\(run.eventLabel) · \(run.workflow)")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

// MARK: - Detail

struct RunDetailPanel: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let d = model.runDetail {
                VStack(spacing: 0) {
                    RunHeader(detail: d)
                    Rectangle().fill(N.hairline).frame(height: 1)
                    if d.jobs.isEmpty {
                        EmptyState(title: d.run.error != nil ? "The workflow file has a problem" : "No jobs", message: d.run.error ?? "Nothing in this run.")
                    } else {
                        HStack(spacing: 0) {
                            JobList(detail: d).frame(width: 230)
                            Rectangle().fill(N.hairline).frame(width: 1)
                            JobLogView()
                        }
                    }
                }
            } else if let error = model.runError {
                EmptyState(title: "Can't show this run", message: error)
            } else {
                EmptyState(title: "No run selected", message: "Pick one from the list.")
            }
        }
        .island()
    }
}

struct RunHeader: View {
    @Environment(AppModel.self) private var model
    var detail: RunDetail

    var body: some View {
        let r = detail.run
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                RunStateIcon(state: r.state, size: 16)
                Text(r.name).font(.system(size: 18, weight: .semibold)).foregroundStyle(N.text)
                Text("Run \(r.number)").font(N.mono(14)).foregroundStyle(N.faint)
                Spacer(minLength: 8)
                if !r.state.finished {
                    Button(model.runBusy == "Cancelling" ? "Cancelling…" : "Cancel") { Task { await model.cancelRun() } }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                        .disabled(model.runBusy != nil)
                } else if r.error == nil {
                    Button(model.runBusy == "Starting" ? "Starting…" : "Run again") { Task { await model.rerun() } }
                        .buttonStyle(PillButtonStyle(kind: .secondary, height: 28))
                        .disabled(model.runBusy != nil)
                        .help("The same workflow on the same commit")
                }
            }
            HStack(spacing: 10) {
                Text(r.state.label).foregroundStyle(r.state == .failure ? N.danger : (r.state == .success ? N.ok : N.muted))
                Text("·").foregroundStyle(N.faint)
                Text(r.eventLabel)
                if let n = r.pull {
                    Text("#\(n)").font(N.mono(12)).foregroundStyle(N.accent)
                } else if let b = r.branch {
                    Text(b).font(N.mono(12)).foregroundStyle(N.accent)
                }
                Text(r.shortSHA).font(N.mono(12)).foregroundStyle(N.faint).textSelection(.enabled)
                Text("·").foregroundStyle(N.faint)
                Text(r.workflow).font(N.mono(12)).foregroundStyle(N.faint)
                if let took = r.duration() {
                    Text("·").foregroundStyle(N.faint)
                    Text(r.state.finished ? "took \(formatDuration(took))" : "running for \(formatDuration(took))")
                }
            }
            .font(.system(size: 12))
            .foregroundStyle(N.muted)
            ForEach(detail.warnings, id: \.self) { w in
                Label(w, systemImage: "exclamationmark.triangle").font(.system(size: 11.5)).foregroundStyle(N.warn)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct JobList: View {
    @Environment(AppModel.self) private var model
    var detail: RunDetail

    var body: some View {
        ScrollView {
            VStack(spacing: 2) {
                ForEach(detail.jobs) { j in
                    Button {
                        model.select(job: j.id)
                    } label: {
                        HStack(spacing: 9) {
                            RunStateIcon(state: j.state)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(j.name).font(.system(size: 12.5, weight: .medium)).foregroundStyle(N.text).lineLimit(1)
                                if !j.needs.isEmpty {
                                    Text("after \(j.needs.joined(separator: ", "))").font(.system(size: 10.5)).foregroundStyle(N.faint).lineLimit(1)
                                }
                            }
                            Spacer(minLength: 4)
                            Text(formatDuration(j.startedAt.map { (j.finishedAt ?? Int64(Date().timeIntervalSince1970)) - $0 })).font(N.mono(10.5)).foregroundStyle(N.faint)
                        }
                        .padding(.horizontal, 10)
                        .frame(minHeight: 38)
                        .background(model.runJobID == j.id ? N.selected : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help(j.error ?? j.state.label)
                }
            }
            .padding(8)
        }
    }
}

struct JobLogView: View {
    @Environment(AppModel.self) private var model
    @State private var collapsed: Set<UInt32> = []
    @State private var expanded: Set<UInt32> = []

    var body: some View {
        if let job = model.selectedJob {
            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Text(job.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(N.text)
                    if let image = job.image { Text(image).font(N.mono(11.5)).foregroundStyle(N.faint) }
                    Text(job.executor).font(N.mono(11.5)).foregroundStyle(N.faint)
                    if job.attempt > 1 { Text("attempt \(job.attempt)").font(.system(size: 11)).foregroundStyle(N.warn) }
                    Spacer()
                    if let e = job.error { Text(e).font(.system(size: 11.5)).foregroundStyle(N.danger).lineLimit(1).truncationMode(.tail) }
                }
                .padding(.horizontal, 14)
                .frame(height: 40)
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 4) {
                            let setup = prelude(job)
                            if !setup.isEmpty { LogBlock(text: setup) }
                            ForEach(job.steps) { step in
                                StepSection(step: step, open: isOpen(step), toggle: { toggle(step) }, text: model.logText(of: step))
                                    .id(step.index)
                            }
                            if !model.jobLogDone, job.state == .running {
                                HStack(spacing: 6) {
                                    ProgressView().controlSize(.mini)
                                    Text("Following the log…").font(.system(size: 11)).foregroundStyle(N.faint)
                                }
                                .padding(.horizontal, 6)
                                .id("tail")
                            }
                        }
                        .padding(12)
                    }
                    .onChange(of: model.jobLogNext) {
                        if job.state == .running { proxy.scrollTo("tail", anchor: .bottom) }
                    }
                }
                .background(N.well)
            }
        } else {
            EmptyState(title: "No job selected", message: "Pick a job to see its steps and output.")
        }
    }

    /// Checkout and container start: what's logged before the first step.
    private func prelude(_ job: RunJob) -> String {
        let end = job.steps.first(where: { $0.state != .skipped && $0.state != .queued })?.logStart ?? UInt64(model.jobLogText.utf8.count)
        let bytes = Array(model.jobLogText.utf8)
        return String(decoding: bytes[0 ..< Int(min(end, UInt64(bytes.count)))], as: UTF8.self)
    }

    /// Failed and running steps start open; you can open or close any.
    private func isOpen(_ step: RunStep) -> Bool {
        if collapsed.contains(step.index) { return false }
        if expanded.contains(step.index) { return true }
        return step.state == .failure || step.state == .running
    }

    private func toggle(_ step: RunStep) {
        if isOpen(step) {
            collapsed.insert(step.index)
            expanded.remove(step.index)
        } else {
            expanded.insert(step.index)
            collapsed.remove(step.index)
        }
    }
}

struct StepSection: View {
    var step: RunStep
    var open: Bool
    var toggle: () -> Void
    var text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: toggle) {
                HStack(spacing: 8) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(N.faint)
                        .rotationEffect(.degrees(open ? 90 : 0))
                    RunStateIcon(state: step.state, size: 12)
                    Text(step.name).font(.system(size: 12.5, weight: .medium)).foregroundStyle(step.state == .skipped ? N.faint : N.text).lineLimit(1)
                    if let code = step.exitCode, code != 0 {
                        Text("exit \(code)").font(N.mono(11)).foregroundStyle(N.danger)
                    }
                    Spacer()
                    Text(formatDuration(step.startedAt.map { (step.finishedAt ?? Int64(Date().timeIntervalSince1970)) - $0 })).font(N.mono(10.5)).foregroundStyle(N.faint)
                }
                .padding(.horizontal, 6)
                .frame(height: 28)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(step.name), \(step.state.label)")
            .accessibilityValue(open ? "expanded" : "collapsed")
            if open {
                if text.isEmpty {
                    Text(step.state == .skipped ? "Skipped." : "No output.").font(.system(size: 11.5)).foregroundStyle(N.faint).padding(.leading, 40).padding(.vertical, 4)
                } else {
                    LogBlock(text: text).padding(.leading, 26)
                }
            }
        }
    }
}

/// Log text, with Neutrino's own lines (marked ▸) dimmed.
struct LogBlock: View {
    var text: String

    var body: some View {
        Text(styled)
            .font(N.mono(11.5))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
    }

    private var styled: AttributedString {
        var out = AttributedString()
        // Drop the colour codes tools print; the log is shown plain.
        let clean = text.replacingOccurrences(of: "\u{1B}\\[[0-9;?]*[A-Za-z]", with: "", options: .regularExpression)
        let lines = clean.split(separator: "\n", omittingEmptySubsequences: false)
        for (i, line) in lines.enumerated() {
            var part = AttributedString(String(line) + (i < lines.count - 1 ? "\n" : ""))
            part.foregroundColor = line.hasPrefix("\u{25B8}") ? N.faint : N.body
            out += part
        }
        return out
    }
}

// MARK: - Start a workflow

struct DispatchSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var branches: [Branch] = []
    @State private var branch = ""
    @State private var files: [WorkflowFile] = []
    @State private var workflow = ""
    @State private var working = false
    @State private var error: String?

    private var startable: [WorkflowFile] { files.filter { $0.triggers.contains("workflow_dispatch") } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Run a workflow").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Branch").font(.system(size: 11)).foregroundStyle(N.muted)
                    Picker("", selection: $branch) { ForEach(branches) { Text($0.name).tag($0.name) } }.labelsHidden().frame(width: 180)
                }
                VStack(alignment: .leading, spacing: 5) {
                    Text("Workflow").font(.system(size: 11)).foregroundStyle(N.muted)
                    Picker("", selection: $workflow) { ForEach(startable) { Text($0.name ?? $0.file).tag($0.file) } }.labelsHidden().frame(width: 220)
                        .disabled(startable.isEmpty)
                }
            }
            if startable.isEmpty, !files.isEmpty || !branch.isEmpty {
                Text("No workflow on \(branch) can be started by hand. Add workflow_dispatch to one's on:.")
                    .font(.system(size: 12)).foregroundStyle(N.muted).fixedSize(horizontal: false, vertical: true)
            }
            ForEach(files.filter { $0.error != nil }) { f in
                Label("\(f.file): \(f.error ?? "")", systemImage: "exclamationmark.triangle").font(.system(size: 11.5)).foregroundStyle(N.warn)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.danger) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button(working ? "Starting…" : "Run") {
                    working = true
                    Task {
                        defer { working = false }
                        do {
                            try await model.dispatch(workflow: workflow, branch: branch)
                            dismiss()
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                .keyboardShortcut(.defaultAction)
                .disabled(working || workflow.isEmpty || branch.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 480)
        .background(N.island)
        .task {
            branches = await model.hostedBranches()
            branch = branches.first { $0.name == model.hostedDefaultBranch }?.name ?? branches.first?.name ?? ""
        }
        .task(id: branch) {
            guard !branch.isEmpty else { return }
            files = await model.workflowFiles(branch: branch)
            if !startable.contains(where: { $0.file == workflow }) { workflow = startable.first?.file ?? "" }
        }
    }
}
