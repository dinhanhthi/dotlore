//! Staging-private names and the committed include-list (`.dotloreproject`).
//!
//! Private names live in a staging repo and must never be mirrored into a
//! live root, nor applied from a peer's tree. The include-list is a
//! generation-addressed map of explicit entries. Tombstones participate in
//! per-key merge; a `Removed` key under a still-tracked parent punches a hole
//! in that parent (the path stops syncing, live bytes stay).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};

use crate::config::{self, Config};

/// The committed ignore file in a staging worktree.
pub const IGNORE_FILE: &str = ".dotloreignore";

/// The committed project file in a staging worktree.
pub const PROJECT_FILE: &str = ".dotloreproject";

/// Default include-list patterns for a project folder (not an agent home).
pub const DEFAULT_PATTERNS: &[&str] = &[
    ".env",
    ".env.local",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    "AGENTS.override.md",
    "AGENT.md",
    "GEMINI.md",
    ".rules",
    ".cursorrules",
    ".windsurfrules",
    ".clinerules",
    ".roorules",
    ".mcp.json",
    ".worktreeinclude",
    "opencode.json",
    "opencode.jsonc",
    ".aider.conf.yml",
    "CONVENTIONS.md",
    ".cursorignore",
    ".geminiignore",
    ".aiderignore",
    ".rooignore",
    ".aiignore",
    ".github/copilot-instructions.md",
    "docs/",
    ".claude/",
    ".codex/",
    ".cursor/",
    ".gemini/",
    ".agents/",
    ".opencode/",
    ".continue/",
    ".junie/",
    ".roo/",
    ".cline/",
    ".clinerules/",
    ".devin/",
    ".windsurf/",
    ".kiro/steering/",
    ".kiro/skills/",
    ".kiro/agents/",
    ".kiro/hooks/",
    ".github/instructions/",
    ".github/prompts/",
    ".github/agents/",
    ".github/skills/",
];

/// Per-agent include-list. Key is the folder `file_name` with the leading
/// dot stripped (`~/.claude` → `claude`, `~/.config/opencode` → `opencode`).
pub const AGENT_PATTERNS: &[(&str, &[&str])] = &[
    (
        "claude",
        &[
            "settings.json",
            "settings.local.json",
            "CLAUDE.md",
            "keybindings.json",
            "statusline-command.sh",
            "agents/",
            "skills/",
            "commands/",
            "rules/",
            "hooks/",
            "scripts/",
            "output-styles/",
            "workflows/",
            "themes/",
            "plugins/installed_plugins.json",
            "plugins/known_marketplaces.json",
            "plugins/blocklist.json",
        ],
    ),
    (
        "codex",
        &[
            "config.toml",
            "AGENTS.md",
            "rules/",
            "hooks.json",
            "skills/",
            "prompts/",
        ],
    ),
    (
        "cursor",
        &[
            "cli-config.json",
            "mcp.json",
            "AGENTS.md",
            "rules/",
            "skills/",
            "skills-cursor/",
            "agents/",
            "commands/",
            "hooks.json",
            "hooks/",
        ],
    ),
    (
        "gemini",
        &[
            "settings.json",
            "GEMINI.md",
            "commands/",
            "agents/",
            "skills/",
            "extensions/",
        ],
    ),
    (
        "opencode",
        &[
            "opencode.json",
            "opencode.jsonc",
            "AGENTS.md",
            "tui.json",
            "agent/",
            "agents/",
            "command/",
            "commands/",
            "plugins/",
            "skills/",
        ],
    ),
    (
        "continue",
        &[
            "config.yaml",
            "rules/",
            "skills/",
            "assistants/",
            "prompts/",
        ],
    ),
    (
        "junie",
        &["AGENTS.md", "guidelines.md", "playbook.md", "rules/"],
    ),
    (
        "kiro",
        &["settings/", "steering/", "skills/", "agents/", "hooks/"],
    ),
    ("roo", &["rules/", "skills/"]),
    ("cline", &["rules/", "skills/"]),
    ("windsurf", &["memories/global_rules.md"]),
];

/// Fallback for an agent folder with no `AGENT_PATTERNS` row.
pub const GENERIC_AGENT_PATTERNS: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "settings.json",
    "config.json",
    "config.toml",
    "agents/",
    "skills/",
    "commands/",
    "rules/",
    "hooks/",
    "prompts/",
];

/// Gitignore text written into each project's `.dotloreignore` at add time.
///
/// Agent-state rules are root-anchored with a leading `/` so the same text
/// can go into every project without matching nested names like `docs/plans/`.
pub const DEFAULT_NEVER_IGNORE: &str = "\
/.credentials.json
/auth.json
/.claude.json
/projects/
/sessions/
/history.jsonl
/file-history/
/plans/
/debug/
/paste-cache/
/image-cache/
/uploads/
/session-env/
/tasks/
/shell-snapshots/
/backups/
/feedback-bundles/
/feedback/drafts/
/usage-data/
/stats-cache.json
/remote-settings.json
/policy-limits.json*
/cache/
/jobs/
/daemon/
/.trash/
/todos/
/statsig/
/logs/
/chats/
/.gemini/tmp/
/agent-memory-local/
/.aider.input.history
/.aider.chat.history.md
/.aider.llm.history
/.aider.tags.cache.v*
gemini-debug.log
.gemini-clipboard/
.DS_Store
*.log
*.tmp
*.bak
*.cache
Thumbs.db
node_modules/
__pycache__/
.venv/
";

