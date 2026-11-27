//! AST declaration extraction, move-aware diff pairing, and noise folding
//! (`Formatting` whitespace changes and `ImportSort` reorders).

use std::collections::{BTreeSet, HashSet};

use neutrino_proto::{Hunk, LineKind, MoveBlock, NoiseKind, SemanticSummary, SymbolDelta, SymbolDeltaKind};
use tree_sitter::Node;

use crate::syntax::{self, Lang};

#[derive(Debug, Clone)]
pub struct DeclNode {
    pub symbol: String,
    pub kind: &'static str,
    /// 1-based inclusive line numbers.
    pub start_line: u32,
    pub end_line: u32,
    pub tokens: Vec<String>,
    pub fingerprint: u64,
}

fn fnv1a_tokens(tokens: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in tokens {
        for &b in t.as_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn decl_kind_label(raw: &str) -> Option<&'static str> {
    match raw {
        "function_item" | "function_declaration" | "function_definition" | "function_signature_item" => Some("function"),
        "method_definition" => Some("method"),
        "struct_item" | "struct_declaration" => Some("struct"),
        "enum_item" | "enum_declaration" => Some("enum"),
        "class_declaration" | "class_definition" => Some("class"),
        "trait_item" | "protocol_declaration" | "interface_declaration" => Some("trait"),
        "impl_item" => Some("impl"),
        "mod_item" => Some("module"),
        _ => None,
    }
}

fn extract_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name").and_then(|n| n.utf8_text(bytes).ok()) {
        return Some(name.trim().to_string());
    }
    if node.kind() == "impl_item"
        && let Some(ty) = node.child_by_field_name("type").and_then(|n| n.utf8_text(bytes).ok())
    {
        return Some(format!("impl {}", ty.trim()));
    }
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        if matches!(ch.kind(), "identifier" | "type_identifier" | "simple_identifier")
            && let Ok(t) = ch.utf8_text(bytes)
        {
            return Some(t.trim().to_string());
        }
    }
    None
}

