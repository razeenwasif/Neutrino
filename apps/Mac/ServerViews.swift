import NeutrinoKit
import SwiftUI

// MARK: - Pairing

/// Pair this Mac with the server: pick it (Bonjour) or type its address, check
/// the certificate's fingerprint against `ntr pair`, then enter the code.
struct PairSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var found: [ServerBrowser.Found] = []
    @State private var browser = ServerBrowser()
    @State private var serviceName: String?
    @State private var address = ""
    @State private var port = String(ServerLink.defaultPort)
    @State private var fingerprint: String?
    @State private var code = ""
    @State private var sshHost = ""
    @State private var working = false
    @State private var error: String?

    private var target: ServerTarget? {
        if let serviceName { return ServerTarget(name: serviceName, serviceName: serviceName) }
        let a = address.trimmingCharacters(in: .whitespaces)
        guard !a.isEmpty, let p = UInt16(port) else { return nil }
        return ServerTarget(name: a, address: a, port: p)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Pair with server")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Text("On the server, run `ntr pair`. It prints a one-time code and the server's fingerprint.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if fingerprint == nil {
                chooseServer
            } else {
                confirmAndPair
            }

            if let error {
                HStack(spacing: 8) {
                    StatusDot(color: N.rose, size: 5)
                    Text(error)
                        .font(.system(size: 12))
                        .foregroundStyle(N.rose)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(N.rose.opacity(0.08), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

            HStack(spacing: 8) {
                if fingerprint != nil {
                    Button("Back") { fingerprint = nil; error = nil }
                        .buttonStyle(PillButtonStyle(kind: .quiet, height: 30))
                }
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
                if fingerprint == nil {
                    Button(working ? "Connecting…" : "Connect") { connect() }
                        .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                        .disabled(target == nil || working)
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button(working ? "Pairing…" : "Pair") { pair() }
                        .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                        .disabled(code.filter(\.isLetter).count + code.filter(\.isNumber).count < 8 || working)
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(24)
        .frame(width: 448)
        .background(N.sheet)
        .onAppear {
            browser.onChange = { found = $0 }
            browser.start()
        }
        .onDisappear { browser.stop() }
    }

    private var chooseServer: some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel("On this network")
            if found.isEmpty {
                Text("Looking for servers… none found yet.")
                    .font(.system(size: 12))
                    .foregroundStyle(N.faint)
                    .padding(.vertical, 4)
            }
            ForEach(found) { f in
                Button {
                    serviceName = serviceName == f.name ? nil : f.name
                } label: {
                    HStack(spacing: 10) {
                        StatusDot(color: N.violet, size: 6, pulse: serviceName == f.name)
                        Text(f.name)
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(N.text)
                        Spacer()
                        if serviceName == f.name {
                            Image(systemName: "checkmark")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(N.indigo)
                        }
                    }
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(serviceName == f.name ? N.surfaceActive : N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                .buttonStyle(.plain)
            }
            SectionLabel("Or by address").padding(.top, 6)
            HStack(spacing: 8) {
                field("192.0.2.20 or server.local", text: $address)
                    .onChange(of: address) { if !address.isEmpty { serviceName = nil } }
                field("Port", text: $port).frame(width: 84)
            }
        }
    }

    private var confirmAndPair: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Check this matches the fingerprint `ntr pair` printed:")
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.body)
                Text(ServerLink.short(fingerprint ?? ""))
                    .font(N.mono(18, weight: .semibold))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(maxWidth: .infinity, minHeight: 42, alignment: .leading)
                    .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                Text("If it doesn't, something is between you and the server. Don't pair.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(N.faint)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Pairing code").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                field("ABCD-EFGH", text: $code).font(N.mono(14))
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("SSH host for pushing").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                field("build-server", text: $sshHost)
                Text("Pushes go to ssh://\(sshHost.isEmpty ? "<host>" : sshHost)/… with your own SSH keys. Leave it empty if the server is this Mac.")
                    .font(.system(size: 11))
                    .foregroundStyle(N.faint)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func field(_ placeholder: String, text: Binding<String>) -> some View {
        TextField("", text: text, prompt: Text(placeholder).foregroundStyle(N.faint))
            .textFieldStyle(.plain)
            .font(.system(size: 13))
            .foregroundStyle(N.text)
            .padding(.horizontal, 12)
            .frame(height: 34)
            .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private func connect() {
        guard let target else { return }
        working = true
        error = nil
        Task {
            do {
                fingerprint = try await Pairing.fingerprint(of: target)
            } catch {
                self.error = error.localizedDescription
            }
            working = false
        }
    }

    private func pair() {
        guard let target, let fingerprint else { return }
        working = true
        error = nil
        Task {
            do {
                let link = try await Pairing.pair(target, fingerprint: fingerprint, code: code, device: Host.current().localizedName ?? "Mac", sshHost: sshHost)
                await model.paired(link)
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
            working = false
        }
    }
}

// MARK: - Publishing

/// Host the selected working copy on the server and push it there.
struct PublishSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var description = ""
    @State private var working = false
    @State private var error: String?
    @State private var remote = "origin"

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Publish to \(model.serverLink?.name ?? "the server")")
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(N.text)
                Text(remote == "origin"
                     ? "Creates a repository on the server, makes it this one's origin, and pushes \(branch) to it."
                     : "Creates a repository on the server, adds it to this one as the remote \(remote) (origin stays as it is), and pushes \(branch) to it.")
                    .font(.system(size: 12.5))
                    .foregroundStyle(N.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: 5) {
                Text("Name on the server").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                TextField("", text: $name, prompt: Text("app").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(N.mono(13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

            VStack(alignment: .leading, spacing: 5) {
                Text("Description (optional)").font(.system(size: 11, weight: .medium)).foregroundStyle(N.muted)
                TextField("", text: $description, prompt: Text("What it is").foregroundStyle(N.faint))
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 34)
                    .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

            if let link = model.serverLink {
                Text(link.sshHost.isEmpty ? "Pushes over the local filesystem." : "Pushes over SSH through the host \(link.sshHost) in your SSH config, with your own keys.")
                    .font(.system(size: 11.5))
                    .foregroundStyle(N.faint)
            }

            if let error {
                Text(error)
                    .font(N.mono(11.5))
                    .foregroundStyle(N.rose)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(N.rose.opacity(0.08), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

            HStack(spacing: 8) {
                Spacer()
                Button("Cancel") { dismiss() }
                    .buttonStyle(PillButtonStyle(kind: .secondary, height: 30))
                    .keyboardShortcut(.cancelAction)
                Button(working ? "Publishing…" : "Publish") { publish() }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || working)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 448)
        .background(N.sheet)
        .onAppear { name = model.repo?.name.replacingOccurrences(of: " ", with: "-").lowercased() ?? "" }
        .task { remote = await model.publishRemoteName() }
    }

    private var branch: String {
        if case let .branch(n, _)? = model.repo?.head { return n }
        return "the current branch"
    }

    private func publish() {
        working = true
        error = nil
        Task {
            do {
                _ = try await model.publish(name: name.trimmingCharacters(in: .whitespaces), description: description)
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
            working = false
        }
    }
}

// MARK: - Sidebar section

struct ServerSection: View {
    @Environment(AppModel.self) private var model
    @State private var pairing = false
    @State private var publishing = false
    /// The selected working copy already pushes to the server: nothing to publish.
    @State private var alreadyHosted = false
    @State private var pairHovered = false
    @State private var publishHovered = false

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                SectionLabel("On \(model.serverLink?.name ?? "server")")
                Spacer()
                if model.serverLink != nil {
                    Menu {
                        if model.source == .local, model.repo != nil, !alreadyHosted {
                            Button("Publish \(model.repo?.name ?? "this repo")…") { publishing = true }.disabled(!model.isServerOnline || !model.isLive)
                        }
                        Button("Reconnect") { Task { await model.connectServer() } }
                        Divider()
                        Button("Forget this server") { model.forgetServer() }
                    } label: {
                        Image(systemName: "ellipsis")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(N.faint)
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .fixedSize()
                    .accessibilityLabel("Server actions")
                }
            }
            .padding(.horizontal, 10)
            .padding(.top, 10)
            .padding(.bottom, 2)

            switch model.serverState {
            case .unpaired:
                Button {
                    pairing = true
                } label: {
                    Label("Pair with server…", systemImage: "link")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(pairHovered ? N.text : N.muted)
                        .padding(.horizontal, 10)
                        .frame(maxWidth: .infinity, minHeight: 30, alignment: .leading)
                        .background(pairHovered ? N.surfaceHover : Color.clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
                .buttonStyle(.plain)
                .onHover { pairHovered = $0 }
                .help("Pair this Mac with neutrinod --server, to host repositories there.")
            case .connecting:
                Text("Connecting…")
                    .font(.system(size: 12))
                    .foregroundStyle(N.faint)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
            case let .offline(why):
                Text("Offline")
                    .font(.system(size: 12))
                    .foregroundStyle(N.amber)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .help(why)
            case .online:
                if model.hostedRepos.isEmpty {
                    Text("No repositories yet.")
                        .font(.system(size: 12))
                        .foregroundStyle(N.faint)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                }
                let shown = model.hostedRepos.filter(model.matchesFilter)
                if shown.isEmpty, !model.hostedRepos.isEmpty {
                    Text("None match.")
                        .font(.system(size: 12))
                        .foregroundStyle(N.faint)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 4)
                }
                VStack(spacing: 2) {
                    ForEach(shown) { r in
                        HostedRow(repo: r)
                    }
                }
                if model.source == .local, model.repo != nil, model.isLive, !alreadyHosted {
                    Button {
                        publishing = true
                    } label: {
                        Label("Publish \(model.repo?.name ?? "")…", systemImage: "arrow.up.circle")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(publishHovered ? N.text : N.muted)
                            .lineLimit(1)
                            .padding(.horizontal, 10)
                            .frame(maxWidth: .infinity, minHeight: 30, alignment: .leading)
                            .background(publishHovered ? N.surfaceHover : Color.clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .onHover { publishHovered = $0 }
                }
            }
        }
        .sheet(isPresented: $pairing) { PairSheet() }
        .sheet(isPresented: $publishing) { PublishSheet() }
        .task(id: "\(model.source)/\(model.repoID ?? "")/\(model.hostedRepos.count)/\(publishing)") {
            alreadyHosted = await model.selectedIsHosted()
        }
    }
}

struct HostedRow: View {
    @Environment(AppModel.self) private var model
    var repo: Repo
    @State private var hovering = false
    @State private var deleting = false

    var body: some View {
        let selected = model.source == .server && model.repoID == repo.id
        Button {
            Task { await model.selectHosted(repo.id) }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "server.rack")
                    .font(.system(size: 11))
                    .foregroundStyle(selected ? N.violet : N.faint)
                    .frame(width: 16)
                Text(repo.name)
                    .font(.system(size: 12.5, weight: selected ? .medium : .regular))
                    .foregroundStyle(selected ? N.text : N.body)
                    .lineLimit(1)
                Spacer()
                Text(repo.head?.sha == nil ? "empty" : repo.head?.label ?? "")
                    .font(N.mono(10.5))
                    .foregroundStyle(N.faint)
                    .lineLimit(1)
                if selected {
                    Circle()
                        .fill(N.violet)
                        .frame(width: 5, height: 5)
                }
            }
            .padding(.horizontal, 10)
            .frame(height: 29)
            .background(
                selected ? N.surfaceActive : (hovering ? N.surfaceHover : Color.clear),
                in: RoundedRectangle(cornerRadius: 8, style: .continuous)
            )
            .contentShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(model.serverLink.map { $0.pushURL(for: repo.path) } ?? repo.path)
        .contextMenu {
            if let link = model.serverLink {
                Button("Copy push URL") { model.copy(link.pushURL(for: repo.path), label: "push URL") }
                Divider()
                Button("Delete from \(link.name)…", role: .destructive) { deleting = true }
                    .disabled(!model.isServerOnline)
            }
        }
        .confirmationDialog("Stop hosting \(repo.name) on \(model.serverLink?.name ?? "the server")?", isPresented: $deleting) {
            Button("Delete \(repo.name)", role: .destructive) {
                Task {
                    do { try await model.deleteHosted(repo.id) } catch { model.notice = error.localizedDescription }
                }
            }
        } message: {
            Text("Working copies whose origin is this repo stop being able to fetch and push, and its GitHub mirror stops being updated. Nothing is erased: it moves to ~/.neutrino/deleted/ on \(model.serverLink?.name ?? "the server"), from where it can be moved back.")
        }
    }
}
