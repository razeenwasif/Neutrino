import NeutrinoKit
import SwiftUI

struct PhoneSettings: View {
    @Environment(PhoneModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var confirmUnpair = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                if let link = model.link {
                    Section("Server") {
                        LabeledContent("Name", value: link.name)
                        LabeledContent("Fingerprint", value: link.shortFingerprint).font(.body.monospaced())
                        LabeledContent("Approvals", value: model.signsApprovals ? "Signed with Face ID" : "Not signed")
                    }
                }
                Section {
                    Toggle("Deploys waiting for approval", isOn: $model.notifyApprovals)
                    Toggle("Failed runs", isOn: $model.notifyFailures)
                    if let e = model.pushError { Text(e).font(.footnote).foregroundStyle(N.danger) }
                } header: {
                    Text("Notifications")
                } footer: {
                    Text("Apple's push service carries only an id and \"Something needs you\"; this phone fetches the real text from your server. Your server needs an APNs key (`ntr notify setup`).")
                }
                Section {
                    Button("Unpair", role: .destructive) { confirmUnpair = true }
                } footer: {
                    Text("To cut this phone off from the server itself, revoke it there with `ntr devices revoke`.")
                }
            }
            .navigationTitle("Settings")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .confirmationDialog("Unpair from \(model.link?.name ?? "the server")?", isPresented: $confirmUnpair, titleVisibility: .visible) {
                Button("Unpair", role: .destructive) {
                    model.unpair()
                    dismiss()
                }
            }
        }
    }
}