fn collect_norm_tokens(node: Node<'_>, bytes: &[u8], out: &mut Vec<String>) {
    if node.kind().contains("comment") {
        return;
    }
    if node.child_count() == 0 {
        if let Ok(text) = node.utf8_text(bytes) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for ch in node.children(&mut cursor) {
        collect_norm_tokens(ch, bytes, out);
    }
}

fn walk_decls(node: Node<'_>, bytes: &[u8], depth: usize, out: &mut Vec<DeclNode>) {
    if let Some(kind_label) = decl_kind_label(node.kind())
        && let Some(symbol) = extract_name(node, bytes)
    {
        let start_line = node.start_position().row as u32 + 1;
        let end_line = node.end_position().row as u32 + 1;
        if end_line >= start_line {
            let mut tokens = Vec::new();
            collect_norm_tokens(node, bytes, &mut tokens);
            if tokens.len() >= 3 {
                let fingerprint = fnv1a_tokens(&tokens);
                out.push(DeclNode { symbol, kind: kind_label, start_line, end_line, tokens, fingerprint });
            }
        }
    }
    if depth >= 3 {
        return;
    }
    let mut cursor = node.walk();
    for ch in node.children(&mut cursor) {
        walk_decls(ch, bytes, depth + 1, out);
    }
}

/// Extract normalized AST declarations (functions, structs, classes, traits, impls) for `path`.
pub fn extract_decls(path: &str, bytes: &[u8]) -> Vec<DeclNode> {
    let Some(lang) = Lang::for_path(path) else {
        return Vec::new();
    };
    if !lang.is_code() || bytes.is_empty() || crate::is_binary(bytes) {
        return Vec::new();
    }
    let Some(tree) = syntax::parse_tree(lang, bytes) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_decls(tree.root_node(), bytes, 0, &mut out);
    out
}

fn dice_similarity(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let set_a: BTreeSet<&str> = a.iter().map(String::as_str).collect();
    let set_b: BTreeSet<&str> = b.iter().map(String::as_str).collect();
    let inter = set_a.intersection(&set_b).count();
    (2.0 * inter as f64) / ((set_a.len() + set_b.len()) as f64)
}

fn looks_like_import(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t.starts_with("use ")
        || t.starts_with("pub use ")
        || t.starts_with("import ")
        || t.starts_with("from ")
        || t.starts_with("@testable import ")
        || t.starts_with("#include ")
        || (t.contains("require(") && t.starts_with("const "))
}

fn classify_hunk_noise(h: &Hunk) -> Option<NoiseKind> {
    let removed: Vec<&str> = h.lines.iter().filter(|l| l.kind == LineKind::Removed).map(|l| l.text.as_str()).collect();
    let added: Vec<&str> = h.lines.iter().filter(|l| l.kind == LineKind::Added).map(|l| l.text.as_str()).collect();
    if removed.is_empty() && added.is_empty() {
        return None;
    }
    if !removed.is_empty() && removed.len() == added.len() && removed.iter().chain(added.iter()).all(|l| looks_like_import(l)) {
        let mut r_sorted: Vec<&str> = removed.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        let mut a_sorted: Vec<&str> = added.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        r_sorted.sort_unstable();
        a_sorted.sort_unstable();
        if r_sorted == a_sorted {
            return Some(NoiseKind::ImportSort);
        }
    }
    let norm = |xs: &[&str]| -> String {
        xs.iter().flat_map(|s| s.chars()).filter(|c| !c.is_whitespace()).collect()
    };
    if norm(&removed) == norm(&added) {
        return Some(NoiseKind::Formatting);
    }
    None
}

fn decl_body_slice(bytes: &[u8], start_line: u32, end_line: u32) -> Vec<u8> {
    let lines: Vec<&[u8]> = bytes.split_inclusive(|&b| b == b'\n').collect();
    let s = (start_line.saturating_sub(1) as usize).min(lines.len());
    let e = (end_line as usize).min(lines.len());
    lines[s..e].concat()
}

fn changed_line_numbers(old: &[u8], new: &[u8]) -> (HashSet<u32>, HashSet<u32>) {
    let mut rem = HashSet::new();
    let mut add = HashSet::new();
    for h in crate::diff(old, new, 0).hunks {
        for l in h.lines {
            match l.kind {
                LineKind::Removed => {
                    if let Some(n) = l.old_line {
                        rem.insert(n);
                    }
                }
                LineKind::Added => {
                    if let Some(n) = l.new_line {
                        add.insert(n);
                    }
                }
                LineKind::Context => {}
            }
        }
    }
    (rem, add)
}

fn overlap_ratio(start: u32, end: u32, lines: &HashSet<u32>) -> f64 {
    let total = end.saturating_sub(start) + 1;
    let hits = (start..=end).filter(|ln| lines.contains(ln)).count() as u32;
    hits as f64 / total.max(1) as f64
}

/// Classify hunk noise and detect intra-file moved AST declarations.
pub fn analyze_file(path: &str, old: &[u8], new: &[u8], hunks: &mut [Hunk]) -> (Vec<MoveBlock>, SemanticSummary) {
    let mut summary = SemanticSummary::default();
    for h in hunks.iter_mut() {
        h.noise = classify_hunk_noise(h);
        match h.noise {
            Some(NoiseKind::Formatting) => summary.formatting_hunks += 1,
            Some(NoiseKind::ImportSort) => summary.import_hunks += 1,
            None => {}
        }
    }

    let (removed_lines, added_lines) = changed_line_numbers(old, new);
    let old_decls = extract_decls(path, old);
    let new_decls = extract_decls(path, new);

    let mut changed_syms = BTreeSet::new();
    for d in &old_decls {
        if overlap_ratio(d.start_line, d.end_line, &removed_lines) > 0.0 {
            changed_syms.insert(d.symbol.clone());
        }
    }
    for d in &new_decls {
        if overlap_ratio(d.start_line, d.end_line, &added_lines) > 0.0 {
            changed_syms.insert(d.symbol.clone());
        }
    }

    let removed_decls: Vec<&DeclNode> = old_decls.iter().filter(|d| overlap_ratio(d.start_line, d.end_line, &removed_lines) >= 0.70).collect();
    let added_decls: Vec<&DeclNode> = new_decls.iter().filter(|d| overlap_ratio(d.start_line, d.end_line, &added_lines) >= 0.70).collect();

    let mut moves = Vec::new();
    let mut used_added = HashSet::new();
    let mut next_id = 1u32;

    for od in &removed_decls {
        let mut best: Option<(usize, f64)> = None;
        for (idx, nd) in added_decls.iter().enumerate() {
            if used_added.contains(&idx) || od.kind != nd.kind {
                continue;
            }
            if od.start_line.abs_diff(nd.start_line) < 2 && od.symbol == nd.symbol {
                continue;
            }
            let sim = if od.fingerprint == nd.fingerprint { 1.0 } else { dice_similarity(&od.tokens, &nd.tokens) };
            let min_sim = if od.symbol == nd.symbol { 0.70 } else { 0.82 };
            if sim >= min_sim && best.is_none_or(|(_, b)| sim > b) {
                best = Some((idx, sim));
            }
        }
        if let Some((idx, _)) = best {
            used_added.insert(idx);
            let nd = added_decls[idx];
            let old_slice = decl_body_slice(old, od.start_line, od.end_line);
            let new_slice = decl_body_slice(new, nd.start_line, nd.end_line);
            let (adds, dels) = crate::count(&old_slice, &new_slice);
            moves.push(MoveBlock {
                id: next_id,
                symbol: nd.symbol.clone(),
                kind: nd.kind.to_string(),
                from_path: path.to_string(),
                from_line: od.start_line,
                to_path: path.to_string(),
                to_line: nd.start_line,
                lines: nd.end_line - nd.start_line + 1,
                changed_lines: adds.max(dels),
            });
            next_id += 1;
        }
    }

    summary.moved_blocks = moves.len() as u32;
    summary.changed_symbols = changed_syms.into_iter().take(24).collect();
    (moves, summary)
}

/// Detect AST declarations moved across different files in the same commit or PR.
pub fn detect_cross_file_moves(files: &[(&str, &[u8], &[u8])]) -> Vec<MoveBlock> {
    struct RemovedEntry<'a> {
        path: &'a str,
        bytes: &'a [u8],
        decl: DeclNode,
    }
    struct AddedEntry<'a> {
        path: &'a str,
        bytes: &'a [u8],
        decl: DeclNode,
    }

    let mut removed = Vec::new();
    let mut added = Vec::new();

    for &(path, old, new) in files {
        let (rem_lines, add_lines) = changed_line_numbers(old, new);
        for decl in extract_decls(path, old) {
            if overlap_ratio(decl.start_line, decl.end_line, &rem_lines) >= 0.70 {
                removed.push(RemovedEntry { path, bytes: old, decl });
            }
        }
        for decl in extract_decls(path, new) {
            if overlap_ratio(decl.start_line, decl.end_line, &add_lines) >= 0.70 {
                added.push(AddedEntry { path, bytes: new, decl });
            }
        }
    }

    let mut out = Vec::new();
    let mut used = HashSet::new();
    let mut next_id = 1u32;

    for r in &removed {
        let mut best: Option<(usize, f64)> = None;
        for (idx, a) in added.iter().enumerate() {
            if used.contains(&idx) || a.path == r.path || a.decl.kind != r.decl.kind {
                continue;
            }
            let sim = if r.decl.fingerprint == a.decl.fingerprint {
                1.0
            } else {
                dice_similarity(&r.decl.tokens, &a.decl.tokens)
            };
            let min_sim = if r.decl.symbol == a.decl.symbol { 0.72 } else { 0.85 };
            if sim >= min_sim && best.is_none_or(|(_, b)| sim > b) {
                best = Some((idx, sim));
            }
        }
        if let Some((idx, _)) = best {
            used.insert(idx);
            let a = &added[idx];
            let old_slice = decl_body_slice(r.bytes, r.decl.start_line, r.decl.end_line);
            let new_slice = decl_body_slice(a.bytes, a.decl.start_line, a.decl.end_line);
            let (adds, dels) = crate::count(&old_slice, &new_slice);
            out.push(MoveBlock {
                id: next_id,
                symbol: a.decl.symbol.clone(),
                kind: a.decl.kind.to_string(),
                from_path: r.path.to_string(),
                from_line: r.decl.start_line,
                to_path: a.path.to_string(),
                to_line: a.decl.start_line,
                lines: a.decl.end_line - a.decl.start_line + 1,
                changed_lines: adds.max(dels),
            });
            next_id += 1;
        }
    }
    out
}

