//! The global settings that sync between a user's devices.
//!
//! Synced keys: `patterns.<catalog>` for every builtin catalog id
//! (`patterns.projects` is `Config::default_patterns`, every other id is
//! `Config::agent_patterns[id]`), `default_ignore`, `sensitive_patterns`,
//! `max_file_mb` and `max_seed_folder_mb`. Device fields stay local. A
//! [`Snapshot`] maps each key to a [`Field`]: a generation plus the raw config
//! value as JSON, where `null` means the builtin.
//!
//! [`apply`] merges per key: the higher gen wins; an equal gen is broken by the
//! greater canonical value bytes, `null` lowest, so every device converges
//! whatever order it reads snapshots in. A key's effective gen is its
//! `Config::settings_gens` entry, else 1 when the field holds an override,
//! else 0. Gen 0 means "never touched here" and is never published, so a
//! fresh device cannot override its peers with builtins.
//!
//! Snapshots come from other devices through the cloud folder and are
//! untrusted: size-capped before parsing, unknown keys dropped, and every
//! value validated the way the engine setters validate before it is stored.
//!
//! Trust boundary: the merge assumes every writer of the cloud folder is one
//! of the user's own devices, as bundles already do. A writer that is not can
//! always win: it picks the gen (up to [`MAX_GEN`]) and the value bytes. A key
//! it publishes at `MAX_GEN` leaves every later local edit tied at `MAX_GEN`,
//! decided by value bytes it chose to sort high. No deterministic merge closes
//! that. Deleting the file only helps before a device imports it: once taken
//! in, the gen lives in each device's `Config::settings_gens` and is published
//! again from there. There is no in-app reset: "Wipe cloud data" keeps
//! `settings_gens`, so the key must be cleared in every device's `config.json`.
//! [`MAX_GEN`] only keeps honest gens from overflowing.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::project::{self, SensitiveMatcher};

/// Largest snapshot file [`parse`] accepts.
pub const MAX_SNAPSHOT_BYTES: usize = 1 << 20;

/// Highest gen [`apply`] accepts and [`bump`] stores. Legitimate gens grow
/// by one per edit, so this is unreachable in practice; a remote gen above
/// it is malformed. See the trust boundary in the module doc.
pub const MAX_GEN: u64 = u32::MAX as u64;

const PATTERNS_PREFIX: &str = "patterns.";

/// One synced key: its merge generation and raw value (`null` = builtin).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Field {
    pub gen: u64,
    pub value: Value,
}

/// Every published key of one device.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    #[serde(default)]
    pub fields: BTreeMap<String, Field>,
}

impl Snapshot {
    /// Canonical bytes: `BTreeMap` keys and serde_json's sorted maps.
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a snapshot always serializes")
    }
}

/// A synced key, resolved to the config field it maps to.
#[derive(Clone, Copy)]
enum Slot<'a> {
    Catalog(&'a str),
    DefaultIgnore,
    Sensitive,
    MaxFile,
    MaxSeedFolder,
}

/// Catalog ids in dropdown order, as `engine::catalog_ids` lists them.
fn catalog_ids() -> impl Iterator<Item = &'static str> {
    std::iter::once("projects")
        .chain(project::AGENT_PATTERNS.iter().map(|(id, _)| *id))
        .chain(std::iter::once("other"))
        .filter(|id| project::catalog_label(id).is_some())
}

/// Every synced key.
pub fn keys() -> Vec<String> {
    catalog_ids()
        .map(|id| format!("{PATTERNS_PREFIX}{id}"))
        .chain(
            [
                "default_ignore",
                "sensitive_patterns",
                "max_file_mb",
                "max_seed_folder_mb",
            ]
            .map(str::to_string),
        )
        .collect()
}

