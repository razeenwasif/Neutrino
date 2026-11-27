import NeutrinoKit
import SwiftUI

struct RunsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let problem = model.pullRepoProblem {
                EmptyState(icon: "play.circle", title: "No runs here", message: problem)
                    .background(N.ground)
            } else {
                HStack(spacing: 0) {
                    RunList()
                        .frame(width: 340)
                        .frame(maxHeight: .infinity)
                    RunDetailPanel()
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
        case .running: ("circle.dotted.circle", N.amber)
        case .success: ("checkmark.circle.fill", N.mint)
        case .failure: ("xmark.circle.fill", N.rose)
        case .cancelled: ("slash.circle", N.slate)
        case .skipped: ("minus.circle", N.slate)
        case .waiting: ("hand.raised.circle", N.amber)
        }
        Image(systemName: symbol)
            .font(.system(size: size, weight: .semibold))
            .foregroundStyle(color)
            .symbolEffect(.pulse, isActive: state == .running)
            .accessibilityLabel(state.label)
    }
}

/// A commit's or branch's CI result; opens its run when clicked.
struct CheckMark: View {
    @Environment(AppModel.self) private var model
    var sha: String

    var body: some View {
        if let state = model.checkState(for: sha) {
            let runs = model.checkRuns(for: sha)
            Button {
                if let r = model.runToOpen(for: sha) { Task { await model.openRun(r.id) } }
            } label: {
                RunStateIcon(state: state, size: 11.5)
            }
            .buttonStyle(.plain)
            .help(runs.map { "\($0.name): \($0.state.label)" }.joined(separator: "\n") + "\nClick to open the run.")
            .accessibilityLabel("CI \(state.label)")
        }
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
                Text("Runs")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Spacer(minLength: 6)
                ActionPillButton(title: "Run workflow…", systemImage: "play.fill", tint: N.amber, background: N.surfacePill) {
                    starting = true
                }
                .help("Start a workflow that has workflow_dispatch")
            }
            .padding(.horizontal, 16)
            .padding(.top, 18)
            .padding(.bottom, 12)

            if let error = model.runError, model.runList.isEmpty {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't load runs", message: error)
            } else if model.runList.isEmpty {
                EmptyState(icon: "play.circle", title: "No runs yet", message: "Push a workflow file to .neutrino/workflows/ and its runs show up here.")
            } else {
                ScrollView(.vertical, showsIndicators: false) {
                    LazyVStack(spacing: 2) {
                        ForEach(model.runList) { r in
                            RunRow(run: r, selected: r.id == model.runID)
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 12)
                }
            }

            CiFooter()
        }
        .background(N.inspector)
        .sheet(isPresented: $starting) { DispatchSheet() }
        .onAppear { takeSheetRequest() }
        .onChange(of: model.sheetRequest) { takeSheetRequest() }
    }
}

struct CiFooter: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let info = model.ciInfo {
            HStack(spacing: 8) {
                StatusDot(color: info.graviton ? N.mint : N.rose, size: 5.5, pulse: info.graviton)
                Text(info.graviton ? "Graviton" : "Graviton unreachable")
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(info.graviton ? N.text : N.rose)
                Spacer()
                Text("\(info.usedCpus)/\(info.budgetCpus) CPUs · \(info.usedMemoryMib / 1024)/\(info.budgetMemoryMib / 1024) GB")
                    .font(N.mono(10.5))
                    .foregroundStyle(N.faint)
                if info.queued > 0 {
                    Text("· \(info.queued)q")
                        .font(N.mono(10.5))
                        .foregroundStyle(N.amber)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .padding(12)
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
                        Text(run.name)
                            .font(.system(size: 13, weight: selected ? .semibold : .medium))
                            .foregroundStyle(N.text)
                            .lineLimit(1)
                        Text("#\(run.number)")
                            .font(N.mono(11))
                            .foregroundStyle(N.faint)
                        Spacer(minLength: 0)
                    }
                    HStack(spacing: 8) {
                        if let n = run.pull {
                            Text("#\(n)").foregroundStyle(N.indigo)
                        } else if let b = run.branch {
                            Text(b).foregroundStyle(N.indigo).lineLimit(1)
                        }
                        Text(run.shortSHA).foregroundStyle(N.faint)
                        Spacer(minLength: 4)
                        Text(formatDuration(run.duration())).foregroundStyle(N.muted)
                        Text(Format.ago(run.createdAt)).font(N.mono(10.5)).foregroundStyle(N.faint)
                    }
                    .font(N.mono(11))
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear),
                in: RoundedRectangle(cornerRadius: 9, style: .continuous)
            )
            .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
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
                    if d.jobs.isEmpty {
                        EmptyState(icon: "exclamationmark.triangle", title: d.run.error != nil ? "The workflow file has a problem" : "No jobs", message: d.run.error ?? "Nothing in this run.")
                    } else {
                        HStack(spacing: 0) {
                            JobList(detail: d).frame(width: 232)
                            JobLogView()
                        }
                    }
                }
            } else if let error = model.runError {
                EmptyState(icon: "exclamationmark.triangle", title: "Can't show this run", message: error)
            } else {
                EmptyState(icon: "play.circle", title: "No run selected", message: "Pick one from the list.")
            }
        }
        .background(N.ground)
    }
}