fn decl_signature_header(path: &str, bytes: &[u8], start_line: u32, end_line: u32) -> (bool, String) {
    let slice = decl_body_slice(bytes, start_line, end_line.min(start_line + 6));
    let text = String::from_utf8_lossy(&slice);
    let first_line = text.lines().next().unwrap_or("").trim();
    let is_public = if path.ends_with(".rs") {
        first_line.starts_with("pub ") || first_line.starts_with("pub(")
    } else if path.ends_with(".swift") {
        first_line.contains("public ") || first_line.contains("open ")
    } else if path.ends_with(".ts") || path.ends_with(".tsx") || path.ends_with(".js") || path.ends_with(".jsx") {
        first_line.starts_with("export ")
    } else if path.ends_with(".py") {
        !text.starts_with(' ')
            && !text.starts_with('\t')
            && !first_line.starts_with("def _")
            && !first_line.starts_with("class _")
            && (first_line.starts_with("def ") || first_line.starts_with("async def ") || first_line.starts_with("class "))
    } else if path.ends_with(".go") {
        first_line
            .strip_prefix("func ")
            .and_then(|s| s.trim_start().chars().next())
            .is_some_and(|c| c.is_ascii_uppercase())
    } else {
        false
    };
    let head = text.split('{').next().unwrap_or(&text);
    let normalized: String = head.split_whitespace().collect::<Vec<_>>().join(" ");
    (is_public, normalized)
}