fn slot(key: &str) -> Option<Slot<'_>> {
    if let Some(id) = key.strip_prefix(PATTERNS_PREFIX) {
        return project::catalog_label(id).map(|_| Slot::Catalog(id));
    }
    match key {
        "default_ignore" => Some(Slot::DefaultIgnore),
        "sensitive_patterns" => Some(Slot::Sensitive),
        "max_file_mb" => Some(Slot::MaxFile),
        "max_seed_folder_mb" => Some(Slot::MaxSeedFolder),
        _ => None,
    }
}

/// The raw config field as JSON; `null` when it holds no override.
fn raw_value(cfg: &Config, slot: Slot) -> Value {
    let v = match slot {
        Slot::Catalog("projects") => serde_json::to_value(&cfg.default_patterns),
        Slot::Catalog(id) => serde_json::to_value(cfg.agent_patterns.get(id)),
        Slot::DefaultIgnore => serde_json::to_value(&cfg.default_ignore),
        Slot::Sensitive => serde_json::to_value(&cfg.sensitive_patterns),
        Slot::MaxFile => serde_json::to_value(cfg.max_file_mb),
        Slot::MaxSeedFolder => serde_json::to_value(cfg.max_seed_folder_mb),
    };
    v.expect("config fields always serialize")
}

fn effective_gen(cfg: &Config, key: &str, slot: Slot) -> u64 {
    match cfg.settings_gens.get(key) {
        Some(gen) => *gen,
        None if raw_value(cfg, slot).is_null() => 0,
        None => 1,
    }
}

/// Whether `a` beats `b`: higher gen, then greater value bytes, `null` lowest.
fn wins(a: &Field, b: &Field) -> bool {
    match a.gen.cmp(&b.gen) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => match (a.value.is_null(), b.value.is_null()) {
            (true, _) => false,
            (false, true) => true,
            (false, false) => serde_json::to_vec(&a.value).ok() > serde_json::to_vec(&b.value).ok(),
        },
    }
}

/// This device's snapshot: every key with an effective gen above 0.
pub fn local(cfg: &Config) -> Snapshot {
    let mut fields = BTreeMap::new();
    for key in keys() {
        let Some(slot) = slot(&key) else { continue };
        let gen = effective_gen(cfg, &key, slot);
        if gen > 0 {
            let value = raw_value(cfg, slot);
            fields.insert(key, Field { gen, value });
        }
    }
    Snapshot { fields }
}

/// Apply every valid `remote` field that beats the local one. Returns
/// whether config changed. An invalid field is skipped, never an error.
pub fn apply(cfg: &mut Config, remote: &Snapshot) -> bool {
    let mut changed = false;
    for (key, field) in &remote.fields {
        // Gen 0 is never published and no edit reaches past MAX_GEN; a
        // field outside that range is malformed.
        if field.gen == 0 || field.gen > MAX_GEN {
            continue;
        }
        let Some(slot) = slot(key) else { continue };
        let Some(value) = normalized(slot, &field.value) else {
            continue;
        };
        let theirs = Field {
            gen: field.gen,
            value,
        };
        let ours = Field {
            gen: effective_gen(cfg, key, slot),
            value: raw_value(cfg, slot),
        };
        if wins(&theirs, &ours) {
            store(cfg, slot, theirs.value);
            cfg.settings_gens.insert(key.clone(), theirs.gen);
            changed = true;
        }
    }
    changed
}

/// The effective gen of `key`; 0 for an unknown key.
pub fn gen_of(cfg: &Config, key: &str) -> u64 {
    slot(key).map_or(0, |slot| effective_gen(cfg, key, slot))
}

/// Record a local edit of `key`, one gen above both `before` (its
/// [`gen_of`] ahead of the write) and its effective gen now. `before`
/// matters for a reset: a pre-sync override peers saw at gen 1 reads as
/// gen 0 once cleared, and 1 would then tie and lose to the old value.
pub fn bump(cfg: &mut Config, key: &str, before: u64) {
    if slot(key).is_none() {
        return;
    }
    let gen = before.max(gen_of(cfg, key)).saturating_add(1).min(MAX_GEN);
    cfg.settings_gens.insert(key.to_string(), gen);
}

