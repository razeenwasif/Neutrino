//! Browsing a commit's files: directory listings, file contents, the last
//! commit to touch each entry, and a summary of the whole tree.
//!
//! Everything reads objects only, so it works the same on bare repos.

use std::collections::{HashMap, HashSet};

use gix::ObjectId;
use gix::bstr::{BString, ByteSlice};
use gix::objs::tree::{EntryKind as GitKind, EntryMode};
use neutrino_proto::{BlobContent, EntryCommit, EntryKind, LanguageShare, RepoOverview, TreeEntry, TreeListing};

use crate::{Ctx, Result, fail, find_commit, split_message};

/// Files bigger than this aren't sent as text.
pub const MAX_BLOB_BYTES: u64 = 1_048_576;
/// Commits searched for the last change to each entry of a directory, at most.
const MAX_TREE_COMMITS: usize = 2_000;
/// Commits counted for the overview, at most.
const MAX_OVERVIEW_COMMITS: usize = 100_000;
/// Tree entries walked for the overview, at most.
const MAX_OVERVIEW_ENTRIES: usize = 200_000;
/// Languages listed in the overview, "Other" included.
const MAX_LANGUAGES: usize = 8;

/// The commit `rev` names, or HEAD's.
fn commit_at<'r>(repo: &'r gix::Repository, rev: Option<&str>) -> Result<gix::Commit<'r>> {
    find_commit(repo, rev.unwrap_or("HEAD"))
}

/// A path's components, without empty ones.
fn components(path: &str) -> Vec<&str> {
    path.split('/').filter(|c| !c.is_empty() && *c != ".").collect()
}

fn kind(mode: EntryMode) -> EntryKind {
    match mode.kind() {
        GitKind::Tree => EntryKind::Dir,
        GitKind::Blob => EntryKind::File,
        GitKind::BlobExecutable => EntryKind::Executable,
        GitKind::Link => EntryKind::Symlink,
        GitKind::Commit => EntryKind::Submodule,
    }
}

/// The directory at `path` in a commit's tree, or why there isn't one.
fn open_dir<'r>(repo: &'r gix::Repository, commit: &gix::Commit<'r>, path: &str) -> Result<gix::Tree<'r>> {
    let root = commit.tree().ctx("reading tree")?;
    let parts = components(path);
    if parts.is_empty() {
        return Ok(root);
    }
    let short = commit.id().to_hex_with_len(7).to_string();
    let Some(entry) = root.lookup_entry(parts).ctx(format!("looking up {path}"))? else {
        return fail(format!("no {path} in {short}"));
    };
    if !entry.mode().is_tree() {
        return fail(format!("{path} is not a directory in {short}"));
    }
    repo.find_tree(entry.object_id()).ctx("reading tree")
}

/// The id of the directory at `parts` in the tree `root`, if it is one.
fn dir_id(repo: &gix::Repository, root: ObjectId, parts: &[&str]) -> Result<Option<ObjectId>> {
    if parts.is_empty() {
        return Ok(Some(root));
    }
    let tree = repo.find_tree(root).ctx("reading tree")?;
    let entry = tree.lookup_entry(parts.iter().copied()).ctx("looking up a directory")?;
    Ok(entry.filter(|e| e.mode().is_tree()).map(|e| e.object_id()))
}

/// A directory's entries by name.
fn dir_entries(repo: &gix::Repository, dir: Option<ObjectId>) -> Result<HashMap<BString, (ObjectId, EntryMode)>> {
    let Some(dir) = dir else { return Ok(HashMap::new()) };
    let tree = repo.find_tree(dir).ctx("reading tree")?;
    let mut out = HashMap::new();
    for e in tree.iter() {
        let e = e.ctx("reading tree")?;
        out.insert(e.filename().to_owned(), (e.object_id(), e.mode()));
    }
    Ok(out)
}

