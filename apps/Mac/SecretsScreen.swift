import NeutrinoKit
import SwiftUI

struct SecretsScreen: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if let problem = model.pullRepoProblem, model.vaultInfo == nil {
                EmptyState(title: "No secrets here", message: problem).island()
            } else {
                VStack(spacing: 10) {
                    ScopeBar()
                    if model.vaultInfo?.setUp == false {
                        VaultNotSetUp()
                    }
                    HStack(alignment: .top, spacing: 10) {
                        SecretList().frame(maxWidth: .infinity, maxHeight: .infinity)
                        VariableList().frame(maxWidth: .infinity, maxHeight: .infinity)
                        if model.pullRepo != nil {
                            EnvironmentList().frame(width: 300).frame(maxHeight: .infinity)
                        }
                    }
                }
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
        HStack(spacing: 10) {
            Text("Secrets and variables").font(.system(size: 14, weight: .semibold)).foregroundStyle(N.text)
            Spacer()
            if let repo = model.pullRepo {
                var options: [(VaultView, String)] = [(.repo, repo), (.everyRepo, "Every repository")]
                let _ = options.append(contentsOf: model.environmentList.map { (VaultView.environment($0.name), "\(repo) · \($0.name)") })
                Segmented(options: options, selection: $model.vaultView)
            } else {
                Text("Every repository").font(.system(size: 12)).foregroundStyle(N.muted)
            }
        }
        .padding(.horizontal, 14)
        .frame(height: 46)
        .island()
    }
}

struct VaultNotSetUp: View {
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "lock.trianglebadge.exclamationmark").foregroundStyle(N.warn).font(.system(size: 15))
            VStack(alignment: .leading, spacing: 3) {
                Text("The vault isn't set up yet").font(.system(size: 13, weight: .semibold)).foregroundStyle(N.text)
                Text("On the server, run `ntr vault setup`. It creates the key that seals secrets and shows its recovery code once; keep that code somewhere away from the server. Variables work without it.")
                    .font(.system(size: 12)).foregroundStyle(N.muted).fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
        }
        .padding(14)
        .island()
    }
}

private struct PanelHeader: View {
    var title: String
    var detail: String
    var action: (String, Bool, () -> Void)?

    var body: some View {
        HStack(spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13.5, weight: .semibold)).foregroundStyle(N.text)
                Text(detail).font(.system(size: 11)).foregroundStyle(N.faint)
            }
            Spacer()
            if let (label, enabled, act) = action {
                Button(label, action: act).buttonStyle(PillButtonStyle(kind: .secondary, height: 26)).disabled(!enabled)
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 12)
        .padding(.bottom, 8)
    }
}

/// Where a wider-scoped entry comes from, shown beside it.
private struct Inherited: View {
    var scope: VaultScope

