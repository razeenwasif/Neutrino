import SwiftUI

/// Design tokens from the Neutrino prototype: Velvet surfaces (near-black with
/// a violet cast, depth from tone and soft light rather than outlines), one
/// lavender accent, pastel status colours that only ever mean status.
public enum N {
    // Surfaces, back to front.
    public static let ground = Color(hex: 0x0B0A10)
    public static let island = Color(hex: 0x15131C)
    public static let well = Color(hex: 0x0F0E15)
    public static let raised = Color(hex: 0x1D1A26)
    public static let control = Color(hex: 0x221F2C)
    public static let selected = Color(hex: 0x262231)
    public static let sheen = Color.white.opacity(0.05)
    public static let hairline = Color.white.opacity(0.05)

    // Text.
    public static let text = Color(hex: 0xEEEAF6)
    public static let body = Color(hex: 0xD6D0E0)
    public static let muted = Color(hex: 0xA39CB4)
    public static let faint = Color(hex: 0x8E879E)

    // Accent and status.
    public static let accent = Color(hex: 0xB9A6F5)
    public static let onAccent = Color(hex: 0x120E1C)
    /// Second marker colour, for things that must differ from the accent (remote refs).
    public static let second = Color(hex: 0x8EC5FF)
    public static let ok = Color(hex: 0x8FD8B5)
    public static let warn = Color(hex: 0xEAC58A)
    public static let danger = Color(hex: 0xEE9A9A)
    /// Machine colours shared with Nexus.
    public static let atlas = Color(hex: 0xA99AF5)
    public static let forge = Color(hex: 0xE9A15B)

    /// Colours for graph lanes, picked by the lane's colour index.
    public static let lanes: [Color] = [
        Color(hex: 0xB9A6F5), Color(hex: 0x8EC5FF), Color(hex: 0x7FD3D0), Color(hex: 0xE8A5C6),
        Color(hex: 0xD9C08F), Color(hex: 0x9BD9B0), Color(hex: 0xE9A15B), Color(hex: 0xA8B4F0),
    ]

    public static func lane(_ index: UInt16) -> Color { lanes[Int(index) % lanes.count] }

    public static func mono(_ size: CGFloat, weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced)
    }

    public static func status(_ s: ChangeStatus) -> Color {
        switch s {
        case .added: ok
        case .deleted: danger
        default: warn
        }
    }
}

public extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255, blue: Double(hex & 0xFF) / 255, opacity: opacity)
    }
}

public enum Format {
    /// "just now", "12 min ago", "3 h ago", "yesterday", "5 d ago", then a date.
    public static func ago(_ seconds: Int64, now: Date = Date()) -> String {
        let s = max(0, Int64(now.timeIntervalSince1970) - seconds)
        switch s {
        case ..<60: return "just now"
        case ..<3600: return "\(s / 60) min ago"
        case ..<86_400: return "\(s / 3600) h ago"
        case ..<172_800: return "yesterday"
        case ..<(30 * 86_400): return "\(s / 86_400) d ago"
        default: return date(seconds)
        }
    }

    public static func date(_ seconds: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(seconds)).formatted(.dateTime.day().month(.abbreviated).year())
    }

    /// The full local time, for tooltips and detail panels.
    public static func full(_ seconds: Int64) -> String {
        Date(timeIntervalSince1970: TimeInterval(seconds)).formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).year().hour().minute())
    }
}