/// Size ceilings. Defaults live here so `seed` compiles; T3.7 enforces them.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_file_bytes: u64,
    pub max_seed_folder_bytes: u64,
}

impl Limits {
    /// Fallback when `Config.max_file_mb` is `None`.
    pub const DEFAULT_MAX_FILE_MB: u64 = 50;
    /// Fallback when `Config.max_seed_folder_mb` is `None`.
    pub const DEFAULT_MAX_SEED_FOLDER_MB: u64 = 200;

    /// Convert config megabyte options into byte ceilings.
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            max_file_bytes: mb_to_bytes(cfg.max_file_mb.unwrap_or(Self::DEFAULT_MAX_FILE_MB)),
            max_seed_folder_bytes: mb_to_bytes(
                cfg.max_seed_folder_mb
                    .unwrap_or(Self::DEFAULT_MAX_SEED_FOLDER_MB),
            ),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: mb_to_bytes(Self::DEFAULT_MAX_FILE_MB),
            max_seed_folder_bytes: mb_to_bytes(Self::DEFAULT_MAX_SEED_FOLDER_MB),
        }
    }
}

fn mb_to_bytes(mb: u64) -> u64 {
    mb.saturating_mul(1024 * 1024)
}

/// A pattern `seed` refused. T3.7 reports directories over the folder limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub rel: String,
    pub bytes: u64,
}

/// Catalog ids in dropdown order: `projects`, each [`AGENT_PATTERNS`] key, then `other`.
const CATALOGS: &[(&str, &str)] = &[
    ("projects", "Projects"),
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("cursor", "Cursor"),
    ("gemini", "Gemini"),
    ("opencode", "OpenCode"),
    ("continue", "Continue"),
    ("junie", "Junie"),
    ("kiro", "Kiro"),
    ("roo", "Roo"),
    ("cline", "Cline"),
    ("windsurf", "Windsurf"),
    ("other", "Other agents"),
];

/// Builtin include-list lines for a catalog id.
///
/// `projects` is [`DEFAULT_PATTERNS`], each agent id is that [`AGENT_PATTERNS`]
/// slice, and `other` is [`GENERIC_AGENT_PATTERNS`]. Any other id returns `None`.
pub fn builtin_lines(catalog: &str) -> Option<&'static [&'static str]> {
    if catalog_label(catalog).is_none() {
        return None;
    }
    match catalog {
        "projects" => Some(DEFAULT_PATTERNS),
        "other" => Some(GENERIC_AGENT_PATTERNS),
        id => AGENT_PATTERNS
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, lines)| *lines),
    }
}

/// Display label for a catalog id. Any other id returns `None`.
pub fn catalog_label(catalog: &str) -> Option<&'static str> {
    CATALOGS
        .iter()
        .find(|(id, _)| *id == catalog)
        .map(|(_, label)| *label)
}

/// Include-list patterns for `root`. Agent folders never read `cfg.default_patterns`.
///
/// An agent catalog uses `cfg.agent_patterns` when that id is present, including
/// an empty list. A missing key uses [`builtin_lines`].
pub fn patterns_for(root: &config::Root, home_dir: &Path, cfg: &Config) -> Vec<String> {
    if root.is_agent(home_dir) {
        let key = root
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.trim_start_matches('.'))
            .unwrap_or("");
        let catalog = if AGENT_PATTERNS.iter().any(|(id, _)| *id == key) {
            key
        } else {
            "other"
        };
        if let Some(patterns) = cfg.agent_patterns.get(catalog) {
            return patterns.clone();
        }
        return owned(builtin_lines(catalog).unwrap_or(&[]));
    }
    match &cfg.default_patterns {
        Some(p) => p.clone(),
        None => owned(DEFAULT_PATTERNS),
    }
}

fn owned(patterns: &[&str]) -> Vec<String> {
    patterns.iter().map(|s| (*s).to_string()).collect()
}

/// Seed an include-list from root-anchored patterns. No glob, no recursion.
///
/// Each pattern is included at `gen: 1, Tracked` when it exists, its kind
/// matches the trailing slash, no component is a symlink, and the ignore
/// matcher (including ancestor rules) does not exclude it. A directory
/// whose ignore-aware size exceeds `limits.max_seed_folder_bytes` is
/// returned in `Skipped` and not seeded.
pub fn seed(
    root: &Path,
    patterns: &[String],
    ignore_text: &str,
    limits: Limits,
) -> Result<(ProjectFile, Vec<Skipped>)> {
    let ignore = build_ignore(root, ignore_text)?;
    let mut file = ProjectFile::default();
    let mut skipped = Vec::new();
    for pattern in patterns {
        let Some(key) = accept_pattern(root, pattern, &ignore)? else {
            continue;
        };
        if key.ends_with('/') {
            let rel = Path::new(key.trim_end_matches('/'));
            let measured = crate::mirror::measure_tree(root, rel, ignore_text, limits)?;
            if measured.bytes > limits.max_seed_folder_bytes {
                skipped.push(Skipped {
                    rel: key,
                    bytes: measured.bytes,
                });
                continue;
            }
        }
        file.entries.insert(
            key,
            EntryRecord {
                gen: 1,
                state: State::Tracked,
            },
        );
    }
    Ok((file, skipped))
}

