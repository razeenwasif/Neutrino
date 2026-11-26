import AppKit
import NeutrinoKit
import SwiftUI

// MARK: - Window Chrome

/// Configures the hosting NSWindow for a seamless, borderless pitch-black macOS appearance.
struct WindowAccessor: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        DispatchQueue.main.async {
            if let window = view.window {
                configure(window)
            }
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            if let window = nsView.window {
                configure(window)
            }
        }
    }

    @MainActor
    private func configure(_ window: NSWindow) {
        window.backgroundColor = .black
        window.appearance = NSAppearance(named: .darkAqua)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isMovableByWindowBackground = true
        window.styleMask.insert(.fullSizeContentView)
    }
}

// MARK: - Tonal Surfaces (Zero Borders, Zero Heavy Shadows)

struct TonalSurfaceModifier: ViewModifier {
    var color: Color = N.surfaceSubtle
    var radius: CGFloat = 10

    func body(content: Content) -> some View {
        content
            .background(color, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
            .clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
    }
}

extension View {
    func tonalSurface(_ color: Color = N.surfaceSubtle, radius: CGFloat = 10) -> some View {
        modifier(TonalSurfaceModifier(color: color, radius: radius))
    }

    func island(radius: CGFloat = 12) -> some View {
        modifier(TonalSurfaceModifier(color: N.inspector, radius: radius))
    }

    func well(radius: CGFloat = 10) -> some View {
        modifier(TonalSurfaceModifier(color: N.surfaceSubtle, radius: radius))
    }
}

// MARK: - Typography & Labels

struct SectionLabel: View {
    var text: String

    init(_ text: String) { self.text = text }

    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 10, weight: .semibold))
            .tracking(0.8)
            .foregroundStyle(N.faint)
    }
}

struct MetricStatColumn: View {
    let label: String
    let value: String
    var subtitle: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(label)
                .font(.system(size: 12, weight: .regular))
                .foregroundStyle(N.subtle)
                .lineLimit(1)
            Text(value)
                .font(N.statValue())
                .foregroundStyle(N.text)
                .lineLimit(1)
            if let subtitle {
                Text(subtitle)
                    .font(.system(size: 11, weight: .regular))
                    .foregroundStyle(N.faint)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct KeyValueCard: View {
    let pairs: [(String, String)]

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(pairs, id: \.0) { label, value in
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    Text(label)
                        .font(.system(size: 12))
                        .foregroundStyle(N.faint)
                    Spacer(minLength: 8)
                    Text(value)
                        .font(N.mono(12))
                        .foregroundStyle(N.text)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
            }
        }
        .padding(12)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
    }
}

// MARK: - Indicators & Chips

struct StatusDot: View {
    let color: Color
    var size: CGFloat = 6
    var pulse: Bool = false

    @State private var isAnimating = false

    var body: some View {
        ZStack {
            if pulse {
                Circle()
                    .fill(color.opacity(0.35))
                    .frame(width: size * 2.1, height: size * 2.1)
                    .scaleEffect(isAnimating ? 1.25 : 0.85)
                    .opacity(isAnimating ? 0.2 : 0.7)
                    .animation(.easeInOut(duration: 1.5).repeatForever(autoreverses: true), value: isAnimating)
                    .onAppear { isAnimating = true }
            }
            Circle()
                .fill(color)
                .frame(width: size, height: size)
        }
        .frame(width: size * 2, height: size * 2)
    }
}

/// Legacy alias delegating to Stratus StatusDot.
struct Dot: View {
    var color: Color
    var glow = true
    var size: CGFloat = 7

    var body: some View {
        StatusDot(color: color, size: size, pulse: glow)
    }
}

struct TagPill: View {
    let text: String
    var color: Color? = nil
    var textColor: Color = N.muted
    var background: Color = N.surfacePill
    var monospaced: Bool = false

