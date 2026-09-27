//! Portable relative paths.
//!
//! Windows is the most restrictive target, so "portable" means representable
//! on NTFS + APFS default + ext4.
//!
//! [`same_path`] and [`starts_with_path`] also compare absolute paths after
//! stripping a Windows verbatim prefix, so a `canonicalize` result (`\\?\…`)
//! matches the same path without one. On a path that has no such prefix —
//! every macOS and Linux path — the comparison is [`Path`]'s own.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Why a relative path cannot be stored on every target filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameIssue {
    /// Short human reason, for example `contains ':'`.
    pub reason: &'static str,
}

impl std::fmt::Display for NameIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason)
    }
}

/// `Some` when `rel` is not portable.
///
/// Each `/`-separated component is checked. Rejected: `< > : " | ? *` and `\`,
/// control characters U+0000–U+001F, a trailing `.` or space, a reserved
/// Windows stem (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`,
/// any case, with or without an extension such as `nul.txt`), more than 255
/// UTF-16 code units, and the `.git` names `core.protectNTFS` blocks (`.git`
/// in any case, and the 8.3 short name `git~1`). The first problem, from left
/// to right, wins.
pub fn name_issue(rel: &str) -> Option<NameIssue> {
    rel.split('/').find_map(component_issue)
}

/// Pairs of paths that are equal under Unicode simple lowercase.
///
/// The whole path is compared, so `Docs/a.md` collides with `docs/a.md`.
/// Lowercasing is Unicode (`str::to_lowercase`), not ASCII-only and not
/// locale-dependent. Inside a colliding group the originals are ordered
/// byte-wise, and every combination is returned. Groups are ordered by their
/// lowercased form.
pub fn case_collisions<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<(String, String)> {
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for path in paths {
        groups.entry(path.to_lowercase()).or_default().push(path);
    }
    let mut pairs = Vec::new();
    for mut group in groups.into_values() {
        if group.len() < 2 {
            continue;
        }
        group.sort_unstable();
        for (i, left) in group.iter().enumerate() {
            for right in group.iter().skip(i + 1) {
                pairs.push(((*left).to_string(), (*right).to_string()));
            }
        }
    }
    pairs
}

/// Paths in `tracked` that must not be published, with the reason to report.
///
/// A [`name_issue`] is skipped. Inside a case-collision group the first path
/// in byte-wise order is kept; every other member is skipped, so the same
/// file loses on every call. A name issue wins over the collision wording
/// when a path has both.
///
/// Reasons look like `name cannot exist on Windows: contains ':'` and
/// `differs only by case from A.md`.
pub fn publish_skips<'a>(tracked: impl IntoIterator<Item = &'a str>) -> Vec<(String, String)> {
    let paths: Vec<&str> = tracked.into_iter().collect();
    let mut reasons: BTreeMap<&str, String> = BTreeMap::new();
    for path in &paths {
        if let Some(issue) = name_issue(path) {
            reasons.insert(
                *path,
                format!("name cannot exist on Windows: {}", issue.reason),
            );
        }
    }
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for path in &paths {
        groups
            .entry((*path).to_lowercase())
            .or_default()
            .push(*path);
    }
    for mut group in groups.into_values() {
        group.sort_unstable();
        group.dedup();
        if group.len() < 2 {
            continue;
        }
        let winner = group[0];
        for loser in group.iter().skip(1) {
            reasons
                .entry(*loser)
                .or_insert_with(|| format!("differs only by case from {winner}"));
        }
    }
    reasons
        .into_iter()
        .map(|(path, reason)| (path.to_string(), reason))
        .collect()
}

fn component_issue(component: &str) -> Option<NameIssue> {
    for c in component.chars() {
        if let Some(reason) = bad_char(c) {
            return Some(NameIssue { reason });
        }
    }
    if let Some(reason) = trailing_issue(component) {
        return Some(NameIssue { reason });
    }
    if reserved_device(component) {
        return Some(NameIssue {
            reason: "is a reserved Windows device name",
        });
    }
    // NTFS component limit is 255 UTF-16 code units, not bytes or scalars.
    if utf16_units(component) > 255 {
        return Some(NameIssue {
            reason: "is longer than 255 UTF-16 code units",
        });
    }
    ntfs_dotgit(component).map(|reason| NameIssue { reason })
}

