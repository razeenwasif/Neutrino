import SwiftUI

/// Maps Tree-sitter syntax spans and diff word-emphasis segments into styled `AttributedString`s
/// aligned with the Stratus T3 pitch-black palette.
public enum SyntaxHighlight {
    public static func color(for kind: SyntaxKind) -> Color {
        switch kind {
        case .keyword: N.violet
        case .type: N.sky
        case .function: N.indigo
        case .string: N.mint
        case .number: N.amber
        case .comment: N.faint
        case .property: Color(hex: 0x38BDF8)
        case .operator: N.muted
        case .attribute: N.coral
        case .constant: N.amber
        }
    }

    /// Build an `AttributedString` combining UTF-8 byte syntax spans and changed-word segments.
    /// Tabs are optionally expanded after UTF-8 slicing so byte offsets remain exact.
    public static func attributedLine(
        text: String,
        syntax: [SyntaxSpan],
        segments: [Segment] = [],
        fallback: Color = N.body,
        emph: Color = .clear,
        expandTabs: Bool = false
    ) -> AttributedString {
        if text.isEmpty {
            var s = AttributedString(" ")
            s.foregroundColor = fallback
            return s
        }
        let utf8 = Array(text.utf8)
        let n = utf8.count
        if syntax.isEmpty && segments.isEmpty {
            let rendered = expandTabs ? text.replacingOccurrences(of: "\t", with: "    ") : text
            var s = AttributedString(rendered.isEmpty ? " " : rendered)
            s.foregroundColor = fallback
            return s
        }

        // Collect byte-offset intervals for changed word-diff segments.
        var emphRanges: [(Int, Int)] = []
        if !segments.isEmpty {
            var offset = 0
            for seg in segments {
                let len = seg.text.utf8.count
                if seg.changed && len > 0 {
                    emphRanges.append((offset, min(offset + len, n)))
                }
                offset += len
            }
        }

        var boundaries: Set<Int> = [0, n]
        var normalizedSpans: [(Int, Int, SyntaxKind)] = []
        normalizedSpans.reserveCapacity(syntax.count)
        for sp in syntax {
            let s = Int(sp.start)
            let e = min(s + Int(sp.len), n)
            if s < e {
                normalizedSpans.append((s, e, sp.kind))
                boundaries.insert(s)
                boundaries.insert(e)
            }
        }
        for r in emphRanges where r.0 < r.1 {
            boundaries.insert(r.0)
            boundaries.insert(r.1)
        }

        let sorted = boundaries.sorted()
        var out = AttributedString()
        for i in 0 ..< (sorted.count - 1) {
            let lo = sorted[i]
            let hi = sorted[i + 1]
            guard lo < hi else { continue }
            var piece = String(decoding: utf8[lo..<hi], as: UTF8.self)
            if expandTabs {
                piece = piece.replacingOccurrences(of: "\t", with: "    ")
            }
            var part = AttributedString(piece)
            var fg = fallback
            // Prefer the most specific (innermost / last) span covering [lo, hi).
            var bestLen = Int.max
            for (s, e, k) in normalizedSpans where s <= lo && e >= hi {
                if (e - s) <= bestLen {
                    bestLen = e - s
                    fg = color(for: k)
                }
            }
            part.foregroundColor = fg
            if emphRanges.contains(where: { $0.0 <= lo && $0.1 >= hi }) {
                part.backgroundColor = emph
            }
            out += part
        }
        return out
    }
}
