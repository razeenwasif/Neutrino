import NeutrinoKit
import SwiftUI

struct SecretsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let problem = model.pullRepoProblem, model.vaultInfo == nil {
                EmptyState(icon: "key", title: "No secrets here", message: problem)
                    .background(N.ground)
            } else {
                VStack(alignment: .leading, spacing: 22) {
                    ScopeBar()

                    if model.vaultInfo?.setUp == false {
                        VaultNotSetUp()
                    }

                    HStack(alignment: .top, spacing: 18) {
                        SecretList()
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                        VariableList()
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                        if model.pullRepo != nil {
                            EnvironmentList()
                                .frame(width: 300)
                                .frame(maxHeight: .infinity)
                        }
                    }
                }
                .padding(.horizontal, 32)
                .padding(.top, 22)
                .padding(.bottom, 28)
                .background(N.ground)
            }
        }
        .task(id: "\(model.source.rawValue):\(model.repoID ?? ""):\(model.isServerOnline)") { await model.loadSecrets() }
        .onChange(of: model.vaultView) { Task { await model.loadSecrets() } }
    }
}

struct ScopeBar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Secrets & Variables")
                    .font(N.pageTitle())
                    .foregroundStyle(N.text)
                Text("Encrypted vault credentials, workflow variables, and deployment environments")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
            }
            Spacer()
            if let repo = model.pullRepo {
                HStack(spacing: 2) {
                    FilterPillButton(title: repo, isSelected: model.vaultView == .repo) {
                        model.vaultView = .repo
                    }
                    FilterPillButton(title: "Every repository", isSelected: model.vaultView == .everyRepo) {
                        model.vaultView = .everyRepo
                    }
                    ForEach(model.environmentList) { env in
                        FilterPillButton(title: "\(repo) · \(env.name)", isSelected: model.vaultView == .environment(env.name)) {
                            model.vaultView = .environment(env.name)
                        }
                    }
                }
                .padding(3)
                .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            } else {
                TagPill(text: "Every repository", textColor: N.muted)
            }
        }
    }
}

struct VaultNotSetUp: View {
    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "lock.trianglebadge.exclamationmark")
                .foregroundStyle(N.amber)
                .font(.system(size: 15))
            VStack(alignment: .leading, spacing: 3) {
                Text("The vault isn't set up yet")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("On the server, run `ntr vault setup`. It creates the key that seals secrets and shows its recovery code once; keep that code somewhere away from the server. Variables work without it.")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
        }
        .padding(14)
        .background(N.amber.opacity(0.08), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

private struct PanelHeader: View {
    var title: String
    var detail: String
    var action: (String, Bool, () -> Void)?

    var body: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(N.text)
                Text(detail)
                    .font(.system(size: 11.5))
                    .foregroundStyle(N.faint)
            }
            Spacer()
            if let (label, enabled, act) = action {
                ActionPillButton(title: label, systemImage: "plus", tint: N.text, background: N.surfacePill, action: act)
                    .disabled(!enabled)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 16)
        .padding(.bottom, 12)
    }
}

/// Where a wider-scoped entry comes from, shown beside it.
private struct Inherited: View {
    var scope: VaultScope

    var body: some View {
        TagPill(
            text: scope.repo == nil ? "every repo" : (scope.environment == nil ? "this repo" : scope.environment!),
            textColor: N.faint
        )
        .help("Set for \(scope.label); one set here would take its place")
    }
}