fn build_ignore(root: &Path, ignore_text: &str) -> Result<Gitignore> {
    let mut b = GitignoreBuilder::new(root);
    for line in ignore_text.lines() {
        b.add_line(None, line)?;
    }
    Ok(b.build()?)
}

/// No-follow walk of each path component. Kind must match a trailing `/`.
fn accept_pattern(root: &Path, pattern: &str, ignore: &Gitignore) -> Result<Option<String>> {
    let wants_dir = pattern.ends_with('/');
    let rel = pattern.trim_end_matches('/');
    if rel.is_empty() || !plain_rel(Path::new(rel)) {
        return Ok(None);
    }
    let mut acc = PathBuf::new();
    let comps: Vec<_> = Path::new(rel).components().collect();
    for (i, component) in comps.iter().enumerate() {
        let name = match component {
            Component::Normal(s) => s,
            _ => return Ok(None),
        };
        acc.push(name);
        let full = root.join(&acc);
        let md = match fs::symlink_metadata(&full) {
            Ok(md) => md,
            Err(_) => return Ok(None),
        };
        if md.file_type().is_symlink() {
            return Ok(None);
        }
        let last = i + 1 == comps.len();
        if last {
            if wants_dir && !md.is_dir() {
                return Ok(None);
            }
            if !wants_dir && !md.is_file() {
                return Ok(None);
            }
        } else if !md.is_dir() {
            return Ok(None);
        }
    }
    if ignore
        .matched_path_or_any_parents(&acc, wants_dir)
        .is_ignore()
    {
        return Ok(None);
    }
    Ok(Some(if wants_dir {
        format!("{rel}/")
    } else {
        rel.to_string()
    }))
}

/// True for names that are staging-private.
///
/// Does not include `.DS_Store`: `delete_stale` must still prune a stray
/// Finder file from the worktree. The daemon adds that name itself.
pub fn staging_private(name: &str) -> bool {
    name == ".git"
        || name == IGNORE_FILE
        || name == PROJECT_FILE
        || name.contains(".conflict-")
        || name.ends_with(".dotlore-tmp")
}

/// Whether an include-list key is live or a merge tombstone.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Tracked,
    Removed,
}

/// One explicit include-list key and its merge generation.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct EntryRecord {
    pub gen: u64,
    pub state: State,
}

/// The committed `.dotloreproject` file: a generation-addressed include-list.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct ProjectFile {
    pub entries: BTreeMap<String, EntryRecord>,
}

/// Tracked explicit entries plus Removed keys that punch a hole in a parent.
#[derive(Clone, Debug, Default)]
pub struct EntryList {
    keys: BTreeSet<String>,
    excluded: BTreeSet<String>,
}

/// Parse a `.dotloreproject` body. Every key must be a plain relative path.
pub fn parse(bytes: &[u8]) -> Result<ProjectFile> {
    let file: ProjectFile = serde_json::from_slice(bytes)?;
    for key in file.entries.keys() {
        if !plain_rel(Path::new(key)) {
            bail!("project entry is not a plain relative path: {key:?}");
        }
    }
    Ok(file)
}

/// Per-key union: higher `gen` wins; on a tie, `Removed` beats `Tracked`.
pub fn merge(a: &ProjectFile, b: &ProjectFile) -> ProjectFile {
    let mut entries = a.entries.clone();
    for (key, theirs) in &b.entries {
        match entries.get(key) {
            None => {
                entries.insert(key.clone(), theirs.clone());
            }
            Some(ours) => {
                let keep = pick(ours, theirs).clone();
                entries.insert(key.clone(), keep);
            }
        }
    }
    ProjectFile { entries }
}