/// Decode a snapshot file. `None` when it is over [`MAX_SNAPSHOT_BYTES`] or not valid JSON.
pub fn parse(bytes: &[u8]) -> Option<Snapshot> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

/// A catalog list as stored: `None` when it equals that catalog's builtin.
pub fn catalog_override(catalog: &str, patterns: Vec<String>) -> Option<Vec<String>> {
    match project::builtin_lines(catalog) {
        Some(builtin) if patterns.iter().eq(builtin.iter()) => None,
        _ => Some(patterns),
    }
}

/// The ignore text as stored: `None` when it equals `DEFAULT_NEVER_IGNORE`.
pub fn ignore_override(text: String) -> Option<String> {
    (text != project::DEFAULT_NEVER_IGNORE).then_some(text)
}

/// The sensitive list as stored: trimmed, empties dropped, deduplicated,
/// `None` when it equals `SECRET_PATTERNS`. An invalid line is an error.
pub fn sensitive_override(patterns: &[String]) -> Result<Option<Vec<String>>> {
    let mut seen = BTreeSet::new();
    let patterns: Vec<String> = patterns
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .filter(|p| seen.insert(p.to_string()))
        .map(str::to_string)
        .collect();
    SensitiveMatcher::new(&patterns)?;
    if patterns.iter().eq(project::SECRET_PATTERNS.iter()) {
        Ok(None)
    } else {
        Ok(Some(patterns))
    }
}

/// `value` validated and normalized for `slot`, as JSON (`null` = builtin).
/// `None` when the value is invalid.
fn normalized(slot: Slot, value: &Value) -> Option<Value> {
    if value.is_null() {
        return Some(Value::Null);
    }
    let v = match slot {
        Slot::Catalog(id) => {
            let lines: Vec<String> = serde_json::from_value(value.clone()).ok()?;
            serde_json::to_value(catalog_override(id, lines))
        }
        Slot::DefaultIgnore => {
            let text: String = serde_json::from_value(value.clone()).ok()?;
            serde_json::to_value(ignore_override(text))
        }
        Slot::Sensitive => {
            let lines: Vec<String> = serde_json::from_value(value.clone()).ok()?;
            serde_json::to_value(sensitive_override(&lines).ok()?)
        }
        Slot::MaxFile | Slot::MaxSeedFolder => {
            let mb: u64 = serde_json::from_value(value.clone()).ok()?;
            serde_json::to_value((mb >= 1).then_some(mb)?)
        }
    };
    v.ok()
}

