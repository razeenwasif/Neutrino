import NeutrinoKit
import SwiftUI

/// The one surface: a rounded island lit by a faint top sheen, floating on the ground.
struct IslandModifier: ViewModifier {
    var radius: CGFloat = 16

    func body(content: Content) -> some View {
        content
            .background(N.island, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: radius, style: .continuous)
                    .strokeBorder(LinearGradient(colors: [N.sheen, .clear], startPoint: .top, endPoint: .center), lineWidth: 1)
                    .allowsHitTesting(false)
            }
            .clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
            .shadow(color: .black.opacity(0.45), radius: 18, y: 12)
    }
}

/// A recessed well for code, logs and fields.
struct WellModifier: ViewModifier {
    var radius: CGFloat = 12

    func body(content: Content) -> some View {
        content
            .background(N.well, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: radius, style: .continuous)
                    .strokeBorder(Color.black.opacity(0.35), lineWidth: 1)
                    .blur(radius: 1.5)
                    .allowsHitTesting(false)
            }
    }
}

extension View {
    func island(radius: CGFloat = 16) -> some View { modifier(IslandModifier(radius: radius)) }
    func well(radius: CGFloat = 12) -> some View { modifier(WellModifier(radius: radius)) }
}

struct SectionLabel: View {
    var text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 10.5, weight: .semibold))
            .tracking(0.8)
            .foregroundStyle(N.faint)
    }
}

/// Every control is a pill.
struct PillButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, quiet }
    var kind: Kind = .secondary
    var height: CGFloat = 28

    func makeBody(configuration: Configuration) -> some View {
        let (bg, fg): (Color, Color) = switch kind {
        case .primary: (N.accent, N.onAccent)
        case .secondary: (N.control, N.body)
        case .quiet: (.clear, N.body)
        }
        return configuration.label
            .font(.system(size: 12, weight: kind == .primary ? .semibold : .medium))
            .foregroundStyle(fg)
            .padding(.horizontal, 12)
            .frame(height: height)
            .background(bg, in: Capsule())
            .brightness(configuration.isPressed ? 0.08 : 0)
            .contentShape(Capsule())
    }
}

/// A two- or three-way choice in a recessed track.
struct Segmented<Value: Hashable>: View {
    var options: [(Value, String)]
    @Binding var selection: Value

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { value, label in
                Button {
                    selection = value
                } label: {
                    Text(label)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(selection == value ? N.text : N.muted)
                        .padding(.horizontal, 11)
                        .frame(height: 24)
                        .background(selection == value ? N.selected : .clear, in: Capsule())
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == value ? .isSelected : [])
            }
        }
        .padding(3)
        .background(N.well, in: Capsule())
    }
}

struct RefChip: View {
    var label: RefLabel

    var body: some View {
        let (bg, fg): (Color, Color) = switch label.kind {
        case .local: (N.accent.opacity(0.14), N.accent)
        case .remote: (N.second.opacity(0.14), N.second)
        case .tag: (N.warn.opacity(0.13), N.warn)
        case .head: (Color.white.opacity(0.08), N.text)
        }
        Text(label.name)
            .font(N.mono(11, weight: .medium))
            .foregroundStyle(fg)
            .lineLimit(1)
            .padding(.horizontal, 8)
            .frame(height: 19)
            .background(bg, in: Capsule())
            .overlay {
                if label.isHead { Capsule().strokeBorder(fg.opacity(0.5), lineWidth: 1) }
            }
            .help(label.isHead ? "\(label.name), checked out" : label.name)
    }
}

struct Dot: View {
    var color: Color
    var glow = true
    var size: CGFloat = 7

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .shadow(color: glow ? color.opacity(0.8) : .clear, radius: 4)
    }
}

/// A centred message for an empty or failed panel.
struct EmptyState: View {
    var title: String
    var message: String
    var action: (String, () -> Void)? = nil

    var body: some View {
        VStack(spacing: 8) {
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(N.text)
            Text(message)
                .font(.system(size: 13))
                .foregroundStyle(N.muted)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 360)
            if let action {
                Button(action.0, action: action.1).buttonStyle(PillButtonStyle(kind: .primary, height: 30)).padding(.top, 6)
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct StatCounts: View {
    var additions: UInt32
    var deletions: UInt32

    var body: some View {
        HStack(spacing: 6) {
            Text("+\(additions)").foregroundStyle(N.ok)
            Text("−\(deletions)").foregroundStyle(N.danger)
        }
        .font(N.mono(11, weight: .medium))
    }
}
