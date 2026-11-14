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
    @State private var sshHost = "mini"
    @State private var working = false
    @State private var error: String?

    private var target: ServerTarget? {
        if let serviceName { return ServerTarget(name: serviceName, serviceName: serviceName) }
        let a = address.trimmingCharacters(in: .whitespaces)
        guard !a.isEmpty, let p = UInt16(port) else { return nil }
        return ServerTarget(name: a, address: a, port: p)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Pair with atlas").font(.system(size: 16, weight: .semibold)).foregroundStyle(N.text)
            Text("On the server, run `ntr pair`. It prints a one-time code and the server's fingerprint.")
                .font(.system(size: 12.5)).foregroundStyle(N.muted).fixedSize(horizontal: false, vertical: true)

            if fingerprint == nil {
                chooseServer
            } else {
                confirmAndPair
            }

            if let error {
                Text(error).font(.system(size: 12)).foregroundStyle(N.danger).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                if fingerprint != nil {
                    Button("Back") { fingerprint = nil; error = nil }.buttonStyle(PillButtonStyle(kind: .quiet, height: 30))
                }
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
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
        .padding(22)
        .frame(width: 440)
        .background(N.island)
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
                Text("Looking for servers… none found yet.").font(.system(size: 12)).foregroundStyle(N.faint)
            }
            ForEach(found) { f in
                Button {
                    serviceName = serviceName == f.name ? nil : f.name
                } label: {
                    HStack {
                        Dot(color: N.atlas, glow: serviceName == f.name)
                        Text(f.name).foregroundStyle(N.text)
                        Spacer()
                        if serviceName == f.name { Image(systemName: "checkmark").foregroundStyle(N.accent) }
                    }
                    .padding(.horizontal, 10)
                    .frame(height: 32)
                    .background(serviceName == f.name ? N.selected : N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                }
                .buttonStyle(.plain)
            }
            SectionLabel("Or by address").padding(.top, 6)
            HStack(spacing: 8) {
                field("192.168.1.20 or atlas.local", text: $address).onChange(of: address) { if !address.isEmpty { serviceName = nil } }
                field("Port", text: $port).frame(width: 80)
            }
        }
    }

    private var confirmAndPair: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Check this matches the fingerprint `ntr pair` printed:").font(.system(size: 12.5)).foregroundStyle(N.body)
                Text(ServerLink.short(fingerprint ?? ""))
                    .font(N.mono(18, weight: .semibold))
                    .foregroundStyle(N.text)
                    .padding(.horizontal, 12)
                    .frame(height: 40)
                    .well(radius: 10)
                Text("If it doesn't, something is between you and the server. Don't pair.").font(.system(size: 11.5)).foregroundStyle(N.faint)
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Pairing code").font(.system(size: 11)).foregroundStyle(N.muted)
                field("ABCD-EFGH", text: $code).font(N.mono(14))
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("SSH host for pushing").font(.system(size: 11)).foregroundStyle(N.muted)
                field("mini", text: $sshHost)
                Text("Pushes go to ssh://\(sshHost.isEmpty ? "<host>" : sshHost)/… with your own SSH keys. Leave it empty if the server is this Mac.")
                    .font(.system(size: 11)).foregroundStyle(N.faint).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func field(_ placeholder: String, text: Binding<String>) -> some View {
        TextField(placeholder, text: text)
            .textFieldStyle(.plain)
            .font(.system(size: 13))
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
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

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Publish to \(model.serverLink?.name ?? "atlas")").font(.system(size: 16, weight: .semibold)).foregroundStyle(N.text)
            Text("Creates a repository on the server, adds it to this one as the remote atlas, and pushes \(branch) to it.")
                .font(.system(size: 12.5)).foregroundStyle(N.muted).fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 5) {
                Text("Name on the server").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("app", text: $name).textFieldStyle(.plain).font(N.mono(13))
                    .padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            VStack(alignment: .leading, spacing: 5) {
                Text("Description (optional)").font(.system(size: 11)).foregroundStyle(N.muted)
                TextField("What it is", text: $description).textFieldStyle(.plain).font(.system(size: 13))
                    .padding(.horizontal, 10).frame(height: 32)
                    .background(N.raised, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
            if let link = model.serverLink {
                Text("Pushes over \(link.sshHost.isEmpty ? "the local filesystem" : "SSH to \(link.sshHost)"), with your own keys.")
                    .font(.system(size: 11.5)).foregroundStyle(N.faint)
            }
            if let error {
                Text(error).font(N.mono(11.5)).foregroundStyle(N.danger).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.buttonStyle(PillButtonStyle(kind: .secondary, height: 30)).keyboardShortcut(.cancelAction)
                Button(working ? "Publishing…" : "Publish") { publish() }
                    .buttonStyle(PillButtonStyle(kind: .primary, height: 30))
                    .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || working)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(22)
        .frame(width: 440)
        .background(N.island)
        .onAppear { name = model.repo?.name.replacingOccurrences(of: " ", with: "-").lowercased() ?? "" }
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

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                SectionLabel("On \(model.serverLink?.name ?? "atlas")")
                Spacer()
                if model.serverLink != nil {
                    Menu {
                        if model.source == .local, model.repo != nil {
                            Button("Publish \(model.repo?.name ?? "this repo")…") { publishing = true }.disabled(!model.isServerOnline || !model.isLive)
                        }
                        Button("Reconnect") { Task { await model.connectServer() } }
                        Divider()
                        Button("Forget this server") { model.forgetServer() }
                    } label: {
                        Image(systemName: "ellipsis").font(.system(size: 11, weight: .semibold)).foregroundStyle(N.faint)
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .fixedSize()
                    .accessibilityLabel("Server actions")
                }
            }
            .padding(.horizontal, 10)
            .padding(.top, 18)
            .padding(.bottom, 8)

            switch model.serverState {
            case .unpaired:
                Button {
                    pairing = true
                } label: {
                    Label("Pair with atlas…", systemImage: "link").font(.system(size: 12.5)).foregroundStyle(N.muted)
                        .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 32, alignment: .leading).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Pair this Mac with neutrinod --server, to host repositories there.")
            case .connecting:
                Text("Connecting…").font(.system(size: 12)).foregroundStyle(N.faint).padding(.horizontal, 10)
            case let .offline(why):
                Text("Offline").font(.system(size: 12)).foregroundStyle(N.warn).padding(.horizontal, 10).help(why)
            case .online:
                if model.hostedRepos.isEmpty {
                    Text("No repositories yet.").font(.system(size: 12)).foregroundStyle(N.faint).padding(.horizontal, 10)
                }
                ForEach(model.hostedRepos) { r in
                    HostedRow(repo: r)
                }
                if model.source == .local, model.repo != nil, model.isLive {
                    Button {
                        publishing = true
                    } label: {
                        Label("Publish \(model.repo?.name ?? "")…", systemImage: "arrow.up.circle").font(.system(size: 12.5)).foregroundStyle(N.muted)
                            .lineLimit(1)
                            .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 30, alignment: .leading).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
            ForEach([("Runs", "play.circle"), ("Secrets", "key")], id: \.0) { title, icon in
                NavRow(title: title, icon: icon, enabled: false, help: "Arrives with milestones 5 and 6.")
            }
        }
        .sheet(isPresented: $pairing) { PairSheet() }
        .sheet(isPresented: $publishing) { PublishSheet() }
    }
}

struct HostedRow: View {
    @Environment(AppModel.self) private var model
    var repo: Repo
    @State private var hovering = false

    var body: some View {
        let selected = model.source == .server && model.repoID == repo.id
        Button {
            Task { await model.selectHosted(repo.id) }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "server.rack").font(.system(size: 11)).foregroundStyle(selected ? N.accent : N.faint).frame(width: 18)
                Text(repo.name).font(.system(size: 12.5, weight: selected ? .medium : .regular)).foregroundStyle(selected ? N.text : N.body).lineLimit(1)
                Spacer()
                Text(repo.head?.sha == nil ? "empty" : repo.head?.label ?? "").font(N.mono(11)).foregroundStyle(N.faint).lineLimit(1)
            }
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(selected ? N.selected : (hovering ? Color.white.opacity(0.04) : .clear), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(model.serverLink.map { $0.pushURL(for: repo.path) } ?? repo.path)
        .contextMenu {
            if let link = model.serverLink {
                Button("Copy push URL") { model.copy(link.pushURL(for: repo.path)) }
            }
        }
    }
}