fn bad_char(c: char) -> Option<&'static str> {
    match c {
        '<' => Some("contains '<'"),
        '>' => Some("contains '>'"),
        ':' => Some("contains ':'"),
        '"' => Some("contains '\"'"),
        '|' => Some("contains '|'"),
        '?' => Some("contains '?'"),
        '*' => Some("contains '*'"),
        '\\' => Some("contains '\\'"),
        '\u{0000}'..='\u{001F}' => Some("contains a control character"),
        _ => None,
    }
}

fn trailing_issue(component: &str) -> Option<&'static str> {
    if component.ends_with('.') {
        Some("ends with '.'")
    } else if component.ends_with(' ') {
        Some("ends with a space")
    } else {
        None
    }
}

/// `CON` / `PRN` / `AUX` / `NUL` / `COM1`–`COM9` / `LPT1`–`LPT9`, ignoring case.
/// The stem is the text before the first `.`, so `nul.txt` matches.
fn reserved_device(component: &str) -> bool {
    let stem = component
        .split_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(component);
    let bytes = stem.as_bytes();
    match bytes.len() {
        3 => {
            eq_ascii(bytes, b"con")
                || eq_ascii(bytes, b"prn")
                || eq_ascii(bytes, b"aux")
                || eq_ascii(bytes, b"nul")
        }
        4 => {
            let digit = bytes[3];
            (b'1'..=b'9').contains(&digit)
                && (eq_ascii(&bytes[..3], b"com") || eq_ascii(&bytes[..3], b"lpt"))
        }
        _ => false,
    }
}

fn eq_ascii(left: &[u8], lower: &[u8]) -> bool {
    left.len() == lower.len()
        && left
            .iter()
            .zip(lower)
            .all(|(byte, expected)| byte.to_ascii_lowercase() == *expected)
}

fn utf16_units(component: &str) -> usize {
    component.chars().map(char::len_utf16).sum()
}

/// `.git` in any case, and the 8.3 short name `git~1`.
/// Stream suffixes such as `::$INDEX_ALLOCATION` contain `:` and are rejected
/// earlier.
fn ntfs_dotgit(component: &str) -> Option<&'static str> {
    if component.eq_ignore_ascii_case(".git") {
        Some("is a .git name")
    } else if component.eq_ignore_ascii_case("git~1") {
        Some("is the 8.3 short name git~1")
    } else {
        None
    }
}

/// Equal after stripping a leading Windows verbatim prefix.
///
/// `\\?\C:\x` equals `C:\x`. `\\?\UNC\server\share` equals `\\server\share`.
/// A path with no `\\?\` prefix uses [`Path`] equality unchanged.
pub fn same_path(a: &Path, b: &Path) -> bool {
    without_verbatim(a) == without_verbatim(b)
}

/// Whether `child` is `root` or lives inside it, after the same strip as
/// [`same_path`].
///
/// `Path` on Unix does not split on `\`. A verbatim Windows path is therefore
/// also checked by `\` / `/` components, so the prefix rule can be unit-tested
/// on macOS. When `\` is already a separator, [`Path::starts_with`] is the
/// whole answer and stays case-insensitive on Windows.
pub fn starts_with_path(child: &Path, root: &Path) -> bool {
    let child_cmp = without_verbatim(child);
    let root_cmp = without_verbatim(root);
    if child_cmp.starts_with(root_cmp.as_ref()) {
        return true;
    }
    if backslash_is_separator() || (!verbatim_prefixed(child) && !verbatim_prefixed(root)) {
        return false;
    }
    component_prefix(child_cmp.as_ref(), root_cmp.as_ref())
}

/// Comparison form of `path`. Not for filesystem calls: the `\\?\` prefix is
/// what lets Windows open a path longer than `MAX_PATH`.
pub(crate) fn without_verbatim(path: &Path) -> Cow<'_, Path> {
    let Some(text) = path.to_str() else {
        return Cow::Borrowed(path);
    };
    if !has_verbatim(text.as_bytes()) {
        return Cow::Borrowed(path);
    }
    Cow::Owned(PathBuf::from(strip_verbatim_prefix(text).into_owned()))
}

fn verbatim_prefixed(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|text| has_verbatim(text.as_bytes()))
}

fn has_verbatim(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes[0] == b'\\'
        && bytes[1] == b'\\'
        && bytes[2] == b'?'
        && bytes[3] == b'\\'
}

