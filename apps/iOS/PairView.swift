import NeutrinoKit
import SwiftUI

/// Pair with the server: find it (or type its address), check the
/// fingerprint against `ntr pair`, enter the code.
struct PairView: View {
    @Environment(PhoneModel.self) private var model
    @State private var browser = ServerBrowser()
    @State private var found: [ServerBrowser.Found] = []
    @State private var address = ""
    @State private var target: ServerTarget?
    @State private var fingerprint: String?
    @State private var code = ""
    @State private var working = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Pair with your server").font(.title2.weight(.semibold))
                        Text("On the server, run `ntr pair`. It shows a code and the fingerprint to check.").font(.subheadline).foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 6)
                }
                if fingerprint == nil {
                    Section("On this network") {
                        if found.isEmpty {
                            HStack(spacing: 8) {
                                ProgressView()
                                Text("Looking for servers…").foregroundStyle(.secondary)
                            }
                        }
                        ForEach(found) { f in
                            Button(f.name) { check(ServerTarget(name: f.name, serviceName: f.name)) }
                        }
                    }
                    Section("Or by address") {
                        TextField("192.168.1.20 or atlas.tailnet.ts.net", text: $address)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .keyboardType(.URL)
                            .submitLabel(.go)
                            .onSubmit { if !address.trimmingCharacters(in: .whitespaces).isEmpty { check(ServerTarget(name: address, address: address.trimmingCharacters(in: .whitespaces))) } }
                        Button("Connect") { check(ServerTarget(name: address, address: address.trimmingCharacters(in: .whitespaces))) }
                            .disabled(address.trimmingCharacters(in: .whitespaces).isEmpty || working)
                    }
                } else if let target, let fingerprint {
                    Section("Check the fingerprint") {
                        Text(ServerLink.short(fingerprint)).font(.system(.title3, design: .monospaced).weight(.semibold))
                        Text("It should match what `ntr pair` shows on \(target.name). If it doesn't, stop: something else is answering.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    Section("Pairing code") {
                        TextField("ABCD-EFGH", text: $code)
                            .textInputAutocapitalization(.characters)
                            .autocorrectionDisabled()
                            .font(.system(.title3, design: .monospaced))
                            .submitLabel(.go)
                            .onSubmit { if code.count >= 8 { pair(target, fingerprint) } }
                        Button(working ? "Pairing…" : "Pair") { pair(target, fingerprint) }
                            .disabled(code.count < 8 || working)
                        Button("Back", role: .cancel) {
                            self.fingerprint = nil
                            self.target = nil
                        }
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(N.danger) }
                }
            }
            .navigationTitle("Neutrino")
            .scrollDismissesKeyboard(.interactively)
            .onAppear {
                browser.onChange = { found = $0 }
                browser.start()
            }
            .onDisappear { browser.stop() }
        }
    }

    private func check(_ t: ServerTarget) {
        working = true
        error = nil
        Task {
            defer { working = false }
            do {
                fingerprint = try await Pairing.fingerprint(of: t)
                target = t
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func pair(_ t: ServerTarget, _ fp: String) {
        working = true
        error = nil
        Task {
            defer { working = false }
            do {
                try await model.pair(target: t, fingerprint: fp, code: code)
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}
