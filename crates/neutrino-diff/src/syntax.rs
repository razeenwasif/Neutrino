//! Tree-sitter syntax highlighting for source blobs and unified diffs.
//!
//! Compiles a curated set of static Tree-sitter grammars with zero runtime
//! grammar fetches and caches line spans per `(content_hash, Lang)`.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

use neutrino_proto::{SyntaxKind, SyntaxSpan};
use tree_sitter::{Node, Parser};

/// Blobs larger than this aren't parsed for syntax tokens.
const MAX_SYNTAX_BYTES: usize = 512 * 1024;
const SYNTAX_CACHE_CAP: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Swift,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Json,
    Toml,
    Yaml,
    Bash,
    Markdown,
}

impl Lang {
    pub fn for_path(path: &str) -> Option<Self> {
        let lower = path.to_ascii_lowercase();
        let name = lower.rsplit('/').next().unwrap_or(&lower);
        match name {
            "cargo.lock" | "podfile" => return Some(Self::Toml),
            "dockerfile" | "makefile" => return Some(Self::Bash),
            _ => {}
        }
        let ext = name.rsplit_once('.')?.1;
        match ext {
            "rs" => Some(Self::Rust),
            "swift" => Some(Self::Swift),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            "py" | "pyi" => Some(Self::Python),
            "json" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "yml" | "yaml" => Some(Self::Yaml),
            "sh" | "bash" | "zsh" => Some(Self::Bash),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    pub fn is_code(self) -> bool {
        matches!(self, Self::Rust | Self::Swift | Self::TypeScript | Self::Tsx | Self::JavaScript | Self::Python | Self::Bash)
    }

    pub fn ts_language(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Swift => tree_sitter_swift::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }
}

type CacheKey = (u64, Lang);
type SyntaxCacheMap = HashMap<CacheKey, Vec<Vec<SyntaxSpan>>>;
static SYNTAX_CACHE: LazyLock<Mutex<(SyntaxCacheMap, VecDeque<CacheKey>)>> =
    LazyLock::new(|| Mutex::new((HashMap::new(), VecDeque::new())));

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub fn parse_tree(lang: Lang, bytes: &[u8]) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    parser.parse(bytes, None)
}

/// Line `[start, end)` byte ranges inside `bytes`, excluding trailing `\r`/`\n`.
fn line_bounds(bytes: &[u8]) -> Vec<(usize, usize)> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pos = 0usize;
    for slice in bytes.split_inclusive(|&b| b == b'\n') {
        let mut len = slice.len();
        if slice.ends_with(b"\n") {
            len -= 1;
        }
        if slice[..len].ends_with(b"\r") {
            len -= 1;
        }
        out.push((pos, pos + len));
        pos += slice.len();
    }
    out
}

/// Highlight `bytes` according to the grammar inferred from `path`, returning one `Vec<SyntaxSpan>` per line.
pub fn highlight_lines(path: &str, bytes: &[u8]) -> Vec<Vec<SyntaxSpan>> {
    let Some(lang) = Lang::for_path(path) else {
        return Vec::new();
    };
    if bytes.is_empty() || bytes.len() > MAX_SYNTAX_BYTES || crate::is_binary(bytes) {
        return Vec::new();
    }

    let key = (fnv1a_64(bytes), lang);
    if let Some(cached) = SYNTAX_CACHE.lock().unwrap().0.get(&key).cloned() {
        return cached;
    }

    let bounds = line_bounds(bytes);
    let mut spans = vec![Vec::new(); bounds.len()];
    let Some(tree) = parse_tree(lang, bytes) else {
        return Vec::new();
    };

    collect_node_spans(lang, tree.root_node(), bytes, &bounds, &mut spans, None);

    // Compact adjacent contiguous spans with matching SyntaxKind on each line.
    for line in &mut spans {
        line.sort_by_key(|s| (s.start, s.len));
        let mut merged: Vec<SyntaxSpan> = Vec::with_capacity(line.len());
        for s in line.drain(..) {
            if s.len == 0 {
                continue;
            }
            if let Some(last) = merged.last_mut() {
                if s.start < last.start + last.len {
                    continue;
                }
                if last.kind == s.kind && last.start + last.len == s.start {
                    last.len += s.len;
                    continue;
                }
            }
            merged.push(s);
        }
        *line = merged;
    }

    let mut cache = SYNTAX_CACHE.lock().unwrap();
    if cache.0.insert(key, spans.clone()).is_none() {
        cache.1.push_back(key);
        while cache.1.len() > SYNTAX_CACHE_CAP {
            if let Some(old) = cache.1.pop_front() {
                cache.0.remove(&old);
            }
        }
    }
    spans
}