struct RunHeader: View {
    @Environment(AppModel.self) private var model
    var detail: RunDetail

    var body: some View {
        let r = detail.run
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                RunStateIcon(state: r.state, size: 16)
                Text(r.name)
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("#\(r.number)")
                    .font(N.mono(15))
                    .foregroundStyle(N.faint)
                Spacer(minLength: 8)
                if !r.state.finished {
                    ActionPillButton(
                        title: model.runBusy == "Cancelling" ? "Cancelling…" : "Cancel",
                        systemImage: "stop.fill",
                        tint: N.rose,
                        background: N.surfacePill,
                        isLoading: model.runBusy == "Cancelling"
                    ) {
                        Task { await model.cancelRun() }
                    }
                    .disabled(model.runBusy != nil)
                } else if r.error == nil {
                    ActionPillButton(
                        title: model.runBusy == "Starting" ? "Starting…" : "Run again",
                        systemImage: "arrow.clockwise",
                        tint: N.text,
                        background: N.surfacePill,
                        isLoading: model.runBusy == "Starting"
                    ) {
                        Task { await model.rerun() }
                    }
                    .disabled(model.runBusy != nil)
                    .help("The same workflow on the same commit")
                }
            }
            HStack(spacing: 10) {
                Text(r.state.label)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(r.state == .failure ? N.rose : (r.state == .success ? N.mint : N.muted))
                Text("·").foregroundStyle(N.faint)
                Text(r.eventLabel)
                if let n = r.pull {
                    TagPill(text: "#\(n)", textColor: N.indigo, monospaced: true)
                } else if let b = r.branch {
                    TagPill(text: b, textColor: N.indigo, monospaced: true)
                }
                Text(r.shortSHA).font(N.mono(12)).foregroundStyle(N.sky).textSelection(.enabled)
                Text("·").foregroundStyle(N.faint)
                Text(r.workflow).font(N.mono(12)).foregroundStyle(N.faint)
                if let took = r.duration() {
                    Text("·").foregroundStyle(N.faint)
                    Text(r.state.finished ? "took \(formatDuration(took))" : "running for \(formatDuration(took))")
                        .font(N.mono(11.5))
                }
            }
            .font(.system(size: 12))
            .foregroundStyle(N.muted)

            ForEach(detail.warnings, id: \.self) { w in
                Label(w, systemImage: "exclamationmark.triangle")
                    .font(.system(size: 11.5))
                    .foregroundStyle(N.amber)
            }

            if !model.artifactList.isEmpty {
                HStack(spacing: 8) {
                    Image(systemName: "shippingbox").font(.system(size: 11)).foregroundStyle(N.faint)
                    ForEach(model.artifactList.prefix(6)) { a in
                        ActionPillButton(
                            title: (a.path as NSString).lastPathComponent,
                            systemImage: "arrow.down.circle",
                            tint: N.sky,
                            background: N.surfacePill
                        ) {
                            Task { await model.save(a) }
                        }
                        .help("Save \(a.job)/\(a.path) (\(a.size.formatted(.byteCount(style: .file))))")
                    }
                    if model.artifactList.count > 6 {
                        Menu("\(model.artifactList.count - 6) more") {
                            ForEach(model.artifactList.dropFirst(6)) { a in Button("\(a.job)/\(a.path)") { Task { await model.save(a) } } }
                        }
                        .menuStyle(.button).buttonStyle(.plain).fixedSize().font(.system(size: 11.5))
                    }
                }
            }
        }
        .task(id: "\(r.id):\(r.state.rawValue)") { await model.loadArtifacts() }
        .padding(.horizontal, 28)
        .padding(.vertical, 18)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(N.ground)
    }
}

