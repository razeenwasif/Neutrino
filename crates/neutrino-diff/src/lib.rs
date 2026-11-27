//! Line diffs with word-level highlights.
//!
//! Lines are compared with imara-diff's histogram algorithm (git's
//! `diff.algorithm=histogram`), then post-processed the way git slides hunks
//! so they line up with indentation. Inside each changed block, removed and
//! added lines are paired in order, and each pair that is similar enough is
//! diffed again by word so the client can highlight what actually changed.

pub mod semantic;
pub mod syntax;
pub mod tour;

use imara_diff::{Algorithm, Diff, InternedInput, TokenSource};
use neutrino_proto::{DiffLine, Hunk, LineKind, MoveBlock, Segment, SemanticSummary};

/// Sides bigger than this are not diffed line by line.
pub const MAX_DIFF_BYTES: usize = 8 << 20;
/// Lines longer than this get no word highlights.
const MAX_WORD_DIFF_LINE: usize = 2_000;

/// What git checks: a NUL byte in the first 8000 bytes means binary.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

pub struct LineDiff {
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
}

pub struct RichLineDiff {
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
    pub moves: Vec<MoveBlock>,
    pub semantic_summary: Option<SemanticSummary>,
}

/// A sequence of byte slices as an imara-diff token source.
struct Tokens<'a>(Vec<&'a [u8]>);

impl<'a> TokenSource for Tokens<'a> {
    type Token = &'a [u8];
    type Tokenizer = std::vec::IntoIter<&'a [u8]>;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.clone().into_iter()
    }

    fn estimate_tokens(&self) -> u32 {
        self.0.len() as u32
    }
}

/// Split into lines, each keeping its `\n`, so a missing final newline is a change.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|&b| b == b'\n').collect()
}