    var body: some View {
        Text(scope.repo == nil ? "every repo" : (scope.environment == nil ? "this repo" : scope.environment!))
            .font(.system(size: 10.5, weight: .medium))
            .foregroundStyle(N.faint)
            .padding(.horizontal, 6)
            .frame(height: 17)
            .background(Color.white.opacity(0.06), in: Capsule())
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
                EmptyState(title: "No secrets", message: "Workflows read them as ${{ secrets.NAME }}; logs show *** in their place.")
            } else {
                ScrollView {
                    VStack(spacing: 2) {
                        ForEach(model.secretList) { s in
                            HStack(spacing: 8) {
                                Image(systemName: "key.fill").font(.system(size: 10)).foregroundStyle(N.accent.opacity(model.isOwn(s.scope) ? 1 : 0.4))
                                // One line, truncated in the middle: long names used to wrap mid-word.
                                Text(s.name).font(N.mono(12.5, weight: .medium)).foregroundStyle(model.isOwn(s.scope) ? N.text : N.muted)
                                    .lineLimit(1).truncationMode(.middle).help(s.name)
                                if !model.isOwn(s.scope) { Inherited(scope: s.scope).fixedSize() }
                                Spacer()
                                Text(Format.ago(s.updatedAt)).font(.system(size: 11)).foregroundStyle(N.faint)
                                if model.isOwn(s.scope) {
                                    Menu {
                                        Button("Replace value…") { editing = s.name }
                                        Button("Delete", role: .destructive) { Task { await model.deleteSecret(s) } }
                                    } label: {
                                        Image(systemName: "ellipsis").font(.system(size: 11, weight: .semibold)).foregroundStyle(N.faint)
                                    }
                                    .menuStyle(.button).buttonStyle(.plain).fixedSize()
                                    .accessibilityLabel("Actions for \(s.name)")
                                }
                            }
                            .padding(.horizontal, 10)
                            .frame(height: 34)
                            .background(N.raised.opacity(0.5), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
                }
            }
        }
        .island()
        .sheet(isPresented: $adding) { SecretSheet(name: "") }
        .sheet(item: Binding(get: { editing.map { IdentifiedName(name: $0) } }, set: { editing = $0?.name })) { SecretSheet(name: $0.name) }
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
    @State private var error: String?
    @State private var working = false

    var body: some View {
        let replacing = model.secretList.contains { $0.name == name && model.isOwn($0.scope) }
        VStack(alignment: .leading, spacing: 14) {
            Text(replacing ? "Replace \(name)" : "New secret").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            Text("For \(model.vaultScope.label).").font(.system(size: 12)).foregroundStyle(N.muted)
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("DEPLOY_TOKEN", text: $name)
                    .textFieldStyle(.plain).font(N.mono(13)).padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .disabled(replacing)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Value").font(.system(size: 11)).foregroundStyle(N.muted)
                SecureField("Pasted, not shown", text: $value)
                    .textFieldStyle(.plain).font(N.mono(13)).padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                Text("It's sealed on the server and can't be read back, here or anywhere.").font(.system(size: 11)).foregroundStyle(N.faint)
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.danger).fixedSize(horizontal: false, vertical: true) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button(working ? "Saving…" : "Save") {
                    working = true
                    Task {
                        defer { working = false }
                        do {
                            try await model.setSecret(name: name.trimmingCharacters(in: .whitespaces), value: value)
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
        .padding(20)
        .frame(width: 440)
        .background(N.island)
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
                EmptyState(title: "No variables", message: "Workflows read them as ${{ vars.NAME }}.")
            } else {
                ScrollView {
                    VStack(spacing: 2) {
                        ForEach(model.variableList) { v in
                            HStack(spacing: 8) {
                                // The name keeps one line and wins the space; the value truncates.
                                Text(v.name).font(N.mono(12.5, weight: .medium)).foregroundStyle(model.isOwn(v.scope) ? N.text : N.muted)
                                    .lineLimit(1).truncationMode(.middle).layoutPriority(1).help(v.name)
                                if !model.isOwn(v.scope) { Inherited(scope: v.scope).fixedSize() }
                                Text(v.value).font(N.mono(12)).foregroundStyle(N.body).lineLimit(1).truncationMode(.tail).textSelection(.enabled).help(v.value)
                                Spacer()
                                if model.isOwn(v.scope) {
                                    Menu {
                                        Button("Edit…") { editing = v }
                                        Button("Delete", role: .destructive) { Task { await model.deleteVariable(v) } }
                                    } label: {
                                        Image(systemName: "ellipsis").font(.system(size: 11, weight: .semibold)).foregroundStyle(N.faint)
                                    }
                                    .menuStyle(.button).buttonStyle(.plain).fixedSize()
                                    .accessibilityLabel("Actions for \(v.name)")
                                }
                            }
                            .padding(.horizontal, 10)
                            .frame(height: 34)
                            .background(N.raised.opacity(0.5), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
                }
            }
        }
        .island()
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
        VStack(alignment: .leading, spacing: 14) {
            Text(editing ? "Edit \(name)" : "New variable").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            Text("For \(model.vaultScope.label).").font(.system(size: 12)).foregroundStyle(N.muted)
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("REGION", text: $name)
                    .textFieldStyle(.plain).font(N.mono(13)).padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .disabled(editing)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Value").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("eu-west", text: $value)
                    .textFieldStyle(.plain).font(N.mono(13)).padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.danger) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
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
        .padding(20)
        .frame(width: 420)
        .background(N.island)
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
                EmptyState(title: "No environments", message: "A job with environment: production gets its secrets, and waits for you if it needs approval.")
            } else {
                ScrollView {
                    VStack(spacing: 4) {
                        ForEach(model.environmentList) { e in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack {
                                    Text(e.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(N.text)
                                    Spacer()
                                    Menu {
                                        Button("Edit…") { editing = e }
                                        Button("Delete", role: .destructive) { Task { await model.deleteEnvironment(e) } }
                                    } label: {
                                        Image(systemName: "ellipsis").font(.system(size: 11, weight: .semibold)).foregroundStyle(N.faint)
                                    }
                                    .menuStyle(.button).buttonStyle(.plain).fixedSize()
                                    .accessibilityLabel("Actions for \(e.name)")
                                }
                                Label(e.requireApproval ? "Needs approval" : "No approval", systemImage: e.requireApproval ? "hand.raised.fill" : "hand.raised.slash")
                                    .font(.system(size: 11.5)).foregroundStyle(e.requireApproval ? N.warn : N.faint)
                                Text(e.branches.isEmpty ? "Any branch" : e.branches.joined(separator: ", ")).font(N.mono(11)).foregroundStyle(N.muted)
                            }
                            .padding(10)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(N.raised.opacity(0.5), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        }
                    }
                    .padding(.horizontal, 10)
                    .padding(.bottom, 10)
                }
            }
        }
        .island()
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
        VStack(alignment: .leading, spacing: 14) {
            Text(editing ? "Edit \(env.name)" : "New environment").font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            Text("For \(model.repo?.name ?? "this repository").").font(.system(size: 12)).foregroundStyle(N.muted)
            VStack(alignment: .leading, spacing: 5) {
                Text("Name").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("production", text: $env.name)
                    .textFieldStyle(.plain).font(N.mono(13)).padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .disabled(editing)
            }
            Toggle("Jobs wait for approval", isOn: $env.requireApproval).toggleStyle(.checkbox).font(.system(size: 12.5))
            VStack(alignment: .leading, spacing: 5) {
                Text("Branches that may deploy (comma-separated, * allowed; empty for any)").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("main, release/*", text: $branches)
                    .textFieldStyle(.plain).font(N.mono(13)).padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            if let error { Text(error).font(.system(size: 12)).foregroundStyle(N.danger) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
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
        .padding(20)
        .frame(width: 420)
        .background(N.island)
        .onAppear { branches = env.branches.joined(separator: ", ") }
    }
}