struct JobList: View {
    @Environment(AppModel.self) private var model
    var detail: RunDetail

    var body: some View {
        ScrollView(.vertical, showsIndicators: false) {
            VStack(spacing: 3) {
                SectionLabel("Jobs")
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .padding(.bottom, 4)

                ForEach(detail.jobs) { j in
                    let isSelected = model.runJobID == j.id
                    Button {
                        model.select(job: j.id)
                    } label: {
                        HStack(spacing: 9) {
                            RunStateIcon(state: j.state)
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 5) {
                                    Text(j.name)
                                        .font(.system(size: 12.5, weight: isSelected ? .semibold : .medium))
                                        .foregroundStyle(N.text)
                                        .lineLimit(1)
                                    if j.frozenSnapshotPath != nil {
                                        Image(systemName: "snowflake")
                                            .font(.system(size: 10.5))
                                            .foregroundStyle(N.sky)
                                    }
                                }
                                if !j.needs.isEmpty {
                                    Text("after \(j.needs.joined(separator: ", "))")
                                        .font(.system(size: 10.5))
                                        .foregroundStyle(N.faint)
                                        .lineLimit(1)
                                }
                            }
                            Spacer(minLength: 4)
                            Text(formatDuration(j.startedAt.map { (j.finishedAt ?? Int64(Date().timeIntervalSince1970)) - $0 }))
                                .font(N.mono(10.5))
                                .foregroundStyle(N.faint)
                        }
                        .padding(.horizontal, 10)
                        .frame(minHeight: 38)
                        .background(isSelected ? N.surfaceActive : Color.clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .help(j.error ?? j.state.label)
                }
            }
            .padding(12)
        }
        .background(N.sidebar)
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
                    Text(job.name)
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundStyle(N.text)
                    if let image = job.image {
                        TagPill(text: image, textColor: N.sky, monospaced: true)
                    }
                    TagPill(text: job.executor, monospaced: true)
                    if job.attempt > 1 {
                        TagPill(text: "attempt \(job.attempt)", textColor: N.amber)
                    }
                    Spacer()
                    if let e = job.error {
                        Text(e)
                            .font(.system(size: 11.5))
                            .foregroundStyle(N.rose)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }
                .padding(.horizontal, 20)
                .frame(height: 44)
                .background(N.surfaceSubtle)

                if job.state == .waiting {
                    ApprovalBanner(job: job)
                        .padding(.top, 10)
                } else if let by = job.approvedBy, let env = job.environment {
                    Label("Deployed to \(env), approved by \(by)", systemImage: "checkmark.seal")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.mint)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 20)
                        .padding(.vertical, 8)
                }

                if job.frozenSnapshotPath != nil {
                    HStack(spacing: 12) {
                        Image(systemName: "snowflake")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(N.sky)
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text("Frozen Failure VM Snapshot")
                                    .font(.system(size: 12, weight: .semibold))
                                    .foregroundStyle(N.text)
                                if let bytes = job.frozenSizeBytes {
                                    Text(bytes.formatted(.byteCount(style: .file)))
                                        .font(N.mono(11))
                                        .foregroundStyle(N.muted)
                                }
                            }
                            if let step = job.frozenFailedStep {
                                Text("Captured at `\(step)`\(job.frozenExitCode.map { " (exit \($0))" } ?? "")")
                                    .font(.system(size: 11))
                                    .foregroundStyle(N.muted)
                            }
                        }
                        Spacer(minLength: 8)
                        ActionPillButton(
                            title: model.runBusy == "Restoring VM" ? "Restoring…" : "Attach to Frozen VM",
                            systemImage: "terminal.fill",
                            tint: N.sky,
                            background: N.sky.opacity(0.14),
                            isLoading: model.runBusy == "Restoring VM"
                        ) {
                            Task { await model.attachFrozen(job: job) }
                        }
                        .disabled(model.runBusy != nil)
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 10)
                    .background(N.sky.opacity(0.08))
                }

                ScrollViewReader { proxy in
                    ScrollView(.vertical, showsIndicators: true) {
                        LazyVStack(alignment: .leading, spacing: 6) {
                            let setup = prelude(job)
                            if !setup.isEmpty { LogBlock(text: setup) }
                            ForEach(job.steps) { step in
                                StepSection(step: step, open: isOpen(step), toggle: { toggle(step) }, text: model.logText(of: step))
                                    .id(step.index)
                            }
                            if !model.jobLogDone, job.state == .running {
                                HStack(spacing: 6) {
                                    ProgressView().controlSize(.mini)
                                    Text("Following the log…")
                                        .font(.system(size: 11))
                                        .foregroundStyle(N.faint)
                                }
                                .padding(.horizontal, 6)
                                .id("tail")
                            }
                        }
                        .padding(16)
                    }
                    .onChange(of: model.jobLogNext) {
                        if job.state == .running { proxy.scrollTo("tail", anchor: .bottom) }
                    }
                }
                .background(N.ground)
            }
        } else {
            EmptyState(icon: "terminal", title: "No job selected", message: "Pick a job to see its steps and output.")
                .background(N.ground)
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
                    Text(step.name)
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundStyle(step.state == .skipped ? N.faint : N.text)
                        .lineLimit(1)
                    if let code = step.exitCode, code != 0 {
                        TagPill(text: "exit \(code)", textColor: N.rose, background: N.rose.opacity(0.14), monospaced: true)
                    }
                    Spacer()
                    Text(formatDuration(step.startedAt.map { (step.finishedAt ?? Int64(Date().timeIntervalSince1970)) - $0 }))
                        .font(N.mono(10.5))
                        .foregroundStyle(N.faint)
                }
                .padding(.horizontal, 10)
                .frame(height: 30)
                .background(open ? N.surfaceSubtle : Color.clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("\(step.name), \(step.state.label)")
            .accessibilityValue(open ? "expanded" : "collapsed")

            if open {
                if text.isEmpty {
                    Text(step.state == .skipped ? "Skipped." : "No output.")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.faint)
                        .padding(.leading, 38)
                        .padding(.vertical, 6)
                } else {
                    LogBlock(text: text).padding(.leading, 22)
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
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
    }

    private var styled: AttributedString {
        var out = AttributedString()
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
        VStack(alignment: .leading, spacing: 16) {
            Text("Run a workflow")
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(N.text)
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Branch").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                    Picker("", selection: $branch) { ForEach(branches) { Text($0.name).tag($0.name) } }.labelsHidden().frame(width: 180)
                }
                VStack(alignment: .leading, spacing: 5) {
                    Text("Workflow").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                    Picker("", selection: $workflow) { ForEach(startable) { Text($0.name ?? $0.file).tag($0.file) } }.labelsHidden().frame(width: 220)
                        .disabled(startable.isEmpty)
                }
            }
            if startable.isEmpty, !files.isEmpty || !branch.isEmpty {
                Text("No workflow on \(branch) can be started by hand. Add workflow_dispatch to one's on:.")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            ForEach(files.filter { $0.error != nil }) { f in
                Label("\(f.file): \(f.error ?? "")", systemImage: "exclamationmark.triangle")
                    .font(.system(size: 11.5))
                    .foregroundStyle(N.amber)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.rose) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
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
        .padding(24)
        .frame(width: 480)
        .background(N.sheet)
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

/// A job waiting on its environment: who may let it go.
struct ApprovalBanner: View {
    @Environment(AppModel.self) private var model
    var job: RunJob

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "hand.raised.fill").foregroundStyle(N.amber)
            VStack(alignment: .leading, spacing: 2) {
                Text("Waiting to deploy to \(job.environment ?? "its environment")")
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("It runs with that environment's secrets once you approve it.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(N.muted)
            }
            Spacer()
            ActionPillButton(
                title: model.runBusy == "Turning down" ? "Turning down…" : "Turn down",
                tint: N.rose,
                background: N.surfacePill
            ) {
                Task { await model.decide(job: job, approve: false) }
            }
            .disabled(model.runBusy != nil)

            ActionPillButton(
                title: model.runBusy == "Approving" ? "Approving…" : "Approve",
                systemImage: "checkmark",
                tint: N.onAccent,
                background: N.mint
            ) {
                Task { await model.decide(job: job, approve: true) }
            }
            .disabled(model.runBusy != nil)
        }
        .padding(12)
        .background(N.amber.opacity(0.08), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.horizontal, 16)
        .padding(.bottom, 6)
    }
}

extension RunList {
    /// Open Run workflow when the palette asked for it.
    func takeSheetRequest() {
        guard model.sheetRequest == .runWorkflow else { return }
        model.sheetRequest = nil
        starting = true
    }
}