struct SecretList: View {
    @Environment(AppModel.self) private var model
    @State private var editing: String?
    @State private var adding = false

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader(title: "Secrets", detail: "Sealed on the server. Write-only: values never come back.",
                        action: ("Add", model.vaultInfo?.setUp == true, { adding = true }))
            if model.secretList.isEmpty {
                EmptyState(icon: "key", title: "No secrets", message: "Workflows read them as ${{ secrets.NAME }}; logs show *** in their place.")
            } else {
                ScrollView(.vertical, showsIndicators: false) {
                    VStack(spacing: 4) {
                        ForEach(model.secretList) { s in
                            HStack(spacing: 8) {
                                Image(systemName: "key.fill")
                                    .font(.system(size: 10))
                                    .foregroundStyle(model.isOwn(s.scope) ? N.rose : N.faint)
                                Text(s.name)
                                    .font(N.mono(12.5, weight: .medium))
                                    .foregroundStyle(model.isOwn(s.scope) ? N.text : N.muted)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .help(s.name)
                                if !model.isOwn(s.scope) { Inherited(scope: s.scope).fixedSize() }
                                if s.kind == .jitToken {
                                    TagPill(text: "JIT", color: N.amber, textColor: N.amber, background: N.amber.opacity(0.14))
                                        .fixedSize()
                                        .help("Auto-expiring deploy token cleared after run finishes")
                                }
                                Spacer()
                                Text(Format.ago(s.updatedAt))
                                    .font(N.mono(10.5))
                                    .foregroundStyle(N.faint)
                                Menu {
                                    Button("Reveal value (Touch ID)…") { Task { await model.revealSecret(s) } }
                                    if model.isOwn(s.scope) {
                                        Divider()
                                        Button("Replace value…") { editing = s.name }
                                        Button("Delete", role: .destructive) { Task { await model.deleteSecret(s) } }
                                    }
                                } label: {
                                    Image(systemName: "ellipsis")
                                        .font(.system(size: 11, weight: .semibold))
                                        .foregroundStyle(N.muted)
                                }
                                .menuStyle(.button).buttonStyle(.plain).fixedSize()
                                .accessibilityLabel("Actions for \(s.name)")
                            }
                            .padding(.horizontal, 12)
                            .frame(height: 36)
                            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
                }
            }
        }
        .background(N.inspector, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .sheet(isPresented: $adding) { SecretSheet(name: "") }
        .sheet(item: Binding(get: { editing.map { IdentifiedName(name: $0) } }, set: { editing = $0?.name })) { SecretSheet(name: $0.name) }
        .sheet(item: Binding(get: { model.revealedSecret }, set: { model.revealedSecret = $0 })) { item in
            SecretRevealSheet(item: item)
        }
    }
}

struct IdentifiedName: Identifiable {
    var name: String
    var id: String { name }
}

struct SecretSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var name: String
    @State private var value = ""
    @State private var isJit = false
    @State private var ttlSeconds: UInt64 = 900
    @State private var error: String?
    @State private var working = false