fn display(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

/// Number of added and removed lines, without building hunks.
pub fn count(old: &[u8], new: &[u8]) -> (u32, u32) {
    let input = InternedInput::new(Tokens(lines(old)), Tokens(lines(new)));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    (diff.count_additions(), diff.count_removals())
}

/// Diff two texts into unified-diff hunks with `context` lines around each change.
pub fn diff(old: &[u8], new: &[u8], context: u32) -> LineDiff {
    let before = lines(old);
    let after = lines(new);
    let input = InternedInput::new(Tokens(before.clone()), Tokens(after.clone()));
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let changes: Vec<imara_diff::Hunk> = diff.hunks().collect();

    let old_no_eol = !old.is_empty() && !old.ends_with(b"\n");
    let new_no_eol = !new.is_empty() && !new.ends_with(b"\n");
    let old_len = before.len() as u32;

    let mut hunks = Vec::new();
    let mut i = 0;
    while i < changes.len() {
        // Merge changes whose unchanged gap would be covered by the context anyway.
        let mut j = i + 1;
        while j < changes.len() && changes[j].before.start - changes[j - 1].before.end <= 2 * context {
            j += 1;
        }
        let group = &changes[i..j];
        let first = &group[0];
        let last = &group[group.len() - 1];
        let old_begin = first.before.start.saturating_sub(context);
        let new_begin = first.after.start - (first.before.start - old_begin);
        let old_end = (last.before.end + context).min(old_len);
        let new_end = last.after.end + (old_end - last.before.end);

        let mut out = Vec::new();
        let ctx_line = |o: u32, n: u32| DiffLine {
            kind: LineKind::Context,
            old_line: Some(o + 1),
            new_line: Some(n + 1),
            text: display(before[o as usize]),
            segments: Vec::new(),
            no_newline: (old_no_eol && o + 1 == old_len) || (new_no_eol && n as usize + 1 == after.len()),
            syntax: Vec::new(),
        };
        let (mut o, mut n) = (old_begin, new_begin);
        for change in group {
            while o < change.before.start {
                out.push(ctx_line(o, n));
                o += 1;
                n += 1;
            }
            let removed: Vec<DiffLine> = change
                .before
                .clone()
                .map(|k| DiffLine {
                    kind: LineKind::Removed,
                    old_line: Some(k + 1),
                    new_line: None,
                    text: display(before[k as usize]),
                    segments: Vec::new(),
                    no_newline: old_no_eol && k + 1 == old_len,
                    syntax: Vec::new(),
                })
                .collect();
            let added: Vec<DiffLine> = change
                .after
                .clone()
                .map(|k| DiffLine {
                    kind: LineKind::Added,
                    old_line: None,
                    new_line: Some(k + 1),
                    text: display(after[k as usize]),
                    segments: Vec::new(),
                    no_newline: new_no_eol && k as usize + 1 == after.len(),
                    syntax: Vec::new(),
                })
                .collect();
            let (mut removed, mut added) = (removed, added);
            for (r, a) in removed.iter_mut().zip(added.iter_mut()) {
                if let Some((rs, as_)) = word_diff(&r.text, &a.text) {
                    r.segments = rs;
                    a.segments = as_;
                }
            }
            out.extend(removed);
            out.extend(added);
            o = change.before.end;
            n = change.after.end;
        }
        while o < old_end {
            out.push(ctx_line(o, n));
            o += 1;
            n += 1;
        }

        let old_lines = old_end - old_begin;
        let new_lines = new_end - new_begin;
        hunks.push(Hunk {
            // A unified diff names the line before an empty range.
            old_start: if old_lines == 0 { old_begin } else { old_begin + 1 },
            old_lines,
            new_start: if new_lines == 0 { new_begin } else { new_begin + 1 },
            new_lines,
            lines: out,
            noise: None,
        });
        i = j;
    }

    LineDiff { additions: diff.count_additions(), deletions: diff.count_removals(), hunks }
}

/// Diff two texts using the Tree-sitter grammar inferred from `path` to populate
/// line syntax spans, hunk noise classification (`Formatting`, `ImportSort`),
/// intra-file AST move blocks, and semantic symbol summary.
pub fn diff_with_path(path: &str, old: &[u8], new: &[u8], context: u32) -> RichLineDiff {
    let base = diff(old, new, context);
    let mut hunks = base.hunks;
    let old_syntax = syntax::highlight_lines(path, old);
    let new_syntax = syntax::highlight_lines(path, new);

    for h in &mut hunks {
        for l in &mut h.lines {
            l.syntax = match l.kind {
                LineKind::Removed => l
                    .old_line
                    .and_then(|n| old_syntax.get(n.saturating_sub(1) as usize))
                    .cloned()
                    .unwrap_or_default(),
                LineKind::Added | LineKind::Context => l
                    .new_line
                    .and_then(|n| new_syntax.get(n.saturating_sub(1) as usize))
                    .cloned()
                    .unwrap_or_default(),
            };
        }
    }

    let (moves, summary) = semantic::analyze_file(path, old, new, &mut hunks);
    let semantic_summary = (summary.moved_blocks > 0
        || summary.formatting_hunks > 0
        || summary.import_hunks > 0
        || !summary.changed_symbols.is_empty())
    .then_some(summary);

    RichLineDiff {
        additions: base.additions,
        deletions: base.deletions,
        hunks,
        moves,
        semantic_summary,
    }
}

/// Words, runs of whitespace, and single punctuation characters.
fn words(text: &str) -> Vec<&str> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<Class> = None;
    for (i, c) in text.char_indices() {
        let k = class(c);
        if let Some(p) = prev
            && (p != k || k == Class::Other)
        {
            out.push(&text[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn segments(tokens: &[&str], changed: impl Fn(u32) -> bool) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        let c = changed(i as u32);
        match out.last_mut() {
            Some(last) if last.changed == c => last.text.push_str(t),
            _ => out.push(Segment { text: (*t).to_string(), changed: c }),
        }
    }
    out
}

/// Word-level segments for a removed line and the added line paired with it,
/// or `None` when the lines are too different for highlights to help.
pub fn word_diff(old: &str, new: &str) -> Option<(Vec<Segment>, Vec<Segment>)> {
    if old == new || old.len() > MAX_WORD_DIFF_LINE || new.len() > MAX_WORD_DIFF_LINE {
        return None;
    }
    let a = words(old);
    let b = words(new);
    let input = InternedInput::new(Tokens(a.iter().map(|w| w.as_bytes()).collect()), Tokens(b.iter().map(|w| w.as_bytes()).collect()));
    let diff = Diff::compute(Algorithm::Histogram, &input);

    // Only highlight when a good share of the visible text is shared.
    let visible = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    let common: usize = a.iter().enumerate().filter(|(i, _)| !diff.is_removed(*i as u32)).map(|(_, w)| visible(w)).sum();
    let longest = visible(old).max(visible(new));
    if longest == 0 || common * 10 < longest * 4 {
        return None;
    }
    Some((segments(&a, |i| diff.is_removed(i)), segments(&b, |i| diff.is_added(i))))
}

/// How a partial patch will be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// `git apply` forwards: picked changes are added on top of the old side.
    Forward,
    /// `git apply --reverse`: picked changes are taken back off the new side.
    Reverse,
}

/// A unified patch holding only the picked lines of `hunks`, for `git apply`.
///
/// Lines that weren't picked must come out unchanged. Applied forwards, an
/// unpicked added line is left out and an unpicked removed line becomes
/// context; applied in reverse it's the other way round. Returns `None` when
/// nothing picked is in the hunks.
pub fn partial_patch(
    path: &str,
    hunks: &[Hunk],
    picked: &std::collections::HashSet<neutrino_proto::LinePick>,
    apply: Apply,
    new_file: bool,
) -> Option<String> {
    use neutrino_proto::LinePick;
    let is_picked = |l: &DiffLine| match l.kind {
        LineKind::Added => picked.contains(&LinePick { kind: LineKind::Added, line: l.new_line.unwrap_or(0) }),
        LineKind::Removed => picked.contains(&LinePick { kind: LineKind::Removed, line: l.old_line.unwrap_or(0) }),
        LineKind::Context => false,
    };

    let mut body = String::new();
    let mut delta: i64 = 0;
    for h in hunks {
        if !h.lines.iter().any(is_picked) {
            continue;
        }
        let mut lines: Vec<(char, &DiffLine)> = Vec::new();
        for l in &h.lines {
            let kept = match (l.kind, is_picked(l), apply) {
                (LineKind::Context, _, _) => Some(' '),
                (LineKind::Added, true, _) => Some('+'),
                (LineKind::Removed, true, _) => Some('-'),
                (LineKind::Added, false, Apply::Forward) => None,
                (LineKind::Removed, false, Apply::Forward) => Some(' '),
                (LineKind::Added, false, Apply::Reverse) => Some(' '),
                (LineKind::Removed, false, Apply::Reverse) => None,
            };
            if let Some(sign) = kept {
                lines.push((sign, l));
            }
        }
        let old_count = lines.iter().filter(|(s, _)| *s != '+').count() as i64;
        let new_count = lines.iter().filter(|(s, _)| *s != '-').count() as i64;
        let (old_start, new_start) = match apply {
            Apply::Forward => (h.old_start as i64, h.old_start as i64 + delta),
            Apply::Reverse => (h.new_start as i64 - delta, h.new_start as i64),
        };
        delta += new_count - old_count;
        body.push_str(&format!("@@ -{},{} +{},{} @@\n", old_start.max(0), old_count, new_start.max(0), new_count));
        for (sign, l) in lines {
            body.push(sign);
            body.push_str(&l.text);
            body.push('\n');
            if l.no_newline {
                body.push_str("\\ No newline at end of file\n");
            }
        }
    }
    if body.is_empty() {
        return None;
    }
    let mut patch = format!("diff --git a/{path} b/{path}\n");
    if new_file {
        patch.push_str(&format!("new file mode 100644\n--- /dev/null\n+++ b/{path}\n"));
    } else {
        patch.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    }
    patch.push_str(&body);
    Some(patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(h: &Hunk) -> String {
        let mut s = format!("@@ -{},{} +{},{} @@\n", h.old_start, h.old_lines, h.new_start, h.new_lines);
        for l in &h.lines {
            let sign = match l.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            };
            s.push(sign);
            s.push_str(&l.text);
            s.push('\n');
        }
        s
    }

    #[test]
    fn single_change_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nb\nc\nD\ne\nf\ng\n";
        let d = diff(old.as_bytes(), new.as_bytes(), 2);
        assert_eq!((d.additions, d.deletions), (1, 1));
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(render(&d.hunks[0]), "@@ -2,5 +2,5 @@\n b\n c\n-d\n+D\n e\n f\n");
    }

    #[test]
    fn distant_changes_split_close_ones_merge() {
        let old: String = (1..=20).map(|i| format!("{i}\n")).collect();
        let new: String = (1..=20)
            .map(|i| match i {
                2 => "two\n".to_string(),
                5 => "five\n".to_string(),
                18 => "eighteen\n".to_string(),
                _ => format!("{i}\n"),
            })
            .collect();
        let d = diff(old.as_bytes(), new.as_bytes(), 3);
        assert_eq!(d.hunks.len(), 2, "{:?}", d.hunks.iter().map(render).collect::<Vec<_>>());
        assert_eq!(d.hunks[0].old_start, 1);
        assert_eq!(d.hunks[1].old_start, 15);
    }

    #[test]
    fn new_file_and_deleted_file_headers() {
        let d = diff(b"", b"x\ny\n", 3);
        assert_eq!(render(&d.hunks[0]), "@@ -0,0 +1,2 @@\n+x\n+y\n");
        let d = diff(b"x\n", b"", 3);
        assert_eq!(render(&d.hunks[0]), "@@ -1,1 +0,0 @@\n-x\n");
    }

    #[test]
    fn missing_final_newline() {
        let d = diff(b"a\nb", b"a\nb\n", 3);
        assert_eq!((d.additions, d.deletions), (1, 1));
        let lines = &d.hunks[0].lines;
        let removed = lines.iter().find(|l| l.kind == LineKind::Removed).unwrap();
        let added = lines.iter().find(|l| l.kind == LineKind::Added).unwrap();
        assert!(removed.no_newline);
        assert!(!added.no_newline);
        assert!(removed.segments.is_empty(), "same text, nothing to highlight");
    }

    #[test]
    fn identical_is_empty() {
        let d = diff(b"same\n", b"same\n", 3);
        assert!(d.hunks.is_empty());
        assert_eq!(count(b"a\nb\n", b"a\nc\nd\n"), (2, 1));
    }

    #[test]
    fn word_highlights() {
        let (old, new) = word_diff("    return node.ram_gb - KEEP - reserved", "    return node.ram_gb - KEEP - reserved - held").unwrap();
        assert!(old.iter().all(|s| !s.changed));
        let changed: String = new.iter().filter(|s| s.changed).map(|s| s.text.as_str()).collect();
        assert_eq!(changed, " - held");
        assert_eq!(new.iter().map(|s| s.text.as_str()).collect::<String>(), "    return node.ram_gb - KEEP - reserved - held");
        assert!(word_diff("completely different", "nothing alike here at all").is_none());
    }

    #[test]
    fn binary_detection() {
        assert!(is_binary(b"PNG\0\x01"));
        assert!(!is_binary("plain text, ünïcode".as_bytes()));
    }

    #[test]
    fn partial_patches() {
        use neutrino_proto::LinePick;
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nD\nE\n";
        let d = diff(old.as_bytes(), new.as_bytes(), 3);
        // Pick only the change to b.
        let picks: std::collections::HashSet<_> =
            [LinePick { kind: LineKind::Removed, line: 2 }, LinePick { kind: LineKind::Added, line: 2 }].into_iter().collect();
        let fwd = partial_patch("f", &d.hunks, &picks, Apply::Forward, false).unwrap();
        assert_eq!(fwd, "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n d\n");
        let rev = partial_patch("f", &d.hunks, &picks, Apply::Reverse, false).unwrap();
        assert_eq!(rev, "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n D\n E\n");
        assert!(partial_patch("f", &d.hunks, &Default::default(), Apply::Forward, false).is_none());
    }

    #[test]
    fn crlf_is_not_shown() {
        let d = diff(b"a\r\nb\r\n", b"a\r\nc\r\n", 3);
        assert_eq!(d.hunks[0].lines[0].text, "a");
    }

    #[test]
    fn syntax_spans_for_languages() {
        let rust = b"// doc\npub fn compute(x: usize) -> usize { x + 42 }\n";
        let spans = syntax::highlight_lines("src/lib.rs", rust);
        assert_eq!(spans.len(), 2);
        assert!(spans[0].iter().any(|s| s.kind == neutrino_proto::SyntaxKind::Comment));
        assert!(spans[1].iter().any(|s| s.kind == neutrino_proto::SyntaxKind::Keyword));
        assert!(spans[1].iter().any(|s| s.kind == neutrino_proto::SyntaxKind::Function));
        assert!(spans[1].iter().any(|s| s.kind == neutrino_proto::SyntaxKind::Number));

        let swift = b"struct Item {\n    let title: String = \"ok\"\n}\n";
        let sw_spans = syntax::highlight_lines("App.swift", swift);
        assert!(sw_spans[0].iter().any(|s| s.kind == neutrino_proto::SyntaxKind::Keyword));
        assert!(sw_spans[1].iter().any(|s| s.kind == neutrino_proto::SyntaxKind::String));

        for (p, src) in [
            ("mod.ts", b"export async function load(n: number): Promise<void> {}" as &[u8]),
            ("app.py", b"def run(x):\n    return \"hi\"\n"),
            ("pkg.json", b"{\"name\": \"neutrino\", \"count\": 3}\n"),
            ("Cargo.toml", b"[package]\nname = \"x\"\n"),
            ("ci.yml", b"name: ci\non: [push]\n"),
            ("run.sh", b"#!/bin/sh\necho \"hello\"\n"),
        ] {
            let s = syntax::highlight_lines(p, src);
            assert!(!s.is_empty(), "expected syntax lines for {p}");
            assert!(s.iter().any(|line| !line.is_empty()), "expected tokens for {p}");
        }
    }

    #[test]
    fn formatting_and_import_sort_noise_folding() {
        let old = b"use b::Beta;\nuse a::Alpha;\n\nconst A: i32 = 1;\nconst B: i32 = 2;\nconst C: i32 = 3;\nconst D: i32 = 4;\nconst E: i32 = 5;\nconst F: i32 = 6;\nconst G: i32 = 7;\n\nfn calc(x: i32) -> i32 { x + 1 }\n";
        let new = b"use a::Alpha;\nuse b::Beta;\n\nconst A: i32 = 1;\nconst B: i32 = 2;\nconst C: i32 = 3;\nconst D: i32 = 4;\nconst E: i32 = 5;\nconst F: i32 = 6;\nconst G: i32 = 7;\n\nfn calc(\n    x: i32,\n) -> i32 {\n    x + 1\n}\n";
        let rich = diff_with_path("src/math.rs", old, new, 3);
        assert!(rich.hunks.iter().any(|h| h.noise == Some(neutrino_proto::NoiseKind::ImportSort)));
        let fmt_old = b"let total=alpha+beta;\n";
        let fmt_new = b"let total = alpha + beta;\n";
        let fmt_rich = diff_with_path("src/a.rs", fmt_old, fmt_new, 3);
        assert_eq!(fmt_rich.hunks[0].noise, Some(neutrino_proto::NoiseKind::Formatting));
    }

    #[test]
    fn intra_and_cross_file_ast_move_detection() {
        let old = b"fn alpha() -> i32 {\n    let a = 10;\n    let b = 20;\n    a + b\n}\n\nfn middle_one() -> i32 { 1 }\nfn middle_two() -> i32 { 2 }\nfn middle_three() -> i32 { 3 }\n";
        let new = b"fn middle_one() -> i32 { 1 }\nfn middle_two() -> i32 { 2 }\nfn middle_three() -> i32 { 3 }\n\nfn alpha() -> i32 {\n    let a = 10;\n    let b = 21;\n    a + b\n}\n";
        let rich = diff_with_path("src/lib.rs", old, new, 3);
        assert_eq!(rich.moves.len(), 1, "expected 1 intra-file move: {:?}", rich.moves);
        assert_eq!(rich.moves[0].symbol, "alpha");
        assert_eq!(rich.moves[0].changed_lines, 1);

        let f1_old = b"pub fn helper_work(val: i32) -> i32 {\n    let doubled = val * 2;\n    doubled + 5\n}\n";
        let f1_new = b"// moved\n";
        let f2_old = b"";
        let f2_new = b"pub fn helper_work(val: i32) -> i32 {\n    let doubled = val * 2;\n    doubled + 5\n}\n";
        let cross = semantic::detect_cross_file_moves(&[("src/a.rs", f1_old, f1_new), ("src/b.rs", f2_old, f2_new)]);
        assert_eq!(cross.len(), 1);
        assert_eq!((cross[0].from_path.as_str(), cross[0].to_path.as_str()), ("src/a.rs", "src/b.rs"));
    }

    #[test]
    fn call_graph_review_tour_order() {
        let files: Vec<(&str, &[u8], &[u8])> = vec![
            ("crates/app/tests/flow.rs", b"", b"fn test_flow() { let _ = build_token(); }"),
            ("crates/app/src/core.rs", b"", b"pub fn build_token() -> Token { Token {} }"),
            ("crates/app/src/proto.rs", b"", b"pub struct Token {}"),
            ("apps/Mac/TokenView.swift", b"", b"struct TokenView { let tok: Token }"),
        ];
        let tour = tour::compute_review_tour(&files);
        let paths: Vec<&str> = tour.iter().map(|s| s.path.as_str()).collect();
        let pos = |p: &str| paths.iter().position(|x| *x == p).unwrap();
        assert!(pos("crates/app/src/proto.rs") < pos("crates/app/src/core.rs"));
        assert!(pos("crates/app/src/core.rs") < pos("apps/Mac/TokenView.swift"));
        assert!(pos("apps/Mac/TokenView.swift") < pos("crates/app/tests/flow.rs"));
    }
}