/// Write a [`normalized`] value into its config field.
fn store(cfg: &mut Config, slot: Slot, value: Value) {
    match slot {
        Slot::Catalog(id) => {
            let lines: Option<Vec<String>> = serde_json::from_value(value).ok().flatten();
            match (id, lines) {
                ("projects", lines) => cfg.default_patterns = lines,
                (id, Some(lines)) => {
                    cfg.agent_patterns.insert(id.to_string(), lines);
                }
                (id, None) => {
                    cfg.agent_patterns.remove(id);
                }
            }
        }
        Slot::DefaultIgnore => cfg.default_ignore = serde_json::from_value(value).ok().flatten(),
        Slot::Sensitive => cfg.sensitive_patterns = serde_json::from_value(value).ok().flatten(),
        Slot::MaxFile => cfg.max_file_mb = serde_json::from_value(value).ok().flatten(),
        Slot::MaxSeedFolder => {
            cfg.max_seed_folder_mb = serde_json::from_value(value).ok().flatten()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(gen: u64, value: Value) -> Field {
        Field { gen, value }
    }

    fn snap(pairs: &[(&str, Field)]) -> Snapshot {
        Snapshot {
            fields: pairs
                .iter()
                .map(|(k, f)| (k.to_string(), f.clone()))
                .collect(),
        }
    }

    fn owned(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    /// `cfg` after applying `first` then `second` to a fresh config.
    fn applied(first: &Snapshot, second: &Snapshot) -> Config {
        let mut cfg = Config::default();
        apply(&mut cfg, first);
        apply(&mut cfg, second);
        cfg
    }

    #[test]
    fn apply_keeps_the_higher_gen() {
        let a = snap(&[("max_file_mb", field(3, json!(10)))]);
        let b = snap(&[
            ("max_file_mb", field(2, json!(99))),
            ("default_ignore", field(1, json!("x"))),
        ]);
        for cfg in [applied(&a, &b), applied(&b, &a)] {
            assert_eq!(cfg.max_file_mb, Some(10));
            assert_eq!(cfg.default_ignore.as_deref(), Some("x"));
            assert_eq!(cfg.settings_gens["max_file_mb"], 3);
        }
    }

    #[test]
    fn an_equal_gen_tie_break_is_order_independent() {
        let pairs = [
            ("max_file_mb", json!(10), json!(20)),
            ("patterns.claude", json!(["a/"]), json!(["b/"])),
            ("patterns.claude", json!(["a/"]), Value::Null),
            ("default_ignore", json!("x"), Value::Null),
            ("max_file_mb", json!(5), Value::Null),
        ];
        for (key, x, y) in pairs {
            let a = snap(&[(key, field(4, x.clone()))]);
            let b = snap(&[(key, field(4, y.clone()))]);
            let ab = local(&applied(&a, &b));
            assert_eq!(ab, local(&applied(&b, &a)), "{x} vs {y}");
            if y.is_null() {
                assert_eq!(ab.fields[key].value, x, "null is lowest");
            }
        }
    }

    #[test]
    fn a_gen_above_the_ceiling_is_rejected() {
        let mut cfg = Config::default();
        let planted = snap(&[
            ("max_file_mb", field(u64::MAX, json!(99))),
            ("default_ignore", field(MAX_GEN + 1, json!("x"))),
        ]);
        assert!(!apply(&mut cfg, &planted));
        assert_eq!(cfg.max_file_mb, None);
        assert_eq!(cfg.default_ignore, None);
        assert!(cfg.settings_gens.is_empty());
        let top = snap(&[("max_file_mb", field(MAX_GEN, json!(7)))]);
        assert!(apply(&mut cfg, &top));
        assert_eq!(cfg.max_file_mb, Some(7));
    }

    #[test]
    fn bump_never_exceeds_the_ceiling() {
        let mut cfg = Config::default();
        cfg.settings_gens.insert("max_file_mb".into(), MAX_GEN);
        bump(&mut cfg, "max_file_mb", MAX_GEN);
        assert_eq!(cfg.settings_gens["max_file_mb"], MAX_GEN);
        bump(&mut cfg, "default_ignore", u64::MAX);
        assert_eq!(cfg.settings_gens["default_ignore"], MAX_GEN);
    }

    #[test]
    fn a_fresh_config_has_an_empty_snapshot() {
        assert_eq!(local(&Config::default()), Snapshot::default());
    }

    #[test]
    fn a_pre_sync_override_counts_as_gen_one() {
        let mut cfg = Config::default();
        cfg.max_file_mb = Some(10);
        cfg.agent_patterns.insert("claude".into(), owned(&["x/"]));
        let s = local(&cfg);
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields["max_file_mb"], field(1, json!(10)));
        assert_eq!(s.fields["patterns.claude"], field(1, json!(["x/"])));
        let before = gen_of(&cfg, "max_file_mb");
        bump(&mut cfg, "max_file_mb", before);
        assert_eq!(cfg.settings_gens["max_file_mb"], 2);
        let before = gen_of(&cfg, "default_ignore");
        bump(&mut cfg, "default_ignore", before);
        assert_eq!(cfg.settings_gens["default_ignore"], 1);
    }

    #[test]
    fn resetting_a_pre_sync_override_outranks_its_old_gen() {
        let mut cfg = Config::default();
        cfg.max_file_mb = Some(10);
        let mut peer = cfg.clone();
        let before = gen_of(&cfg, "max_file_mb");
        assert_eq!(before, 1);
        cfg.max_file_mb = None;
        bump(&mut cfg, "max_file_mb", before);
        assert_eq!(cfg.settings_gens["max_file_mb"], 2);
        assert!(apply(&mut peer, &local(&cfg)));
        assert_eq!(peer.max_file_mb, None);
        assert_eq!(gen_of(&cfg, "nope"), 0);
    }

    #[test]
    fn a_null_value_clears_the_override() {
        let mut cfg = Config::default();
        cfg.max_file_mb = Some(10);
        cfg.agent_patterns.insert("claude".into(), owned(&["x/"]));
        let remote = snap(&[
            ("max_file_mb", field(2, Value::Null)),
            ("patterns.claude", field(2, Value::Null)),
        ]);
        assert!(apply(&mut cfg, &remote));
        assert_eq!(cfg.max_file_mb, None);
        assert!(!cfg.agent_patterns.contains_key("claude"));
        assert_eq!(cfg.settings_gens["max_file_mb"], 2);
        assert_eq!(cfg.settings_gens["patterns.claude"], 2);
        // A cleared key at gen 2 is still published so peers clear too.
        assert_eq!(local(&cfg).fields["max_file_mb"], field(2, Value::Null));
    }

    #[test]
    fn apply_maps_catalog_keys_to_default_and_agent_patterns() {
        let mut cfg = Config::default();
        let remote = snap(&[
            ("patterns.projects", field(1, json!(["p/"]))),
            ("patterns.claude", field(1, json!(["c/"]))),
            ("patterns.other", field(1, json!([]))),
        ]);
        assert!(apply(&mut cfg, &remote));
        assert_eq!(cfg.default_patterns, Some(owned(&["p/"])));
        assert_eq!(cfg.agent_patterns["claude"], owned(&["c/"]));
        assert_eq!(cfg.agent_patterns["other"], Vec::<String>::new());
        assert!(!cfg.agent_patterns.contains_key("projects"));
        assert!(keys().contains(&"patterns.projects".to_string()));
        assert!(keys().contains(&"patterns.other".to_string()));
    }

    #[test]
    fn unknown_keys_and_catalogs_are_dropped() {
        let mut cfg = Config::default();
        let remote = snap(&[
            ("patterns.nope", field(5, json!(["a/"]))),
            ("device_name", field(5, json!("evil"))),
            ("provider_dir", field(5, json!("/tmp"))),
            ("patterns.projects", field(5, json!("not a list"))),
            ("default_ignore", field(5, json!(42))),
        ]);
        assert!(!apply(&mut cfg, &remote));
        assert!(cfg.agent_patterns.is_empty());
        assert_eq!(cfg.default_patterns, None);
        assert_eq!(cfg.default_ignore, None);
        assert!(cfg.device_name.is_empty());
        assert!(cfg.settings_gens.is_empty());
    }

    #[test]
    fn a_zero_limit_is_rejected() {
        let mut cfg = Config::default();
        let remote = snap(&[
            ("max_file_mb", field(3, json!(0))),
            ("max_seed_folder_mb", field(3, json!(-4))),
        ]);
        assert!(!apply(&mut cfg, &remote));
        assert_eq!(cfg.max_file_mb, None);
        assert_eq!(cfg.max_seed_folder_mb, None);
        assert!(cfg.settings_gens.is_empty());
        let ok = snap(&[("max_seed_folder_mb", field(3, json!(1)))]);
        assert!(apply(&mut cfg, &ok));
        assert_eq!(cfg.max_seed_folder_mb, Some(1));
    }

    #[test]
    fn an_invalid_sensitive_list_is_rejected() {
        let mut cfg = Config::default();
        let bad = snap(&[("sensitive_patterns", field(2, json!(["*.pem", "bad[z-a]"])))]);
        assert!(!apply(&mut cfg, &bad));
        assert_eq!(cfg.sensitive_patterns, None);
        assert!(!cfg.settings_gens.contains_key("sensitive_patterns"));
        let messy = snap(&[(
            "sensitive_patterns",
            field(2, json!([" *.pem ", "", "*.pem", "*.key"])),
        )]);
        assert!(apply(&mut cfg, &messy));
        assert_eq!(cfg.sensitive_patterns, Some(owned(&["*.pem", "*.key"])));
    }

    #[test]
    fn a_braced_trailing_whitespace_pattern_survives_the_write_path() {
        use crate::project::Sensitivity;
        use std::path::Path;
        let input = vec!["/s{ }".to_string(), "/t{\u{a0}}".to_string()];
        let lines = sensitive_override(&input).unwrap().unwrap();
        assert_eq!(lines, input);
        let m = SensitiveMatcher::new(&lines).unwrap();
        assert_eq!(m.classify(Path::new("s ")), Some(Sensitivity::Secret));
        assert_eq!(m.classify(Path::new("t\u{a0}")), Some(Sensitivity::Secret));
        assert_eq!(m.classify(Path::new("s")), None);
    }

    #[test]
    fn a_value_equal_to_builtin_is_stored_as_none() {
        let mut cfg = Config::default();
        let claude = project::builtin_lines("claude").unwrap();
        let remote = snap(&[
            (
                "patterns.projects",
                field(2, json!(project::DEFAULT_PATTERNS)),
            ),
            ("patterns.claude", field(2, json!(claude))),
            (
                "default_ignore",
                field(2, json!(project::DEFAULT_NEVER_IGNORE)),
            ),
            (
                "sensitive_patterns",
                field(2, json!(project::SECRET_PATTERNS)),
            ),
        ]);
        assert!(apply(&mut cfg, &remote));
        assert_eq!(cfg.default_patterns, None);
        assert!(cfg.agent_patterns.is_empty());
        assert_eq!(cfg.default_ignore, None);
        assert_eq!(cfg.sensitive_patterns, None);
        assert_eq!(cfg.settings_gens["patterns.claude"], 2);
    }

    #[test]
    fn an_oversized_snapshot_is_not_parsed() {
        let s = snap(&[("max_file_mb", field(1, json!(7)))]);
        assert_eq!(parse(&s.to_bytes()), Some(s));
        assert_eq!(parse(b"{}"), Some(Snapshot::default()));
        assert_eq!(parse(b"not json"), None);
        let pad = "x".repeat(MAX_SNAPSHOT_BYTES);
        let big = format!(r#"{{"fields":{{"default_ignore":{{"gen":1,"value":"{pad}"}}}}}}"#);
        assert!(big.len() > MAX_SNAPSHOT_BYTES);
        assert_eq!(parse(big.as_bytes()), None);
    }

    #[test]
    fn apply_reports_no_change_when_nothing_wins() {
        let mut cfg = Config::default();
        cfg.max_file_mb = Some(10);
        cfg.settings_gens.insert("max_file_mb".into(), 3);
        let before = cfg.clone();
        let remote = snap(&[
            ("max_file_mb", field(2, json!(99))),
            ("default_ignore", field(0, json!("x"))),
        ]);
        assert!(!apply(&mut cfg, &remote));
        let own = local(&cfg);
        assert!(!apply(&mut cfg, &own));
        assert_eq!(cfg.max_file_mb, before.max_file_mb);
        assert_eq!(cfg.default_ignore, None);
        assert_eq!(cfg.settings_gens, before.settings_gens);
    }
}
