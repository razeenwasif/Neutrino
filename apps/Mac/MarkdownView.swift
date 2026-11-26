import NeutrinoKit
import SwiftUI

/// READMEs and Markdown files, rendered with editorial typography on pitch-black surfaces.
struct MarkdownView: View {
    var text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(Array(Markdown.blocks(text).enumerated()), id: \.offset) { _, block in
                view(block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    @ViewBuilder
    private func view(_ block: Markdown.Block) -> some View {
        switch block {
        case let .heading(level, text):
            VStack(alignment: .leading, spacing: 6) {
                Text(Markdown.inline(text))
                    .font(.system(size: [0, 24, 19, 16, 14, 13, 13][min(level, 6)], weight: .semibold))
                    .tracking(level <= 2 ? -0.3 : 0)
                    .foregroundStyle(N.text)
                    .padding(.top, level <= 2 ? 8 : 3)
                if level <= 2 {
                    Rectangle().fill(N.hairline).frame(height: 1)
                }
            }
        case let .paragraph(text):
            Text(Markdown.inline(text))
                .font(.system(size: 13.5))
                .foregroundStyle(N.body)
                .lineSpacing(4)
        case let .list(items, ordered):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { i, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(item.task.map { $0 ? "☑︎" : "☐" } ?? (ordered ? "\(i + 1)." : "•"))
                            .font(.system(size: 13))
                            .foregroundStyle(item.task == true ? N.mint : N.faint)
                            .frame(minWidth: 14, alignment: .trailing)
                        Text(Markdown.inline(item.text))
                            .font(.system(size: 13.5))
                            .foregroundStyle(N.body)
                            .lineSpacing(4)
                    }
                    .padding(.leading, CGFloat(item.depth) * 18)
                }
            }
        case let .quote(text):
            HStack(spacing: 12) {
                RoundedRectangle(cornerRadius: 2).fill(N.indigo.opacity(0.65)).frame(width: 2.5)
                Text(Markdown.inline(text))
                    .font(.system(size: 13.5))
                    .foregroundStyle(N.muted)
                    .lineSpacing(4)
            }
            .padding(.vertical, 2)
            .fixedSize(horizontal: false, vertical: true)
        case let .code(text, language):
            VStack(alignment: .leading, spacing: 0) {
                if !language.isEmpty {
                    Text(language)
                        .font(N.mono(10, weight: .semibold))
                        .foregroundStyle(N.faint)
                        .padding(.horizontal, 12)
                        .padding(.top, 9)
                }
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(text)
                        .font(N.mono(12))
                        .foregroundStyle(N.body)
                        .padding(.horizontal, 12)
                        .padding(.vertical, language.isEmpty ? 12 : 8)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        case let .table(header, rows):
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 8) {
                GridRow {
                    ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                        Text(Markdown.inline(cell))
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(N.text)
                    }
                }
                Rectangle().fill(N.hairline).frame(height: 1).gridCellUnsizedAxes(.horizontal)
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                            Text(Markdown.inline(cell))
                                .font(.system(size: 12.5))
                                .foregroundStyle(N.body)
                        }
                    }
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(N.surfaceSubtle, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        case .rule:
            Rectangle().fill(N.hairline).frame(height: 1).padding(.vertical, 6)
        }
    }
}

enum Markdown {
    struct Item: Equatable {
        var text: String
        var depth: Int
        /// Checked state for `- [x]` items.
        var task: Bool?
    }

    enum Block: Equatable {
        case heading(Int, String)
        case paragraph(String)
        case list([Item], ordered: Bool)
        case quote(String)
        case code(String, language: String)
        case table([String], [[String]])
        case rule
    }