    var body: some View {
        let replacing = model.secretList.contains { $0.name == name && model.isOwn($0.scope) }
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(replacing ? "Replace \(name)" : "New secret")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("For \(model.vaultScope.label).")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                TextField("", text: $name, prompt: Text("DEPLOY_TOKEN").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .disabled(replacing)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Value").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                SecureField("", text: $value, prompt: Text("Pasted, not shown").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                Text("Sealed on the server with XChaCha20-Poly1305. Break-glass reveals require Touch ID or phone approval.")
                    .font(.system(size: 11))
                    .foregroundStyle(N.faint)
            }
            VStack(alignment: .leading, spacing: 8) {
                Toggle("JIT deploy token (auto-purged after job)", isOn: $isJit)
                    .toggleStyle(.checkbox)
                    .font(.system(size: 12.5))
                if isJit {
                    Picker("TTL", selection: $ttlSeconds) {
                        Text("5 minutes").tag(UInt64(300))
                        Text("15 minutes").tag(UInt64(900))
                        Text("1 hour").tag(UInt64(3600))
                    }
                    .pickerStyle(.segmented)
                }
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.rose).fixedSize(horizontal: false, vertical: true) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
                Button(working ? "Saving…" : "Save") {
                    working = true
                    Task {
                        defer { working = false }
                        do {
                            try await model.setSecret(
                                name: name.trimmingCharacters(in: .whitespaces),
                                value: value,
                                kind: isJit ? .jitToken : .standard,
                                ttlSeconds: isJit ? ttlSeconds : nil
                            )
                            value = ""
                            dismiss()
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                .keyboardShortcut(.defaultAction)
                .disabled(working || name.trimmingCharacters(in: .whitespaces).isEmpty || value.isEmpty)
            }
        }
        .padding(24)
        .frame(width: 440)
        .background(N.sheet)
    }
}

struct SecretRevealSheet: View {
    @Environment(AppModel.self) private var model
    var item: SecretRevealState
    @State private var remaining = 30

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Image(systemName: "lock.open.fill")
                    .foregroundStyle(N.amber)
                Text("Break-Glass Reveal · \(item.name)")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(N.text)
                Spacer()
                TagPill(text: "\(remaining)s", color: N.amber, textColor: N.amber, background: N.amber.opacity(0.14))
            }
            Text("Scope: \(item.scopeLabel). Recorded in the server audit log.")
                .font(.system(size: 12))
                .foregroundStyle(N.muted)
            Text(item.value)
                .font(N.mono(13))
                .foregroundStyle(N.text)
                .textSelection(.enabled)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            HStack {
                Button("Copy secret") { model.copy(item.value, label: item.name) }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                Spacer()
                Button("Hide now") { model.revealedSecret = nil }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
            }
        }
        .padding(24)
        .frame(width: 460)
        .background(N.sheet)
        .task(id: item.id) {
            while !Task.isCancelled {
                let left = Int(ceil(item.expiresAt.timeIntervalSinceNow))
                if left <= 0 {
                    model.revealedSecret = nil
                    break
                }
                remaining = left
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }
}