    var body: some View {
        HStack(spacing: 5) {
            if let color {
                Circle()
                    .fill(color)
                    .frame(width: 5, height: 5)
            }
            Text(text)
                .font(monospaced ? N.mono(11, weight: .medium) : .system(size: 11, weight: .medium))
                .foregroundStyle(textColor)
                .lineLimit(1)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(background, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
    }
}

struct RefChip: View {
    var label: RefLabel

    var body: some View {
        let (bg, fg): (Color, Color) = switch label.kind {
        case .local: (N.indigo.opacity(0.14), N.indigo)
        case .remote: (N.sky.opacity(0.14), N.sky)
        case .tag: (N.amber.opacity(0.14), N.amber)
        case .head: (N.surfaceActive, N.text)
        }
        HStack(spacing: 5) {
            if label.isHead {
                Circle()
                    .fill(fg)
                    .frame(width: 5, height: 5)
            }
            Text(label.name)
                .font(N.mono(11, weight: .medium))
                .foregroundStyle(fg)
                .lineLimit(1)
        }
        .padding(.horizontal, 7)
        .frame(height: 20)
        .background(bg, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
        .help(label.isHead ? "\(label.name), checked out" : label.name)
    }
}

struct StatCounts: View {
    var additions: UInt32
    var deletions: UInt32

    var body: some View {
        HStack(spacing: 6) {
            Text("+\(additions)").foregroundStyle(N.mint)
            Text("−\(deletions)").foregroundStyle(N.rose)
        }
        .font(N.mono(11, weight: .medium).monospacedDigit())
    }
}

// MARK: - Controls

struct PillButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, quiet }
    var kind: Kind = .secondary
    var height: CGFloat = 28

    func makeBody(configuration: Configuration) -> some View {
        Pill(label: configuration.label, kind: kind, height: height, pressed: configuration.isPressed)
    }

    private struct Pill<Label: View>: View {
        var label: Label
        var kind: Kind
        var height: CGFloat
        var pressed: Bool
        @Environment(\.isEnabled) private var enabled
        @State private var hovered = false

        var body: some View {
            let (bg, fg): (Color, Color) = switch kind {
            case .primary: (hovered ? N.indigo.opacity(0.9) : N.indigo, N.onAccent)
            case .secondary: (hovered ? N.surfaceActive : N.surfacePill, N.text)
            case .quiet: (hovered ? N.surfaceHover : .clear, N.muted)
            }
            label
                .font(.system(size: 12, weight: kind == .primary ? .semibold : .medium))
                .foregroundStyle(fg)
                .padding(.horizontal, 11)
                .frame(height: height)
                .background(bg, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .scaleEffect(pressed ? 0.98 : 1.0)
                .opacity(enabled ? 1 : 0.4)
                .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
                .onHover { hovered = $0 }
        }
    }
}

struct ActionPillButton: View {
    let title: String
    var systemImage: String? = nil
    var tint: Color = N.text
    var background: Color = N.surfacePill
    var isLoading: Bool = false
    let action: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if isLoading {
                    ProgressView()
                        .controlSize(.small)
                        .scaleEffect(0.7)
                } else if let systemImage {
                    Image(systemName: systemImage)
                        .font(.system(size: 11, weight: .semibold))
                }
                Text(title)
                    .font(.system(size: 12, weight: .medium))
            }
            .foregroundStyle(tint)
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .background(
                (isHovered ? N.surfaceActive : background),
                in: RoundedRectangle(cornerRadius: 7, style: .continuous)
            )
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(isLoading)
        .onHover { isHovered = $0 }
    }
}

struct FilterPillButton: View {
    let title: String
    let isSelected: Bool
    let action: () -> Void

    @State private var isHovered = false

    var body: some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12, weight: isSelected ? .semibold : .medium))
                .foregroundStyle(isSelected ? N.text : N.muted)
                .padding(.horizontal, 11)
                .padding(.vertical, 5)
                .background(
                    isSelected ? N.surfaceActive : (isHovered ? N.surfaceHover : Color.clear),
                    in: RoundedRectangle(cornerRadius: 7, style: .continuous)
                )
                .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { isHovered = $0 }
    }
}

/// A segmented switch styled with Stratus's borderless tonal pills.
struct Segmented<Value: Hashable>: View {
    var options: [(Value, String)]
    @Binding var selection: Value
    @State private var hoveredValue: Value?