fn push_range(start_byte: usize, end_byte: usize, kind: SyntaxKind, bounds: &[(usize, usize)], out: &mut [Vec<SyntaxSpan>]) {
    if start_byte >= end_byte || bounds.is_empty() {
        return;
    }
    // Binary search for the first line whose end_byte > start_byte.
    let mut idx = bounds.partition_point(|&(_, lend)| lend <= start_byte);
    while idx < bounds.len() {
        let (lstart, lend) = bounds[idx];
        if lstart >= end_byte {
            break;
        }
        let s = start_byte.max(lstart);
        let e = end_byte.min(lend);
        if e > s {
            out[idx].push(SyntaxSpan {
                start: (s - lstart) as u32,
                len: (e - s) as u32,
                kind,
            });
        }
        idx += 1;
    }
}

fn whole_node_kind(lang: Lang, node: Node<'_>, bytes: &[u8]) -> Option<SyntaxKind> {
    let k = node.kind();
    if k.contains("comment") {
        return Some(SyntaxKind::Comment);
    }
    if matches!(
        k,
        "string_literal"
            | "raw_string_literal"
            | "char_literal"
            | "string"
            | "line_string_literal"
            | "multi_line_string_literal"
            | "template_string"
            | "raw_string"
            | "regex"
            | "string_scalar"
            | "single_quote_scalar"
            | "double_quote_scalar"
    ) {
        return Some(SyntaxKind::String);
    }
    if matches!(k, "integer_literal" | "float_literal" | "number" | "integer" | "float" | "hex_literal") {
        return Some(SyntaxKind::Number);
    }
    if matches!(k, "attribute_item" | "inner_attribute_item" | "attribute" | "decorator" | "shebang" | "directive") {
        return Some(SyntaxKind::Attribute);
    }
    if matches!(k, "boolean_literal" | "true" | "false" | "null" | "nil" | "none" | "self") {
        return Some(SyntaxKind::Constant);
    }
    if lang == Lang::Markdown && matches!(k, "atx_heading" | "setext_heading") {
        return Some(SyntaxKind::Keyword);
    }
    if lang == Lang::Markdown && matches!(k, "code_span" | "fenced_code_block") {
        return Some(SyntaxKind::String);
    }
    let _ = bytes;
    None
}

