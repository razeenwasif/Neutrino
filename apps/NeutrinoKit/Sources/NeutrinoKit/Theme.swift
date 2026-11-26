import SwiftUI

/// Stratus-aligned pitch-black T3 design tokens.
/// Zero borders, zero pane dividers, pure typographic hierarchy and subtle tonal elevation.
public enum N {
    // Surfaces, back to front (T3 pitch-black elevation).
    public static let ground = Color(hex: 0x000000)
    public static let sidebar = Color(hex: 0x030304)
    public static let inspector = Color(hex: 0x060608)
    public static let palette = Color(hex: 0x0B0B0F)
    public static let sheet = Color(hex: 0x050507)
    public static let surfaceSubtle = Color(hex: 0x0B0B0E)
    public static let surfaceHover = Color(hex: 0x111114)
    public static let surfaceActive = Color(hex: 0x16161B)
    public static let surfacePill = Color(hex: 0x151519)

    // Backwards-compatible semantic surface aliases used across Mac and iOS.
    public static let island = inspector
    public static let well = surfaceSubtle
    public static let raised = surfaceHover
    public static let control = surfacePill
    public static let selected = surfaceActive
    public static let sheen = Color.white.opacity(0.04)
    public static let hairline = Color.white.opacity(0.06)

    // Text hierarchy.
    public static let text = Color(hex: 0xF4F4F6)
    public static let body = Color(hex: 0xD4D4DC)
    public static let muted = Color(hex: 0x8E8E99)
    public static let subtle = Color(hex: 0x6E6E77)
    public static let faint = Color(hex: 0x585862)

    // Stratus semantic accent palette.
    public static let indigo = Color(hex: 0x7A89FF)
    public static let sky = Color(hex: 0x4BC3F7)
    public static let mint = Color(hex: 0x34D399)
    public static let amber = Color(hex: 0xFBBF24)
    public static let rose = Color(hex: 0xF87171)
    public static let coral = Color(hex: 0xE88255)
    public static let violet = Color(hex: 0xA78BFA)
    public static let slate = Color(hex: 0x71717A)

    // Backwards-compatible accent and status aliases.
    public static let accent = indigo
    public static let onAccent = Color(hex: 0x050507)
    /// Second marker colour, for things that must differ from the accent (remote refs).
    public static let second = sky
    public static let ok = mint
    public static let warn = amber
    public static let danger = rose
    /// Machine colours shared with Nexus.
    public static let atlas = violet
    public static let forge = coral

    /// Colours for graph lanes and language segments, picked by index.
    public static let lanes: [Color] = [
        indigo, sky, mint, coral, violet, amber, rose, Color(hex: 0x38BDF8),
    ]

    public static func lane(_ index: UInt16) -> Color { lanes[Int(index) % lanes.count] }

    // Typography helpers.
    public static func heroKPI() -> Font {
        .system(size: 36, weight: .bold, design: .default).monospacedDigit()
    }

    public static func pageTitle() -> Font {
        .system(size: 22, weight: .semibold, design: .default)
    }

    public static func sectionTitle() -> Font {
        .system(size: 13, weight: .semibold, design: .default)
    }

    public static func statValue() -> Font {
        .system(size: 20, weight: .semibold, design: .default).monospacedDigit()
    }

    public static func mono(_ size: CGFloat = 12, weight: Font.Weight = .regular) -> Font {
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