/// Read `<dir>/.dotloreproject`, or an empty list when the file is absent.
pub fn read(dir: &Path) -> Result<ProjectFile> {
    let path = dir.join(PROJECT_FILE);
    match fs::read(&path) {
        Ok(bytes) => parse(&bytes).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(ProjectFile::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `<dir>/.dotloreproject` from a parsed include-list.
pub fn write(dir: &Path, file: &ProjectFile) -> Result<()> {
    let path = dir.join(PROJECT_FILE);
    fs::write(&path, file.to_bytes()?).with_context(|| format!("writing {}", path.display()))
}

impl ProjectFile {
    /// Canonical JSON. `BTreeMap` keeps the bytes identical across devices.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Explicit `Tracked` keys, plus `Removed` keys that punch a hole in a parent.
    pub fn tracked(&self) -> EntryList {
        let keys: BTreeSet<String> = self
            .entries
            .iter()
            .filter(|(_, rec)| rec.state == State::Tracked)
            .map(|(k, _)| k.clone())
            .collect();
        let excluded = self
            .entries
            .iter()
            .filter(|(k, rec)| {
                rec.state == State::Removed && covering_key(&keys, Path::new(k)).is_some()
            })
            .map(|(k, _)| k.clone())
            .collect();
        EntryList { keys, excluded }
    }

    /// Tombstone `rel`. An inherited path under a tracked directory becomes a
    /// hole in that parent. `as_directory` picks the key form for a new hole;
    /// `None` defaults to a file key.
    pub fn untrack(&mut self, rel: &Path) -> Result<()> {
        self.untrack_as(rel, None)
    }

    pub fn untrack_as(&mut self, rel: &Path, as_directory: Option<bool>) -> Result<()> {
        if let Some(key) = self.explicit_key(rel) {
            if self.entries[&key].state == State::Removed {
                return Ok(());
            }
            self.tombstone(key);
            return Ok(());
        }
        if covering_key(&self.tracked().keys, rel).is_some() {
            let base = path_key(rel);
            if base.is_empty() {
                bail!("cannot untrack the project root");
            }
            let key = if as_directory.unwrap_or(false) {
                dir_key(&base)
            } else {
                base
            };
            self.tombstone(key);
            return Ok(());
        }
        bail!(
            "cannot untrack {}: not an explicit include entry",
            rel.display()
        );
    }

    fn tombstone(&mut self, key: String) {
        let gen = self.max_gen().saturating_add(1);
        self.entries.insert(
            key,
            EntryRecord {
                gen,
                state: State::Removed,
            },
        );
    }

    fn explicit_key(&self, rel: &Path) -> Option<String> {
        let key = path_key(rel);
        if self.entries.contains_key(&key) {
            return Some(key);
        }
        let dir = dir_key(&key);
        if self.entries.contains_key(&dir) {
            return Some(dir);
        }
        None
    }

    fn max_gen(&self) -> u64 {
        self.entries.values().map(|e| e.gen).max().unwrap_or(0)
    }

    /// Other explicit tracked keys that overlap `key` (parent or child).
    pub fn covering_keys(&self, key: &str) -> Vec<String> {
        self.tracked()
            .keys
            .iter()
            .filter(|k| *k != key && keys_overlap(k, key))
            .cloned()
            .collect()
    }
}

impl EntryList {
    /// True when `rel` equals a tracked entry or sits under a tracked directory
    /// and is not carved out by a more specific exclude. An explicit tracked
    /// key wins over an excluded ancestor (re-include).
    pub fn contains_rel(&self, rel: &Path) -> bool {
        if self.is_explicit(rel) {
            return true;
        }
        if self.is_excluded(rel) {
            return false;
        }
        covering_key(&self.keys, rel).is_some()
    }

    fn is_excluded(&self, rel: &Path) -> bool {
        let key = path_key(rel);
        self.excluded.contains(&key)
            || self.excluded.contains(&dir_key(&key))
            || covering_key(&self.excluded, rel).is_some()
    }

    /// True when a tracked entry lives strictly under `rel` (ancestor walk).
    pub fn has_tracked_descendant(&self, rel: &Path) -> bool {
        let ancestor = path_key(rel);
        self.keys.iter().any(|entry| is_under(entry, &ancestor))
    }

    /// True when `rel` is itself an explicit tracked key (file or directory).
    pub fn is_explicit(&self, rel: &Path) -> bool {
        let key = path_key(rel);
        self.keys.contains(&key) || self.keys.contains(&dir_key(&key))
    }
}

fn pick<'a>(a: &'a EntryRecord, b: &'a EntryRecord) -> &'a EntryRecord {
    if a.gen > b.gen {
        a
    } else if b.gen > a.gen {
        b
    } else if a.state == State::Removed {
        a
    } else if b.state == State::Removed {
        b
    } else {
        a
    }
}

/// Same rule as `engine::live_root_path`: no absolute, no `..`, no empty.
fn plain_rel(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

fn path_key(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn dir_key(key: &str) -> String {
    if key.ends_with('/') {
        key.to_string()
    } else {
        format!("{key}/")
    }
}

fn covering_key(keys: &BTreeSet<String>, rel: &Path) -> Option<String> {
    for anc in rel.ancestors().skip(1) {
        if anc.as_os_str().is_empty() {
            break;
        }
        let dir = dir_key(&path_key(anc));
        if keys.contains(&dir) {
            return Some(dir);
        }
    }
    None
}

fn keys_overlap(a: &str, b: &str) -> bool {
    let a_base = a.trim_end_matches('/');
    let b_base = b.trim_end_matches('/');
    (a.ends_with('/') && (b_base == a_base || is_under(b, a_base)))
        || (b.ends_with('/') && (a_base == b_base || is_under(a, b_base)))
}

fn is_under(entry: &str, ancestor: &str) -> bool {
    let entry = entry.trim_end_matches('/');
    if ancestor.is_empty() {
        !entry.is_empty()
    } else {
        entry.starts_with(&format!("{ancestor}/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pf(pairs: &[(&str, u64, State)]) -> ProjectFile {
        ProjectFile {
            entries: pairs
                .iter()
                .map(|(k, g, s)| ((*k).to_string(), EntryRecord { gen: *g, state: *s }))
                .collect(),
        }
    }

    #[test]
    fn parse_rejects_a_non_plain_entry_path() {
        let rec = r#"{"gen":1,"state":"tracked"}"#;
        for key in ["/abs", "../x", ""] {
            let json = format!(r#"{{"entries":{{"{key}":{rec}}}}}"#);
            assert!(
                parse(json.as_bytes()).is_err(),
                "accepted non-plain key {key:?}"
            );
        }
    }

    #[test]
    fn merge_keeps_the_higher_generation() {
        let older = pf(&[("notes.md", 1, State::Tracked)]);
        let newer = pf(&[("notes.md", 4, State::Removed)]);
        let merged = merge(&older, &newer);
        assert_eq!(merged.entries["notes.md"].gen, 4);
        assert_eq!(merged.entries["notes.md"].state, State::Removed);

        let newer_track = pf(&[("notes.md", 5, State::Tracked)]);
        let older_remove = pf(&[("notes.md", 2, State::Removed)]);
        let merged = merge(&newer_track, &older_remove);
        assert_eq!(merged.entries["notes.md"].gen, 5);
        assert_eq!(merged.entries["notes.md"].state, State::Tracked);
    }

    #[test]
    fn merge_prefers_removal_on_an_equal_generation() {
        let tracked = pf(&[("notes.md", 3, State::Tracked)]);
        let removed = pf(&[("notes.md", 3, State::Removed)]);
        assert_eq!(
            merge(&tracked, &removed).entries["notes.md"].state,
            State::Removed
        );
        assert_eq!(
            merge(&removed, &tracked).entries["notes.md"].state,
            State::Removed
        );
    }

    #[test]
    fn merge_is_symmetric_and_byte_identical_in_both_orders() {
        let a = pf(&[
            ("a.md", 1, State::Tracked),
            ("b.md", 3, State::Removed),
            ("c.md", 2, State::Tracked),
        ]);
        let b = pf(&[
            ("a.md", 2, State::Removed),
            ("b.md", 3, State::Tracked),
            ("d.md", 1, State::Tracked),
        ]);
        assert_eq!(
            merge(&a, &b).to_bytes().unwrap(),
            merge(&b, &a).to_bytes().unwrap()
        );
    }

    #[test]
    fn an_entry_matches_its_own_path_and_anything_under_a_directory_entry() {
        let list = pf(&[
            ("CLAUDE.md", 1, State::Tracked),
            ("docs/", 1, State::Tracked),
        ])
        .tracked();
        assert!(list.contains_rel(Path::new("CLAUDE.md")));
        assert!(!list.contains_rel(Path::new("CLAUDE.md/extra")));
        assert!(list.contains_rel(Path::new("docs")));
        assert!(list.contains_rel(Path::new("docs/")));
        assert!(list.contains_rel(Path::new("docs/foo.md")));
        assert!(list.contains_rel(Path::new("docs/a/b")));
        assert!(!list.contains_rel(Path::new("other.md")));
        assert!(list.is_explicit(Path::new("CLAUDE.md")));
        assert!(list.is_explicit(Path::new("docs")));
        assert!(!list.is_explicit(Path::new("docs/foo.md")));
    }

    #[test]
    fn an_ancestor_is_traversable_without_being_tracked() {
        let list = pf(&[(".github/copilot-instructions.md", 1, State::Tracked)]).tracked();
        assert!(!list.contains_rel(Path::new(".github")));
        assert!(!list.is_explicit(Path::new(".github")));
        assert!(list.has_tracked_descendant(Path::new(".github")));
        assert!(list.contains_rel(Path::new(".github/copilot-instructions.md")));
        assert!(list.is_explicit(Path::new(".github/copilot-instructions.md")));
        assert!(!list.contains_rel(Path::new(".github/other.md")));
        assert!(!list.has_tracked_descendant(Path::new(".github/copilot-instructions.md")));
    }

    #[test]
    fn overlapping_explicit_entries_keep_independent_coverage() {
        let mut parent_removed = pf(&[
            ("docs/", 1, State::Tracked),
            ("docs/readme.md", 1, State::Tracked),
        ]);
        parent_removed.untrack(Path::new("docs")).unwrap();
        let after_parent = parent_removed.tracked();
        assert!(after_parent.contains_rel(Path::new("docs/readme.md")));
        assert!(after_parent.is_explicit(Path::new("docs/readme.md")));
        assert!(!after_parent.contains_rel(Path::new("docs/other.md")));
        assert!(!after_parent.is_explicit(Path::new("docs")));

        let mut child_removed = pf(&[
            ("docs/", 1, State::Tracked),
            ("docs/readme.md", 1, State::Tracked),
        ]);
        child_removed.untrack(Path::new("docs/readme.md")).unwrap();
        let after_child = child_removed.tracked();
        assert!(
            !after_child.contains_rel(Path::new("docs/readme.md")),
            "untracking the child punches a hole in the parent"
        );
        assert!(!after_child.is_explicit(Path::new("docs/readme.md")));
        assert!(after_child.is_explicit(Path::new("docs")));
        assert!(after_child.contains_rel(Path::new("docs/other.md")));

        let parent = pf(&[("docs/", 1, State::Tracked)]);
        let child = pf(&[("docs/readme.md", 2, State::Tracked)]);
        let ab = merge(&parent, &child);
        let ba = merge(&child, &parent);
        assert_eq!(ab.to_bytes().unwrap(), ba.to_bytes().unwrap());
        let coverage = ab.tracked();
        assert!(coverage.contains_rel(Path::new("docs/other.md")));
        assert!(coverage.contains_rel(Path::new("docs/readme.md")));
        assert!(coverage.is_explicit(Path::new("docs")));
        assert!(coverage.is_explicit(Path::new("docs/readme.md")));
    }

    #[test]
    fn untracking_an_inherited_only_path_punches_a_hole() {
        let mut file = pf(&[("docs/", 1, State::Tracked)]);
        file.untrack(Path::new("docs/foo.md")).unwrap();
        assert_eq!(file.entries["docs/foo.md"].state, State::Removed);
        assert_eq!(file.entries["docs/"].state, State::Tracked);
        let list = file.tracked();
        assert!(!list.contains_rel(Path::new("docs/foo.md")));
        assert!(list.contains_rel(Path::new("docs/other.md")));
        file.untrack(Path::new("docs/foo.md")).unwrap();
        assert_eq!(file.entries["docs/foo.md"].state, State::Removed);
    }

    #[test]
    fn untracking_an_inherited_directory_writes_a_trailing_slash_key() {
        let mut file = pf(&[("docs/", 1, State::Tracked)]);
        file.untrack_as(Path::new("docs/secret"), Some(true))
            .unwrap();
        assert_eq!(file.entries["docs/secret/"].state, State::Removed);
        assert!(
            !file.entries.contains_key("docs/secret"),
            "a file-key hole would leave children under docs/secret/ covered"
        );
        assert_eq!(file.entries["docs/"].state, State::Tracked);
        let list = file.tracked();
        assert!(!list.contains_rel(Path::new("docs/secret")));
        assert!(!list.contains_rel(Path::new("docs/secret/foo.md")));
        assert!(list.contains_rel(Path::new("docs/other.md")));
    }

    fn put(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn tracked_keys(file: &ProjectFile) -> Vec<String> {
        file.entries
            .iter()
            .filter(|(_, rec)| rec.state == State::Tracked)
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn default_pattern_strings() -> Vec<String> {
        DEFAULT_PATTERNS.iter().map(|s| (*s).to_string()).collect()
    }

    fn staged_files(root: &Path, ignore: &str) -> Vec<String> {
        let td = tempfile::TempDir::new().unwrap();
        let staging = td.path().join("staging");
        fs::create_dir_all(&staging).unwrap();
        let mut file = ProjectFile::default();
        for e in fs::read_dir(root).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let key = if dir { format!("{name}/") } else { name };
            file.entries.insert(
                key,
                EntryRecord {
                    gen: 1,
                    state: State::Tracked,
                },
            );
        }
        crate::mirror::root_to_staging(root, &staging, &file.tracked(), ignore, Limits::default())
            .unwrap();
        fn go(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if fs::symlink_metadata(&p).unwrap().is_dir() {
                    go(base, &p, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                }
            }
        }
        let mut out = Vec::new();
        go(&staging, &staging, &mut out);
        out.sort();
        out
    }

    fn agent_root(home: &Path, name: &str) -> crate::config::Root {
        crate::config::Root {
            slug: "t".into(),
            path: home.join(name),
            initializing: false,
        }
    }

    #[test]
    fn seed_materialises_only_existing_default_patterns() {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        put(&root.join("CLAUDE.md"), "hi");
        put(&root.join("docs/a.md"), "docs");
        put(&root.join(".env"), "SECRET=1");
        put(&root.join("README.md"), "nope");
        fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join(".claude")).unwrap();

        let (file, skipped) =
            seed(root, &default_pattern_strings(), "", Limits::default()).unwrap();
        assert!(skipped.is_empty());
        let keys = tracked_keys(&file);
        assert_eq!(keys, vec![".env", "CLAUDE.md", "docs/"]);
        for k in &keys {
            assert_eq!(file.entries[k].gen, 1);
            assert_eq!(file.entries[k].state, State::Tracked);
        }

        let (ignored, _) = seed(
            root,
            &default_pattern_strings(),
            "CLAUDE.md\n",
            Limits::default(),
        )
        .unwrap();
        assert_eq!(tracked_keys(&ignored), vec![".env", "docs/"]);
    }

    #[test]
    fn seed_ignores_a_pattern_whose_kind_does_not_match_its_trailing_slash() {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        fs::write(root.join("docs"), "a file named docs").unwrap();
        fs::create_dir_all(root.join("CLAUDE.md")).unwrap();
        put(&root.join("AGENTS.md"), "ok");

        let (file, _) = seed(root, &default_pattern_strings(), "", Limits::default()).unwrap();
        let keys = tracked_keys(&file);
        assert!(!keys.iter().any(|k| k == "docs" || k == "docs/"));
        assert!(!keys.iter().any(|k| k == "CLAUDE.md" || k == "CLAUDE.md/"));
        assert_eq!(keys, vec!["AGENTS.md"]);
    }

    #[test]
    fn seed_does_not_descend_into_subdirectories() {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        put(&root.join("packages/web/.claude/settings.json"), "{}");

        let (file, _) = seed(root, &default_pattern_strings(), "", Limits::default()).unwrap();
        assert!(
            tracked_keys(&file).is_empty(),
            "nested .claude/ must not be seeded: {:?}",
            tracked_keys(&file)
        );
    }

    #[test]
    fn an_agent_folder_seeds_from_its_own_agent_patterns() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let cursor = home.join(".cursor");
        put(&cursor.join("chats/old.md"), "chat");
        put(&cursor.join("rules/a.md"), "rule");
        put(&cursor.join("cli-config.json"), "{}");

        let root = agent_root(home, ".cursor");
        let cfg = crate::config::Config::default();
        let pats = patterns_for(&root, home, &cfg);
        let (file, _) = seed(&cursor, &pats, DEFAULT_NEVER_IGNORE, Limits::default()).unwrap();
        let keys = tracked_keys(&file);
        assert!(keys.contains(&"rules/".into()), "{keys:?}");
        assert!(keys.contains(&"cli-config.json".into()), "{keys:?}");
        assert!(!keys.iter().any(|k| k.starts_with("chats")));
    }

    #[test]
    fn an_unknown_agent_folder_seeds_from_the_generic_patterns() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let agent = home.join(".newagent");
        put(&agent.join("AGENTS.md"), "hi");
        put(&agent.join("settings.json"), "{}");
        put(&agent.join("rules/a.md"), "r");
        put(&agent.join("chats/x.md"), "nope");

        let root = agent_root(home, ".newagent");
        let cfg = crate::config::Config::default();
        let pats = patterns_for(&root, home, &cfg);
        assert_eq!(
            pats,
            GENERIC_AGENT_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
        let (file, _) = seed(&agent, &pats, DEFAULT_NEVER_IGNORE, Limits::default()).unwrap();
        let keys = tracked_keys(&file);
        assert_eq!(keys, vec!["AGENTS.md", "rules/", "settings.json"]);
    }

    #[test]
    fn a_default_patterns_override_does_not_apply_to_an_agent_folder() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let claude = home.join(".claude");
        put(&claude.join("rules/a.md"), "r");
        put(&claude.join("docs/x.md"), "docs");

        let root = agent_root(home, ".claude");
        let cfg = crate::config::Config {
            default_patterns: Some(vec!["docs/".into()]),
            ..Default::default()
        };
        let pats = patterns_for(&root, home, &cfg);
        let (file, _) = seed(&claude, &pats, DEFAULT_NEVER_IGNORE, Limits::default()).unwrap();
        let keys = tracked_keys(&file);
        assert!(keys.contains(&"rules/".into()), "{keys:?}");
        assert!(!keys.iter().any(|k| k == "docs" || k == "docs/"));
    }

    #[test]
    fn a_claude_override_is_returned_for_a_claude_root() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let root = agent_root(home, ".claude");
        let mut cfg = crate::config::Config::default();
        cfg.agent_patterns
            .insert("claude".into(), vec!["custom.md".into()]);
        assert_eq!(
            patterns_for(&root, home, &cfg),
            vec!["custom.md".to_string()]
        );
    }

    #[test]
    fn an_other_override_is_returned_for_an_unknown_agent_and_not_for_claude() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let unknown = agent_root(home, ".newagent");
        let claude = agent_root(home, ".claude");
        let mut cfg = crate::config::Config::default();
        cfg.agent_patterns
            .insert("other".into(), vec!["only-other.md".into()]);
        assert_eq!(
            patterns_for(&unknown, home, &cfg),
            vec!["only-other.md".to_string()]
        );
        assert_eq!(
            patterns_for(&claude, home, &cfg),
            owned(builtin_lines("claude").unwrap())
        );
    }

    #[test]
    fn a_missing_claude_key_returns_the_builtin_claude_slice() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let root = agent_root(home, ".claude");
        let cfg = crate::config::Config::default();
        assert!(!cfg.agent_patterns.contains_key("claude"));
        let builtin = AGENT_PATTERNS
            .iter()
            .find(|(key, _)| *key == "claude")
            .map(|(_, lines)| *lines)
            .unwrap();
        assert_eq!(builtin_lines("claude"), Some(builtin));
        assert_eq!(patterns_for(&root, home, &cfg), owned(builtin));
    }

    #[test]
    fn an_empty_claude_vec_returns_an_empty_list() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let root = agent_root(home, ".claude");
        let mut cfg = crate::config::Config::default();
        cfg.agent_patterns.insert("claude".into(), Vec::new());
        assert_eq!(patterns_for(&root, home, &cfg), Vec::<String>::new());
    }

    #[test]
    fn default_patterns_include_docs_and_env_files() {
        assert!(DEFAULT_PATTERNS.contains(&"docs/"));
        assert!(DEFAULT_PATTERNS.contains(&".env"));
        assert!(DEFAULT_PATTERNS.contains(&".env.local"));
    }

    #[test]
    fn default_ignore_allows_only_the_selected_claude_plugin_json_files() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let claude = home.join(".claude");
        put(&claude.join("plugins/installed_plugins.json"), "{}");
        put(&claude.join("plugins/known_marketplaces.json"), "{}");
        put(&claude.join("plugins/blocklist.json"), "{}");
        put(&claude.join("plugins/extra-cache.json"), "cache");
        put(&claude.join("settings.json"), "{}");
        put(&claude.join("projects/a.jsonl"), "{}");
        put(&claude.join("chats/x.md"), "chat");

        let root = agent_root(home, ".claude");
        let cfg = crate::config::Config::default();
        let pats = patterns_for(&root, home, &cfg);
        let (file, _) = seed(&claude, &pats, DEFAULT_NEVER_IGNORE, Limits::default()).unwrap();
        let keys = tracked_keys(&file);
        assert!(keys.contains(&"plugins/installed_plugins.json".into()));
        assert!(keys.contains(&"plugins/known_marketplaces.json".into()));
        assert!(keys.contains(&"plugins/blocklist.json".into()));
        assert!(!keys.iter().any(|k| k.contains("extra-cache")));
        assert!(!keys.iter().any(|k| k.starts_with("projects")));

        let staged = staged_files(&claude, DEFAULT_NEVER_IGNORE);
        assert!(staged.contains(&"plugins/installed_plugins.json".into()));
        assert!(staged.contains(&"plugins/known_marketplaces.json".into()));
        assert!(staged.contains(&"plugins/blocklist.json".into()));
        assert!(
            staged.contains(&"plugins/extra-cache.json".into()),
            "no blanket plugins/ ignore: {staged:?}"
        );
        assert!(!staged.iter().any(|p| p.starts_with("projects/")));
        assert!(!staged.iter().any(|p| p.starts_with("chats/")));
    }

    #[test]
    fn default_ignore_allows_opencode_plugin_source_but_not_credentials() {
        let td = tempfile::TempDir::new().unwrap();
        let home = td.path();
        let opencode = home.join(".config/opencode");
        put(&opencode.join("plugins/foo/index.ts"), "export {}");
        put(&opencode.join("opencode.json"), "{}");
        put(&opencode.join("auth.json"), "secret");
        put(&opencode.join(".credentials.json"), "secret");

        let root = crate::config::Root {
            slug: "t".into(),
            path: opencode.clone(),
            initializing: false,
        };
        let cfg = crate::config::Config::default();
        assert!(root.is_agent(home));
        let pats = patterns_for(&root, home, &cfg);
        let (file, _) = seed(&opencode, &pats, DEFAULT_NEVER_IGNORE, Limits::default()).unwrap();
        let keys = tracked_keys(&file);
        assert!(keys.contains(&"plugins/".into()), "{keys:?}");
        assert!(keys.contains(&"opencode.json".into()), "{keys:?}");
        assert!(!keys
            .iter()
            .any(|k| k.contains("auth") || k.contains("credential")));

        let staged = staged_files(&opencode, DEFAULT_NEVER_IGNORE);
        assert!(
            staged.contains(&"plugins/foo/index.ts".into()),
            "opencode plugin source must not be ignored: {staged:?}"
        );
        assert!(staged.contains(&"opencode.json".into()));
        assert!(!staged
            .iter()
            .any(|p| p == "auth.json" || p.ends_with("/auth.json")));
        assert!(!staged.iter().any(|p| p.ends_with(".credentials.json")));
    }

    #[test]
    fn default_ignore_does_not_exclude_docs_plans_inside_a_project() {
        let td = tempfile::TempDir::new().unwrap();
        let proj = td.path().join("myproj");
        put(&proj.join("docs/plans/x.md"), "plan");
        put(&proj.join("CLAUDE.md"), "hi");

        let (file, _) = seed(
            &proj,
            &default_pattern_strings(),
            DEFAULT_NEVER_IGNORE,
            Limits::default(),
        )
        .unwrap();
        assert!(tracked_keys(&file).contains(&"docs/".into()));

        let staged = staged_files(&proj, DEFAULT_NEVER_IGNORE);
        assert!(
            staged.contains(&"docs/plans/x.md".into()),
            "root-anchored /plans/ must not hide docs/plans/: {staged:?}"
        );

        let home = td.path().join("home");
        let agent = home.join(".claude");
        put(&agent.join("plans/secret.md"), "nope");
        put(&agent.join("settings.json"), "{}");
        let staged_agent = staged_files(&agent, DEFAULT_NEVER_IGNORE);
        assert!(staged_agent.contains(&"settings.json".into()));
        assert!(
            !staged_agent.iter().any(|p| p.starts_with("plans/")),
            "agent-root /plans/ must stay excluded: {staged_agent:?}"
        );
    }

    #[test]
    fn seed_skips_a_directory_larger_than_the_seed_limit_and_reports_it() {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        put(&root.join("docs/a.md"), "aaaaaaaaaaaaaaa");
        put(&root.join("docs/b.md"), "bbbbbbbbbbbbbbb");
        let limits = Limits {
            max_file_bytes: 1000,
            max_seed_folder_bytes: 20,
        };
        let (file, skipped) = seed(root, &["docs/".into()], "", limits).unwrap();
        assert!(
            tracked_keys(&file).is_empty(),
            "oversized folder must not be seeded: {:?}",
            tracked_keys(&file)
        );
        assert_eq!(
            skipped,
            vec![Skipped {
                rel: "docs/".into(),
                bytes: 30,
            }]
        );
    }

    #[test]
    fn seed_counts_a_directory_after_the_ignore_matcher() {
        let td = tempfile::TempDir::new().unwrap();
        let root = td.path();
        put(&root.join("docs/keep.md"), "0123456789");
        put(
            &root.join("docs/secret.log"),
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        );
        let limits = Limits {
            max_file_bytes: 10_000,
            max_seed_folder_bytes: 20,
        };
        let (file, skipped) = seed(root, &["docs/".into()], "*.log\n", limits).unwrap();
        assert_eq!(tracked_keys(&file), vec!["docs/".to_string()]);
        assert!(
            skipped.is_empty(),
            "ignored bytes must not count toward the folder limit: {skipped:?}"
        );
    }
}