    static func inline(_ s: String) -> AttributedString {
        // Images become their alt text as a link; HTML tags are dropped.
        var t = s.replacingOccurrences(of: #"!\[([^\]]*)\]\(([^)]*)\)"#, with: "[$1]($2)", options: .regularExpression)
        t = t.replacingOccurrences(of: #"<[^>]+>"#, with: "", options: .regularExpression)
        var a = (try? AttributedString(markdown: t, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(t)
        for run in a.runs {
            if let intent = run.inlinePresentationIntent, intent.contains(.code) {
                a[run.range].font = N.mono(12)
                a[run.range].backgroundColor = N.surfacePill
                a[run.range].foregroundColor = N.text
            }
            if run.link != nil {
                a[run.range].foregroundColor = N.sky
            }
        }
        return a
    }

    static func blocks(_ text: String) -> [Block] {
        var out: [Block] = []
        var para: [String] = []
        var lines = text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")[...]

        func flush() {
            if !para.isEmpty { out.append(.paragraph(para.joined(separator: " "))) }
            para = []
        }

        while let line = lines.popFirst() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                flush()
                let fence = String(trimmed.prefix(3))
                var body: [String] = []
                while let l = lines.popFirst(), !l.trimmingCharacters(in: .whitespaces).hasPrefix(fence) { body.append(l) }
                out.append(.code(body.joined(separator: "\n"), language: String(trimmed.dropFirst(3)).trimmingCharacters(in: .whitespaces)))
            } else if trimmed.isEmpty {
                flush()
            } else if let h = heading(trimmed) {
                flush()
                out.append(h)
            } else if ["---", "***", "___"].contains(where: { trimmed.replacingOccurrences(of: " ", with: "").hasPrefix($0) && Set(trimmed.replacingOccurrences(of: " ", with: "")).count == 1 }) {
                flush()
                out.append(.rule)
            } else if trimmed.hasPrefix(">") {
                flush()
                var quote = [String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces)]
                while let next = lines.first, next.trimmingCharacters(in: .whitespaces).hasPrefix(">") {
                    quote.append(String(next.trimmingCharacters(in: .whitespaces).dropFirst()).trimmingCharacters(in: .whitespaces))
                    lines.removeFirst()
                }
                out.append(.quote(quote.joined(separator: " ")))
            } else if let first = listItem(line) {
                flush()
                var items = [first.item]
                while let next = lines.first {
                    if let i = listItem(next) {
                        items.append(i.item)
                    } else if !next.trimmingCharacters(in: .whitespaces).isEmpty, next.hasPrefix("  "), !items.isEmpty {
                        items[items.count - 1].text += " " + next.trimmingCharacters(in: .whitespaces)
                    } else {
                        break
                    }
                    lines.removeFirst()
                }
                out.append(.list(items, ordered: first.ordered))
            } else if trimmed.hasPrefix("|"), let sep = lines.first, isTableSeparator(sep) {
                flush()
                lines.removeFirst()
                var rows: [[String]] = []
                while let next = lines.first, next.trimmingCharacters(in: .whitespaces).hasPrefix("|") {
                    rows.append(cells(next))
                    lines.removeFirst()
                }
                out.append(.table(cells(trimmed), rows))
            } else if let next = lines.first, isSetextUnderline(next) {
                flush()
                out.append(.heading(next.contains("=") ? 1 : 2, trimmed))
                lines.removeFirst()
            } else {
                para.append(trimmed)
            }
        }
        flush()
        return out
    }

    private static func heading(_ s: String) -> Block? {
        let hashes = s.prefix(while: { $0 == "#" }).count
        guard (1 ... 6).contains(hashes), s.dropFirst(hashes).first == " " else { return nil }
        let title = s.dropFirst(hashes).trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "#")).trimmingCharacters(in: .whitespaces)
        return .heading(hashes, title)
    }

    private static func isSetextUnderline(_ s: String) -> Bool {
        let t = s.trimmingCharacters(in: .whitespaces)
        return t.count >= 3 && (Set(t) == ["="] || Set(t) == ["-"])
    }

    private static func listItem(_ line: String) -> (item: Item, ordered: Bool)? {
        let indent = line.prefix(while: { $0 == " " }).count
        let s = line.dropFirst(indent)
        var ordered = false
        var rest: Substring
        if let m = s.first, "-*+".contains(m), s.dropFirst().first == " " {
            rest = s.dropFirst(2)
        } else {
            let digits = s.prefix(while: \.isNumber)
            guard !digits.isEmpty, digits.count <= 9 else { return nil }
            let after = s.dropFirst(digits.count)
            guard let p = after.first, p == "." || p == ")", after.dropFirst().first == " " else { return nil }
            rest = after.dropFirst(2)
            ordered = true
        }
        var task: Bool?
        if rest.hasPrefix("[ ] ") { task = false; rest = rest.dropFirst(4) }
        if rest.lowercased().hasPrefix("[x] ") { task = true; rest = rest.dropFirst(4) }
        return (Item(text: rest.trimmingCharacters(in: .whitespaces), depth: indent / 2, task: task), ordered)
    }

    private static func isTableSeparator(_ s: String) -> Bool {
        let t = s.trimmingCharacters(in: .whitespaces)
        return t.hasPrefix("|") && t.contains("-") && t.allSatisfy { "|-: ".contains($0) }
    }

    private static func cells(_ s: String) -> [String] {
        var t = s.trimmingCharacters(in: .whitespaces)
        if t.hasPrefix("|") { t.removeFirst() }
        if t.hasSuffix("|") { t.removeLast() }
        return t.split(separator: "|", omittingEmptySubsequences: false).map { $0.trimmingCharacters(in: .whitespaces) }
    }
}