/// `\\?\UNC\` (any ASCII case), which must be tested before the bare `\\?\` strip.
fn has_verbatim_unc(bytes: &[u8]) -> bool {
    bytes.len() >= 8
        && has_verbatim(bytes)
        && bytes[4..7].eq_ignore_ascii_case(b"UNC")
        && bytes[7] == b'\\'
}

/// `\\?\C:\x` → `C:\x`. `\\?\UNC\server\share` → `\\server\share`.
fn strip_verbatim_prefix(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    if has_verbatim_unc(bytes) {
        let mut out = String::with_capacity(text.len().saturating_sub(6));
        out.push('\\');
        out.push('\\');
        out.push_str(&text[8..]);
        return Cow::Owned(out);
    }
    if has_verbatim(bytes) {
        return Cow::Borrowed(&text[4..]);
    }
    Cow::Borrowed(text)
}

fn backslash_is_separator() -> bool {
    Path::new(r"a\b").starts_with("a")
}

fn component_prefix(child: &Path, root: &Path) -> bool {
    let (Some(child), Some(root)) = (child.to_str(), root.to_str()) else {
        return false;
    };
    windows_components(child).starts_with(&windows_components(root))
}

fn windows_components(path: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = path.split(['\\', '/']).collect();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    parts
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn reason(rel: &str) -> Option<&'static str> {
        name_issue(rel).map(|issue| issue.reason)
    }

    #[test]
    fn con_md_rejected() {
        assert_eq!(reason("CON.md"), Some("is a reserved Windows device name"));
    }

    #[test]
    fn com1_rejected() {
        assert_eq!(reason("com1"), Some("is a reserved Windows device name"));
    }

    #[test]
    fn colon_rejected() {
        assert_eq!(reason("a:b.md"), Some("contains ':'"));
    }

    #[test]
    fn trailing_dot_rejected() {
        assert_eq!(reason("trail."), Some("ends with '.'"));
    }

    #[test]
    fn plain_name_passes() {
        assert!(name_issue("ok.md").is_none());
    }

    #[test]
    fn a_md_case_collision() {
        assert_eq!(
            case_collisions(["A.md", "a.md"].into_iter()),
            vec![("A.md".to_string(), "a.md".to_string())]
        );
    }

    #[test]
    fn nested_case_collision() {
        assert_eq!(
            case_collisions(["a/b.md", "a/B.md"].into_iter()),
            vec![("a/B.md".to_string(), "a/b.md".to_string())]
        );
    }

    #[test]
    fn git_short_name_rejected() {
        assert_eq!(reason("GIT~1/x"), Some("is the 8.3 short name git~1"));
    }

    #[test]
    fn dot_git_any_case_rejected() {
        assert_eq!(reason(".GIT/x"), Some("is a .git name"));
    }

    #[test]
    fn control_char_rejected() {
        assert_eq!(reason("a\u{0001}b"), Some("contains a control character"));
    }

    #[test]
    fn trailing_space_rejected() {
        assert_eq!(reason("trail "), Some("ends with a space"));
    }

    #[test]
    fn backslash_rejected() {
        assert_eq!(reason(r"a\b"), Some("contains '\\'"));
    }

    #[test]
    fn star_rejected() {
        assert_eq!(reason("a*b"), Some("contains '*'"));
    }

    #[test]
    fn question_rejected() {
        assert_eq!(reason("a?b"), Some("contains '?'"));
    }

    #[test]
    fn quote_rejected() {
        assert_eq!(reason("a\"b"), Some("contains '\"'"));
    }

    #[test]
    fn less_than_rejected() {
        assert_eq!(reason("a<b"), Some("contains '<'"));
    }

    #[test]
    fn greater_than_rejected() {
        assert_eq!(reason("a>b"), Some("contains '>'"));
    }

    #[test]
    fn pipe_rejected() {
        assert_eq!(reason("a|b"), Some("contains '|'"));
    }

    #[test]
    fn prn_rejected() {
        assert_eq!(reason("PRN"), Some("is a reserved Windows device name"));
    }

    #[test]
    fn aux_rejected() {
        assert_eq!(reason("AUX"), Some("is a reserved Windows device name"));
    }

    #[test]
    fn nul_rejected() {
        assert_eq!(reason("NUL"), Some("is a reserved Windows device name"));
    }

    #[test]
    fn lpt1_rejected() {
        assert_eq!(reason("LPT1"), Some("is a reserved Windows device name"));
    }

    #[test]
    fn component_over_255_utf16_rejected() {
        // U+10000 is one scalar and two UTF-16 units, so 128 of them is 256
        // units (and only 128 scalars). A char-count limit would miss this.
        let name: String = std::iter::repeat('\u{10000}').take(128).collect();
        assert_eq!(
            reason(&name).as_deref(),
            Some("is longer than 255 UTF-16 code units")
        );
    }

    #[test]
    fn directory_case_collision() {
        assert_eq!(
            case_collisions(["Docs/a.md", "docs/a.md"].into_iter()),
            vec![("Docs/a.md".to_string(), "docs/a.md".to_string())]
        );
    }

    #[test]
    fn publish_skips_names_the_windows_reason() {
        assert_eq!(
            publish_skips(["ok.md", "a:b.md", "CON.md"]),
            vec![
                (
                    "CON.md".to_string(),
                    "name cannot exist on Windows: is a reserved Windows device name".to_string()
                ),
                (
                    "a:b.md".to_string(),
                    "name cannot exist on Windows: contains ':'".to_string()
                ),
            ]
        );
    }

    #[test]
    fn publish_skips_keeps_the_byte_wise_first_casing() {
        assert_eq!(
            publish_skips(["a.md", "A.md", "ok.md"]),
            vec![(
                "a.md".to_string(),
                "differs only by case from A.md".to_string()
            )]
        );
    }

    #[test]
    fn publish_skips_prefers_a_name_issue_over_the_collision_wording() {
        let got = publish_skips(["A:B", "a:b"]);
        assert!(got
            .iter()
            .all(|(_, reason)| reason.starts_with("name cannot exist on Windows:")));
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn verbatim_disk_path_compares_equal_to_the_plain_path() {
        assert!(same_path(Path::new(r"\\?\C:\x"), Path::new(r"C:\x")));
        assert!(starts_with_path(Path::new(r"\\?\C:\x"), Path::new(r"C:\x")));
        assert!(starts_with_path(Path::new(r"C:\x"), Path::new(r"\\?\C:\x")));
        assert!(starts_with_path(
            Path::new(r"\\?\C:\x\y"),
            Path::new(r"C:\x")
        ));
        assert!(!starts_with_path(
            Path::new(r"\\?\C:\xy"),
            Path::new(r"C:\x")
        ));
        assert!(!same_path(Path::new(r"\\?\C:\x"), Path::new(r"C:\y")));
    }

    #[test]
    fn verbatim_unc_path_compares_equal_to_the_plain_unc_path() {
        assert!(same_path(
            Path::new(r"\\?\UNC\server\share"),
            Path::new(r"\\server\share"),
        ));
        assert!(starts_with_path(
            Path::new(r"\\?\UNC\server\share"),
            Path::new(r"\\server\share"),
        ));
        assert!(starts_with_path(
            Path::new(r"\\server\share"),
            Path::new(r"\\?\UNC\server\share"),
        ));
        assert!(starts_with_path(
            Path::new(r"\\?\UNC\server\share\a"),
            Path::new(r"\\server\share"),
        ));
        assert!(starts_with_path(
            Path::new(r"\\?\unc\server\share\a"),
            Path::new(r"\\server\share"),
        ));
        assert!(!starts_with_path(
            Path::new(r"\\?\UNC\server\share2"),
            Path::new(r"\\server\share"),
        ));
        assert!(!same_path(
            Path::new(r"\\?\UNC\server\share"),
            Path::new(r"\\other\share"),
        ));
    }

    #[test]
    fn paths_without_a_verbatim_prefix_compare_as_plain_paths() {
        assert!(same_path(Path::new("/tmp/a"), Path::new("/tmp/a")));
        assert!(!same_path(Path::new("/tmp/a"), Path::new("/tmp/b")));
        assert!(starts_with_path(Path::new("/tmp/a/b"), Path::new("/tmp/a")));
        assert!(!starts_with_path(Path::new("/tmp/ab"), Path::new("/tmp/a")));
        assert!(starts_with_path(
            Path::new("/tmp/a/b"),
            Path::new("/tmp/a/")
        ));
    }
}