struct VariableList: View {
    @Environment(AppModel.self) private var model
    @State private var editing: Variable?
    @State private var adding = false

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader(title: "Variables", detail: "Plain values, visible to anyone who can see runs.", action: ("Add", true, { adding = true }))
            if model.variableList.isEmpty {
                EmptyState(icon: "textformat.abc", title: "No variables", message: "Workflows read them as ${{ vars.NAME }}.")
            } else {
                ScrollView(.vertical, showsIndicators: false) {
                    VStack(spacing: 4) {
                        ForEach(model.variableList) { v in
                            HStack(spacing: 8) {
                                Text(v.name)
                                    .font(N.mono(12.5, weight: .medium))
                                    .foregroundStyle(model.isOwn(v.scope) ? N.text : N.muted)
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .layoutPriority(1)
                                    .help(v.name)
                                if !model.isOwn(v.scope) { Inherited(scope: v.scope).fixedSize() }
                                Text(v.value)
                                    .font(N.mono(12))
                                    .foregroundStyle(N.sky)
                                    .lineLimit(1)
                                    .truncationMode(.tail)
                                    .textSelection(.enabled)
                                    .help(v.value)
                                Spacer()
                                if model.isOwn(v.scope) {
                                    Menu {
                                        Button("Edit…") { editing = v }
                                        Button("Delete", role: .destructive) { Task { await model.deleteVariable(v) } }
                                    } label: {
                                        Image(systemName: "ellipsis")
                                            .font(.system(size: 11, weight: .semibold))
                                            .foregroundStyle(N.muted)
                                    }
                                    .menuStyle(.button).buttonStyle(.plain).fixedSize()
                                    .accessibilityLabel("Actions for \(v.name)")
                                }
                            }
                            .padding(.horizontal, 12)
                            .frame(height: 36)
                            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
                }
            }
        }
        .background(N.inspector, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .sheet(isPresented: $adding) { VariableSheet(name: "", value: "", editing: false) }
        .sheet(item: $editing) { VariableSheet(name: $0.name, value: $0.value, editing: true) }
    }
}

struct VariableSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var name: String
    @State var value: String
    var editing: Bool
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(editing ? "Edit \(name)" : "New variable")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("For \(model.vaultScope.label).")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                TextField("", text: $name, prompt: Text("REGION").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .disabled(editing)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Value").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                TextField("", text: $value, prompt: Text("eu-west").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.rose) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
                Button("Save") {
                    Task {
                        do {
                            try await model.setVariable(name: name.trimmingCharacters(in: .whitespaces), value: value)
                            dismiss()
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                .keyboardShortcut(.defaultAction)
                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(24)
        .frame(width: 420)
        .background(N.sheet)
    }
}

struct EnvironmentList: View {
    @Environment(AppModel.self) private var model
    @State private var editing: DeployEnvironment?
    @State private var adding = false

    var body: some View {
        VStack(spacing: 0) {
            PanelHeader(title: "Environments", detail: "Where jobs deploy: own secrets, approval.", action: ("Add", true, { adding = true }))
            if model.environmentList.isEmpty {
                EmptyState(icon: "server.rack", title: "No environments", message: "A job with environment: production gets its secrets, and waits for you if it needs approval.")
            } else {
                ScrollView(.vertical, showsIndicators: false) {
                    VStack(spacing: 6) {
                        ForEach(model.environmentList) { e in
                            VStack(alignment: .leading, spacing: 5) {
                                HStack {
                                    Text(e.name)
                                        .font(.system(size: 13, weight: .semibold))
                                        .foregroundStyle(N.text)
                                    Spacer()
                                    Menu {
                                        Button("Edit…") { editing = e }
                                        Button("Delete", role: .destructive) { Task { await model.deleteEnvironment(e) } }
                                    } label: {
                                        Image(systemName: "ellipsis")
                                            .font(.system(size: 11, weight: .semibold))
                                            .foregroundStyle(N.muted)
                                    }
                                    .menuStyle(.button).buttonStyle(.plain).fixedSize()
                                    .accessibilityLabel("Actions for \(e.name)")
                                }
                                Label(e.requireApproval ? "Needs approval" : "No approval", systemImage: e.requireApproval ? "hand.raised.fill" : "hand.raised.slash")
                                    .font(.system(size: 11.5))
                                    .foregroundStyle(e.requireApproval ? N.amber : N.faint)
                                Text(e.branches.isEmpty ? "Any branch" : e.branches.joined(separator: ", "))
                                    .font(N.mono(11))
                                    .foregroundStyle(N.muted)
                            }
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
                }
            }
        }
        .background(N.inspector, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .sheet(isPresented: $adding) { EnvironmentSheet(env: DeployEnvironment(repo: model.pullRepo ?? "", name: "", requireApproval: true), editing: false) }
        .sheet(item: $editing) { EnvironmentSheet(env: $0, editing: true) }
    }
}

struct EnvironmentSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var env: DeployEnvironment
    var editing: Bool
    @State private var branches = ""
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(editing ? "Edit \(env.name)" : "New environment")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("For \(model.repo?.name ?? "this repository").")
                    .font(.system(size: 12))
                    .foregroundStyle(N.muted)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                TextField("", text: $env.name, prompt: Text("production").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .disabled(editing)
            }
            Toggle("Jobs wait for approval", isOn: $env.requireApproval)
                .toggleStyle(.checkbox)
                .font(.system(size: 12.5))
            VStack(alignment: .leading, spacing: 5) {
                Text("Branches that may deploy (comma-separated, * allowed; empty for any)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(N.muted)
                TextField("", text: $branches, prompt: Text("main, release/*").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.rose) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
                Button("Save") {
                    var e = env
                    e.branches = branches.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                    Task {
                        do {
                            try await model.setEnvironment(e)
                            dismiss()
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                }
                .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                .keyboardShortcut(.defaultAction)
                .disabled(env.name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(24)
        .frame(width: 420)
        .background(N.sheet)
        .onAppear { branches = env.branches.joined(separator: ", ") }
    }
}
