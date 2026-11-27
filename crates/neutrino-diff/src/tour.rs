//! Lightweight intra-repo call-graph / dependency ordering for pull request review tours.
//!
//! Prioritizes schema/model definitions before core logic before server/UI callers before tests.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use neutrino_proto::TourStep;

use crate::semantic;

fn tier_info(path: &str) -> (&'static str, u8) {
    let lower = path.to_ascii_lowercase();
    if lower.contains("test") || lower.ends_with("_test.rs") || lower.ends_with("tests.swift") {
        return ("Tests", 5);
    }
    if lower.ends_with(".toml")
        || lower.ends_with(".yml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".json")
        || lower.ends_with(".md")
        || lower.starts_with("scripts/")
    {
        return ("Config", 4);
    }
    if lower.contains("proto")
        || lower.contains("model")
        || lower.contains("schema")
        || lower.contains("types")
        || lower.ends_with(".sql")
    {
        return ("Model / Schema", 0);
    }
    if lower.starts_with("apps/")
        || lower.ends_with(".swift")
        || lower.contains("screen")
        || lower.contains("view")
        || lower.contains("panel")
    {
        return ("UI", 3);
    }
    if lower.contains("server")
        || lower.contains("neutrinod")
        || lower.contains("gravitond")
        || lower.contains("pulls")
        || lower.contains("handler")
        || lower.contains("main.rs")
    {
        return ("Server / CLI", 2);
    }
    ("Core", 1)
}

fn is_noise_ident(sym: &str) -> bool {
    sym.len() < 3
        || matches!(
            sym,
            "new" | "get" | "set" | "run" | "init" | "main" | "test" | "self" | "Self" | "body" | "from" | "into" | "default" | "String" | "Result"
        )
}

fn scan_identifiers(bytes: &[u8]) -> HashSet<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    let mut start = None;
    for (i, ch) in text.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            let tok = &text[s..i];
            if tok.len() >= 3 && !is_noise_ident(tok) {
                out.insert(tok.to_string());
            }
        }
    }
    if let Some(s) = start {
        let tok = &text[s..];
        if tok.len() >= 3 && !is_noise_ident(tok) {
            out.insert(tok.to_string());
        }
    }
    out
}

/// Compute a dependency-ordered review tour across changed `(path, old, new)` files.
pub fn compute_review_tour(files: &[(&str, &[u8], &[u8])]) -> Vec<TourStep> {
    if files.is_empty() {
        return Vec::new();
    }

    let n = files.len();
    let mut defines: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut idents: Vec<HashSet<String>> = vec![HashSet::new(); n];
    let mut tiers: Vec<(&'static str, u8)> = Vec::with_capacity(n);

    // Map defined symbol -> defining file indices.
    let mut symbol_owners: HashMap<String, Vec<usize>> = HashMap::new();

    for (idx, &(path, old, new)) in files.iter().enumerate() {
        let content = if new.is_empty() { old } else { new };
        tiers.push(tier_info(path));
        let decls = semantic::extract_decls(path, content);
        let mut def_set = BTreeSet::new();
        for d in decls {
            // Strip "impl " prefix if present so impl Foo matches Foo cleanly.
            let clean = d.symbol.strip_prefix("impl ").unwrap_or(&d.symbol).to_string();
            if !is_noise_ident(&clean) {
                def_set.insert(clean.clone());
                symbol_owners.entry(clean).or_default().push(idx);
            }
        }
        defines[idx] = def_set.into_iter().take(12).collect();
        idents[idx] = scan_identifiers(content);
    }

    // Build edges u -> v ("review u before v"): u defines a unique symbol used by v.
    let mut adj: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    let mut indegree: Vec<usize> = vec![0; n];
    let mut referenced_by: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];

    for v in 0..n {
        for tok in &idents[v] {
            if let Some(owners) = symbol_owners.get(tok)
                && owners.len() <= 2
            {
                for &u in owners {
                    if u != v && adj[u].insert(v) {
                        indegree[v] += 1;
                        referenced_by[u].insert(files[v].0.rsplit('/').next().unwrap_or(files[v].0).to_string());
                    }
                }
            }
        }
    }

    // Kahn's topological sort breaking ties by (tier_rank, path).
    let mut remaining: BTreeMap<(u8, &str, usize), ()> = BTreeMap::new();
    for i in 0..n {
        remaining.insert((tiers[i].1, files[i].0, i), ());
    }

    let mut order = Vec::with_capacity(n);
    while !remaining.is_empty() {
        let next_idx = remaining
            .keys()
            .find(|&&(_, _, idx)| indegree[idx] == 0)
            .copied()
            .or_else(|| remaining.keys().next().copied())
            .expect("non-empty remaining");
        remaining.remove(&next_idx);
        let u = next_idx.2;
        order.push(u);
        for &v in &adj[u] {
            indegree[v] = indegree[v].saturating_sub(1);
        }
    }

    order
        .into_iter()
        .map(|i| {
            let (tier, _) = tiers[i];
            let refs: Vec<String> = referenced_by[i].iter().cloned().collect();
            let reason = if !refs.is_empty() {
                format!("{tier} layer · referenced by {}", refs.join(", "))
            } else if !defines[i].is_empty() {
                format!("{tier} layer · defines {}", defines[i].iter().take(3).cloned().collect::<Vec<_>>().join(", "))
            } else {
                format!("{tier} layer")
            };
            TourStep {
                path: files[i].0.to_string(),
                tier: tier.to_string(),
                reason,
                defines: defines[i].clone(),
                referenced_by: refs,
            }
        })
        .collect()
}