fn blob_size(repo: &gix::Repository, id: ObjectId) -> Option<u64> {
    // A partial clone may not have the blob; then the size is unknown.
    repo.try_find_header(id).ok().flatten().map(|h| h.size())
}

/// Dirs and submodules first, then the rest, each by name ignoring case.
fn sort_entries(entries: &mut [TreeEntry]) {
    entries.sort_by(|a, b| {
        let group = |e: &TreeEntry| !matches!(e.kind, EntryKind::Dir | EntryKind::Submodule);
        (group(a), a.name.to_lowercase(), &a.name).cmp(&(group(b), b.name.to_lowercase(), &b.name))
    });
}

fn listing(repo: &gix::Repository, commit: &gix::Commit<'_>, path: &str) -> Result<TreeListing> {
    let dir = open_dir(repo, commit, path)?;
    let prefix = components(path).join("/");
    let mut entries = Vec::new();
    for e in dir.iter() {
        let e = e.ctx("reading tree")?;
        let name = e.filename().to_str_lossy().into_owned();
        let kind = kind(e.mode());
        let size = match kind {
            EntryKind::Dir | EntryKind::Submodule => None,
            _ => blob_size(repo, e.object_id()),
        };
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        entries.push(TreeEntry { name, path, kind, size });
    }
    sort_entries(&mut entries);
    Ok(TreeListing { commit: commit.id().to_string(), path: prefix, entries })
}

/// One directory of a commit's tree.
pub fn tree(repo: &gix::Repository, rev: Option<&str>, path: &str) -> Result<TreeListing> {
    let commit = commit_at(repo, rev)?;
    listing(repo, &commit, path)
}

/// The newest commit to change each entry of a directory, following first
/// parents. Entries not changed within the last 2,000 commits are left out.
pub fn tree_commits(repo: &gix::Repository, rev: Option<&str>, path: &str) -> Result<Vec<EntryCommit>> {
    let head = commit_at(repo, rev)?;
    let listed = listing(repo, &head, path)?;
    let parts = components(path);

    let mut pending: HashSet<BString> = listed.entries.iter().map(|e| BString::from(e.name.as_str())).collect();
    let mut found: HashMap<BString, EntryCommit> = HashMap::new();
    let mut id = head.id().detach();
    let mut this_dir = dir_id(repo, head.tree_id().ctx("reading tree")?.detach(), &parts)?;

    for _ in 0..MAX_TREE_COMMITS {
        if pending.is_empty() {
            break;
        }
        let commit = repo.find_commit(id).ctx("reading commit")?;
        let parent = commit.parent_ids().next().map(|p| p.detach());
        let parent_dir = match parent {
            Some(p) => dir_id(repo, repo.find_commit(p).ctx("reading parent")?.tree_id().ctx("reading tree")?.detach(), &parts)?,
            None => None,
        };
        if this_dir != parent_dir {
            let (now, before) = (dir_entries(repo, this_dir)?, dir_entries(repo, parent_dir)?);
            let changed: Vec<BString> = pending.iter().filter(|n| now.get(*n) != before.get(*n)).cloned().collect();
            if !changed.is_empty() {
                let (subject, _) = split_message(commit.message_raw_sloppy());
                let time = commit.time().map(|t| t.seconds).unwrap_or(0);
                for name in changed {
                    pending.remove(&name);
                    let entry = EntryCommit { name: name.to_str_lossy().into_owned(), sha: id.to_string(), subject: subject.clone(), time };
                    found.insert(name, entry);
                }
            }
        }
        match parent {
            Some(p) => (id, this_dir) = (p, parent_dir),
            None => break,
        }
    }

    Ok(listed.entries.iter().filter_map(|e| found.remove(e.name.as_bytes().as_bstr())).collect())
}