fn leaf_kind(lang: Lang, node: Node<'_>, parent: Option<Node<'_>>, bytes: &[u8]) -> Option<SyntaxKind> {
    let k = node.kind();
    let text = node.utf8_text(bytes).ok()?;
    if text.trim().is_empty() {
        return None;
    }

    if let Some(whole) = whole_node_kind(lang, node, bytes) {
        return Some(whole);
    }

    if matches!(k, "bare_key" | "dotted_key" | "property_name" | "property_identifier" | "shorthand_field_identifier" | "field_identifier") {
        return Some(SyntaxKind::Property);
    }
    if k == "type_identifier" || k == "primitive_type" {
        return Some(SyntaxKind::Type);
    }
    if matches!(k, "lifetime" | "mutable_specifier") {
        return Some(SyntaxKind::Keyword);
    }

    if matches!(k, "identifier" | "simple_identifier" | "word" | "name") {
        if let Some(p) = parent {
            let pk = p.kind();
            if matches!(
                pk,
                "function_item"
                    | "function_declaration"
                    | "function_definition"
                    | "method_definition"
                    | "function_signature_item"
                    | "call_expression"
                    | "method_call_expression"
                    | "macro_invocation"
                    | "command_name"
            ) && (p.child_by_field_name("name").map(|c| c.id()) == Some(node.id())
                || p.child_by_field_name("function").map(|c| c.id()) == Some(node.id())
                || p.child_by_field_name("macro").map(|c| c.id()) == Some(node.id())
                || pk == "command_name")
            {
                return Some(SyntaxKind::Function);
            }
            if matches!(pk, "field_declaration" | "pair" | "key_value_pair" | "member_expression" | "field_expression" | "navigation_suffix")
                && (p.child_by_field_name("name").map(|c| c.id()) == Some(node.id())
                    || p.child_by_field_name("field").map(|c| c.id()) == Some(node.id())
                    || p.child_by_field_name("property").map(|c| c.id()) == Some(node.id())
                    || p.child_by_field_name("key").map(|c| c.id()) == Some(node.id()))
            {
                return Some(SyntaxKind::Property);
            }
            if matches!(pk, "struct_item" | "enum_item" | "trait_item" | "class_declaration" | "struct_declaration" | "protocol_declaration" | "class_definition" | "interface_declaration" | "type_alias_declaration")
                && p.child_by_field_name("name").map(|c| c.id()) == Some(node.id())
            {
                return Some(SyntaxKind::Type);
            }
        }
        if matches!(text, "self" | "Self" | "true" | "false" | "nil" | "null" | "None" | "True" | "False" | "super" | "crate") {
            return Some(SyntaxKind::Constant);
        }
        let mut chars = text.chars();
        if let Some(first) = chars.next()
            && first.is_ascii_uppercase()
        {
            if text.len() > 1 && text.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                return Some(SyntaxKind::Constant);
            }
            return Some(SyntaxKind::Type);
        }
        return None;
    }

    if !node.is_named() {
        if text.chars().all(|c| c.is_ascii_alphabetic() || c == '_') {
            return Some(match text {
                "true" | "false" | "nil" | "null" | "self" | "Self" | "None" | "True" | "False" => SyntaxKind::Constant,
                _ => SyntaxKind::Keyword,
            });
        }
        if matches!(
            text,
            "->" | "=>" | "::" | "==" | "!=" | "<=" | ">=" | "&&" | "||" | "+=" | "-=" | "*=" | "/=" | "+" | "-" | "*" | "/" | "%" | "=" | "&" | "|" | "^" | "!" | "?" | ".." | "..="
        ) {
            return Some(SyntaxKind::Operator);
        }
    }

    None
}

fn collect_node_spans(
    lang: Lang,
    node: Node<'_>,
    bytes: &[u8],
    bounds: &[(usize, usize)],
    out: &mut [Vec<SyntaxSpan>],
    parent: Option<Node<'_>>,
) {
    if let Some(kind) = whole_node_kind(lang, node, bytes) {
        push_range(node.start_byte(), node.end_byte(), kind, bounds, out);
        return;
    }
    // Special handling for JSON / YAML object keys ("foo": 123).
    if (lang == Lang::Json && node.kind() == "pair") || (lang == Lang::Yaml && node.kind() == "block_mapping_pair") {
        if let Some(k) = node.child_by_field_name("key") {
            push_range(k.start_byte(), k.end_byte(), SyntaxKind::Property, bounds, out);
        }
        if let Some(v) = node.child_by_field_name("value") {
            collect_node_spans(lang, v, bytes, bounds, out, Some(node));
        }
        return;
    }
    if node.child_count() == 0 {
        if let Some(kind) = leaf_kind(lang, node, parent, bytes) {
            push_range(node.start_byte(), node.end_byte(), kind, bounds, out);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_node_spans(lang, child, bytes, bounds, out, Some(node));
    }
}