/// Compare exported/public declarations in `old` vs `new` for PR Blast Radius Receipts (Wave 5B).
pub fn public_symbol_deltas(path: &str, old: &[u8], new: &[u8]) -> Vec<SymbolDelta> {
    use std::collections::BTreeMap;

    let collect_public = |bytes: &[u8]| -> BTreeMap<(String, &'static str), String> {
        let mut map = BTreeMap::new();
        for decl in extract_decls(path, bytes) {
            let (is_pub, sig) = decl_signature_header(path, bytes, decl.start_line, decl.end_line);
            if is_pub {
                map.insert((decl.symbol, decl.kind), sig);
            }
        }
        map
    };

    let old_map = collect_public(old);
    let new_map = collect_public(new);
    let mut out = Vec::new();

    for ((sym, kind), sig) in &new_map {
        match old_map.get(&(sym.clone(), *kind)) {
            None => out.push(SymbolDelta {
                path: path.to_string(),
                symbol: sym.clone(),
                kind: (*kind).to_string(),
                change: SymbolDeltaKind::Added,
            }),
            Some(prev) if prev != sig => out.push(SymbolDelta {
                path: path.to_string(),
                symbol: sym.clone(),
                kind: (*kind).to_string(),
                change: SymbolDeltaKind::SignatureChanged,
            }),
            Some(_) => {}
        }
    }
    for (sym, kind) in old_map.keys() {
        if !new_map.contains_key(&(sym.clone(), *kind)) {
            out.push(SymbolDelta {
                path: path.to_string(),
                symbol: sym.clone(),
                kind: (*kind).to_string(),
                change: SymbolDeltaKind::Removed,
            });
        }
    }
    out
}