/// One file's contents at a commit. A symlink reads as its target.
pub fn blob(repo: &gix::Repository, rev: Option<&str>, path: &str) -> Result<BlobContent> {
    let commit = commit_at(repo, rev)?;
    let short = commit.id().to_hex_with_len(7).to_string();
    let parts = components(path);
    let clean = parts.join("/");
    if parts.is_empty() {
        return fail("no file named");
    }
    let root = commit.tree().ctx("reading tree")?;
    let Some(entry) = root.lookup_entry(parts).ctx(format!("looking up {clean}"))? else {
        return fail(format!("no {clean} in {short}"));
    };
    let mode = entry.mode();
    if !mode.is_blob_or_symlink() {
        return fail(format!("{clean} is not a file in {short}"));
    }
    let id = entry.object_id();
    if !repo.has_object(id) {
        return fail(format!(
            "the contents of {clean} aren't in this repository (a partial clone fetches them on demand, and Neutrino doesn't fetch)"
        ));
    }
    let size = repo.find_header(id).ctx("reading blob header")?.size();
    let mut out = BlobContent {
        commit: commit.id().to_string(),
        path: clean,
        size,
        binary: false,
        too_large: false,
        text: None,
        lines: 0,
        syntax: Vec::new(),
    };
    if size > MAX_BLOB_BYTES {
        out.too_large = true;
        return Ok(out);
    }
    let data = repo.find_blob(id).ctx("reading blob")?.take_data();
    if !mode.is_link() && neutrino_diff::is_binary(&data) {
        out.binary = true;
        return Ok(out);
    }
    out.lines = line_count(&data);
    out.syntax = neutrino_diff::syntax::highlight_lines(&out.path, &data);
    out.text = Some(String::from_utf8_lossy(&data).into_owned());
    Ok(out)
}

/// Lines in some text; a trailing newline doesn't start another.
fn line_count(data: &[u8]) -> u32 {
    let newlines = data.iter().filter(|b| **b == b'\n').count();
    let unterminated = !data.is_empty() && !data.ends_with(b"\n");
    (newlines + usize::from(unterminated)) as u32
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

/// The language a file is written in, by name. Prose, data and config are left out.
fn language(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "dockerfile" | "containerfile" => return Some("Dockerfile"),
        "makefile" | "gnumakefile" => return Some("Makefile"),
        _ => {}
    }
    if lower.starts_with("dockerfile.") {
        return Some("Dockerfile");
    }
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        return None;
    }
    let (_, ext) = lower.rsplit_once('.')?;
    Some(match ext {
        "rs" => "Rust",
        "swift" => "Swift",
        "py" | "pyi" => "Python",
        "js" | "mjs" | "cjs" | "jsx" => "JavaScript",
        "ts" | "mts" | "cts" | "tsx" => "TypeScript",
        "go" => "Go",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "inl" => "C++",
        "cs" => "C#",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "scala" => "Scala",
        "rb" => "Ruby",
        "php" => "PHP",
        "pl" | "pm" => "Perl",
        "sh" | "bash" | "zsh" | "fish" => "Shell",
        "ps1" | "psm1" => "PowerShell",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "scss" | "sass" => "SCSS",
        "lua" => "Lua",
        "zig" => "Zig",
        "hs" | "lhs" => "Haskell",
        "ml" | "mli" => "OCaml",
        "ex" | "exs" => "Elixir",
        "erl" | "hrl" => "Erlang",
        "clj" | "cljs" | "cljc" => "Clojure",
        "fs" | "fsi" | "fsx" => "F#",
        "dart" => "Dart",
        "r" => "R",
        "jl" => "Julia",
        "m" | "mm" => "Objective-C",
        "metal" => "Metal",
        "glsl" | "vert" | "frag" | "geom" | "tesc" | "tese" => "GLSL",
        "hlsl" => "HLSL",
        "wgsl" => "WGSL",
        "cu" | "cuh" => "CUDA",
        "sql" => "SQL",
        "nix" => "Nix",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "tex" | "sty" => "TeX",
        "mk" => "Makefile",
        "cmake" => "CMake",
        "dockerfile" => "Dockerfile",
        "asm" | "s" => "Assembly",
        _ => return None,
    })
}