    var body: some View {
        HStack(spacing: 2) {
            ForEach(options, id: \.0) { value, label in
                let isSelected = selection == value
                let isHovered = hoveredValue == value
                Button {
                    selection = value
                } label: {
                    Text(label)
                        .font(.system(size: 12, weight: isSelected ? .semibold : .medium))
                        .foregroundStyle(isSelected ? N.text : N.muted)
                        .padding(.horizontal, 10)
                        .frame(height: 24)
                        .background(
                            isSelected ? N.surfaceActive : (isHovered ? N.surfaceHover : Color.clear),
                            in: RoundedRectangle(cornerRadius: 6, style: .continuous)
                        )
                        .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                }
                .buttonStyle(.plain)
                .onHover { hoveredValue = $0 ? value : nil }
                .accessibilityAddTraits(isSelected ? .isSelected : [])
            }
        }
        .padding(3)
        .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

// MARK: - Search Field

private struct OptionalFocusModifier: ViewModifier {
    var focused: FocusState<Bool>.Binding?

    func body(content: Content) -> some View {
        if let focused {
            content.focused(focused)
        } else {
            content
        }
    }
}

struct BorderlessSearchField: View {
    @Binding var text: String
    var placeholder: String = "Search..."
    var width: CGFloat? = nil
    var focused: FocusState<Bool>.Binding? = nil

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(N.faint)
            TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(N.faint))
                .textFieldStyle(.plain)
                .font(.system(size: 12.5))
                .foregroundStyle(N.text)
                .modifier(OptionalFocusModifier(focused: focused))
                .onKeyPress(.escape) {
                    guard !text.isEmpty else { return .ignored }
                    text = ""
                    return .handled
                }
            if !text.isEmpty {
                Button {
                    text = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11.5))
                        .foregroundStyle(N.faint)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 29)
        .frame(width: width)
        .frame(maxWidth: width == nil ? .infinity : nil)
        .background(N.surfaceHover, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

// MARK: - Segmented Bar & Flow Layout (Stratus Overview Style)

struct BarSegment: Identifiable, Sendable, Hashable {
    let id: String
    let label: String
    let value: Double
    let formattedValue: String
    let color: Color

    init(id: String? = nil, label: String, value: Double, formattedValue: String, color: Color) {
        self.id = id ?? label
        self.label = label
        self.value = value
        self.formattedValue = formattedValue
        self.color = color
    }
}

struct SegmentedBar: View {
    let segments: [BarSegment]

    private var total: Double {
        max(segments.reduce(0) { $0 + max(0, $1.value) }, 0.0001)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            GeometryReader { geo in
                let gapTotal = CGFloat(max(0, segments.count - 1)) * 2
                let availableWidth = max(1, geo.size.width - gapTotal)
                HStack(spacing: 2) {
                    ForEach(segments) { segment in
                        let fraction = max(0, segment.value) / total
                        let w = max(4, availableWidth * fraction)
                        RoundedRectangle(cornerRadius: 3, style: .continuous)
                            .fill(segment.color)
                            .frame(width: w, height: 6)
                            .help("\(segment.label): \(segment.formattedValue)")
                    }
                }
            }
            .frame(height: 6)

            FlowWrapLayout(horizontalSpacing: 14, verticalSpacing: 6) {
                ForEach(segments) { segment in
                    HStack(spacing: 5) {
                        RoundedRectangle(cornerRadius: 2, style: .continuous)
                            .fill(segment.color)
                            .frame(width: 6, height: 6)
                        Text(segment.label)
                            .font(.system(size: 11, weight: .regular))
                            .foregroundStyle(N.muted)
                        Text(segment.formattedValue)
                            .font(N.mono(11, weight: .semibold))
                            .foregroundStyle(N.text)
                    }
                }
            }
        }
    }
}

struct FlowWrapLayout: Layout {
    var horizontalSpacing: CGFloat = 10
    var verticalSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? 600
        var currentX: CGFloat = 0
        var currentY: CGFloat = 0
        var lineHeight: CGFloat = 0

        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if currentX + size.width > maxWidth && currentX > 0 {
                currentX = 0
                currentY += lineHeight + verticalSpacing
                lineHeight = 0
            }
            currentX += size.width + horizontalSpacing
            lineHeight = max(lineHeight, size.height)
        }
        return CGSize(width: maxWidth, height: currentY + lineHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var currentX = bounds.minX
        var currentY = bounds.minY
        var lineHeight: CGFloat = 0

        for view in subviews {
            let size = view.sizeThatFits(.unspecified)
            if currentX + size.width > bounds.maxX && currentX > bounds.minX {
                currentX = bounds.minX
                currentY += lineHeight + verticalSpacing
                lineHeight = 0
            }
            view.place(at: CGPoint(x: currentX, y: currentY), proposal: ProposedViewSize(size))
            currentX += size.width + horizontalSpacing
            lineHeight = max(lineHeight, size.height)
        }
    }
}

// MARK: - Empty States & Toasts

struct EmptyState: View {
    var icon: String? = nil
    var title: String
    var message: String
    var action: (String, () -> Void)? = nil

    var body: some View {
        VStack(spacing: 12) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: 28, weight: .light))
                    .foregroundStyle(N.faint)
            }
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(N.text)
            Text(message)
                .font(.system(size: 13))
                .foregroundStyle(N.muted)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 360)
            if let action {
                ActionPillButton(title: action.0, tint: N.indigo, action: action.1)
                    .padding(.top, 4)
            }
        }
        .padding(36)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct ToastBanner: View {
    let message: String
    let isError: Bool
    var onDismiss: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 8) {
            StatusDot(color: isError ? N.rose : N.mint, size: 6)
            Text(message)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(N.text)
                .lineLimit(2)
                .truncationMode(.middle)
            if let onDismiss {
                Button(action: onDismiss) {
                    Image(systemName: "xmark")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(N.muted)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Dismiss")
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .frame(maxWidth: 460)
        .background(N.palette, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .shadow(color: .black.opacity(0.65), radius: 16, y: 6)
    }
}