/// Folders of other people's code, left out of the language count.
fn is_vendored(dir: &str) -> bool {
    matches!(dir, "vendor" | "node_modules" | "third_party")
}

/// How well a root file name works as the README; lower is better.
fn readme_rank(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    let (stem, ext) = lower.split_once('.').unwrap_or((&lower, ""));
    if stem != "readme" {
        return None;
    }
    Some(match ext {
        "md" => 0,
        "markdown" => 1,
        "" => 2,
        "rst" => 3,
        "txt" => 4,
        _ => 5,
    })
}

fn is_license(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["license", "licence", "copying"].iter().any(|p| lower.starts_with(p))
}

/// Commit and contributor counts, files, languages and the README and licence
/// of the tree at a commit.
pub fn overview(repo: &gix::Repository, rev: Option<&str>) -> Result<RepoOverview> {
    let commit = commit_at(repo, rev)?;
    let id = commit.id().detach();

    let mut commits = 0u32;
    let mut commits_capped = false;
    let mut emails: HashSet<String> = HashSet::new();
    for info in repo.rev_walk([id]).all().ctx("walking history")? {
        let info = info.ctx("walking history")?;
        if commits as usize == MAX_OVERVIEW_COMMITS {
            commits_capped = true;
            break;
        }
        commits += 1;
        let c = info.object().ctx("reading commit")?;
        if let Ok(author) = c.author() {
            emails.insert(author.email.trim().to_str_lossy().to_lowercase());
        }
    }

    let root = commit.tree_id().ctx("reading tree")?.detach();
    let (mut readme, mut license): (Option<(u8, String)>, Option<String>) = (None, None);
    let mut files = 0u32;
    let mut bytes: HashMap<&'static str, u64> = HashMap::new();
    let mut walked = 0usize;
    // Directories still to read, with their path and whether they're vendored.
    let mut stack: Vec<(ObjectId, String, bool)> = vec![(root, String::new(), false)];
    'walk: while let Some((dir, prefix, vendored)) = stack.pop() {
        let tree = repo.find_tree(dir).ctx("reading tree")?;
        for e in tree.iter() {
            if walked == MAX_OVERVIEW_ENTRIES {
                break 'walk;
            }
            walked += 1;
            let e = e.ctx("reading tree")?;
            let name = e.filename().to_str_lossy();
            let mode = e.mode();
            if mode.is_tree() {
                let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
                stack.push((e.object_id(), path, vendored || is_vendored(&name)));
                continue;
            }
            if !mode.is_blob_or_symlink() {
                continue;
            }
            files += 1;
            if prefix.is_empty() {
                if let Some(rank) = readme_rank(&name)
                    && readme.as_ref().is_none_or(|(best, path)| (rank, name.as_ref()) < (*best, path.as_str()))
                {
                    readme = Some((rank, name.to_string()));
                }
                if is_license(&name) && license.as_ref().is_none_or(|l| name.as_ref() < l.as_str()) {
                    license = Some(name.to_string());
                }
            }
            if vendored || !mode.is_blob() {
                continue;
            }
            if let Some(lang) = language(&name) {
                *bytes.entry(lang).or_default() += blob_size(repo, e.object_id()).unwrap_or(0);
            }
        }
    }

    let mut languages: Vec<LanguageShare> = bytes.into_iter().map(|(name, bytes)| LanguageShare { name: name.into(), bytes }).collect();
    languages.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    if languages.len() > MAX_LANGUAGES {
        let rest: u64 = languages.drain(MAX_LANGUAGES - 1..).map(|l| l.bytes).sum();
        languages.push(LanguageShare { name: "Other".into(), bytes: rest });
    }

    Ok(RepoOverview {
        commit: id.to_string(),
        commits,
        commits_capped,
        contributors: emails.len() as u32,
        files,
        languages,
        readme: readme.map(|(_, path)| path),
        license,
    })
}
