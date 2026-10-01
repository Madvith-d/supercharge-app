//! Unified write layer for App agent-home `config.toml`.
//!
//! Independent session-data mode (`GROK_HOME` = App agent-home) may write
//! allowlisted keys. Shared mode always refuses path resolve / strict write so
//! the App never rewrites the user's personal `~/.grok/config.toml`.
//!
//! Pure TOML helpers are line-oriented upserts (no full parse) so unrelated
//! sections and secrets stay intact.
//!
//! ## Safety
//! - Key match is **exact** (never `starts_with`) so `yolo` cannot clobber `yolo_mode`.
//! - Table headers accept trailing comments (`[ui] # note`).
//! - [`ensure_agent_home_config_sane`] dedupes broken files only; valid configs are
//!   never rewritten.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths::{agent_config_toml, ensure_app_dirs};

/// Serialize independent agent-home config writers (cascade sync + heal + prefs).
static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Run `f` while holding the agent-home config write lock.
pub fn with_config_write_lock<T>(f: impl FnOnce() -> T) -> T {
    let _guard = CONFIG_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// Normalize session_data_mode to `independent` | `shared`.
pub fn normalize_mode(session_data_mode: &str) -> &'static str {
    if session_data_mode.trim().eq_ignore_ascii_case("shared") {
        "shared"
    } else {
        "independent"
    }
}

/// Error message when shared mode refuses a write-path resolve.
pub const SHARED_MODE_REFUSED: &str =
    "shared session mode: agent-home config.toml writes refused (never rewrite ~/.grok)";

/// Resolve App agent-home `config.toml` for writes.
///
/// - independent → `…/agent-home/config.toml` (ensures app dirs)
/// - shared → `Err` (refuse; do not return `~/.grok/config.toml`)
pub fn resolve_writable_config_path(session_data_mode: &str) -> Result<PathBuf, String> {
    if normalize_mode(session_data_mode) == "shared" {
        return Err(SHARED_MODE_REFUSED.into());
    }
    let _ = ensure_app_dirs();
    Ok(agent_config_toml())
}

fn bool_lit(v: bool) -> &'static str {
    if v {
        "true"
    } else {
        "false"
    }
}

fn finish_join(original: &str, lines: &[String]) -> String {
    let mut joined = lines.join("\n");
    if (original.ends_with('\n') || original.is_empty()) && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

fn finish_join_always_nl(_original: &str, lines: &[String]) -> String {
    let mut joined = lines.join("\n");
    if !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Parse a TOML table / array-table header.
///
/// Accepts trailing comments: `[ui] # note`, `[[hooks]] # x`.
/// Returns `(is_array_table, table_name)`.
pub fn parse_table_header(trimmed: &str) -> Option<(bool, &str)> {
    let t = trimmed.trim();
    if !t.starts_with('[') {
        return None;
    }
    let is_array = t.starts_with("[[");
    let (name, after_close) = if is_array {
        let end = t.find("]]")?;
        let name = t[2..end].trim();
        let after = t[end + 2..].trim();
        (name, after)
    } else {
        let end = t.find(']')?;
        let name = t[1..end].trim();
        let after = t[end + 1..].trim();
        (name, after)
    };
    if name.is_empty() {
        return None;
    }
    // Only whitespace / comments allowed after the closing bracket.
    if !after_close.is_empty() && !after_close.starts_with('#') {
        return None;
    }
    Some((is_array, name))
}

/// Key of a simple `key = value` assignment line (None for headers / comments / blank).
pub fn assignment_key(trimmed: &str) -> Option<&str> {
    let t = trimmed.trim();
    if t.is_empty() || t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let eq = t.find('=')?;
    let key = t[..eq].trim();
    if key.is_empty() {
        return None;
    }
    Some(key)
}

/// Upsert a bare top-level `key = value` assignment (not inside a `[table]`).
pub fn set_top_level_assignment(text: &str, key: &str, value: &str) -> String {
    let line_val = format!("{key} = {value}");
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let mut in_table = false;
    let mut first_table_idx: Option<usize> = None;

    for i in 0..lines.len() {
        let trimmed = lines[i].trim();
        if parse_table_header(trimmed).is_some() {
            if first_table_idx.is_none() {
                first_table_idx = Some(i);
            }
            in_table = true;
            continue;
        }
        if in_table {
            continue;
        }
        if assignment_key(trimmed) == Some(key) {
            lines[i] = line_val;
            return finish_join(text, &lines);
        }
    }

    if let Some(idx) = first_table_idx {
        lines.insert(idx, line_val);
        return finish_join_always_nl(text, &lines);
    }

    let base = text.trim_end();
    if base.is_empty() {
        format!("{line_val}\n")
    } else {
        format!("{base}\n{line_val}\n")
    }
}

/// Upsert top-level `key = true|false`.
pub fn set_top_level_bool(text: &str, key: &str, value: bool) -> String {
    set_top_level_assignment(text, key, bool_lit(value))
}

/// Upsert top-level `key = "value"` (quoted string).
#[allow(dead_code)]
pub fn set_top_level_string(text: &str, key: &str, value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    set_top_level_assignment(text, key, &format!("\"{escaped}\""))
}

/// Upsert `key = value` under `[table]` without touching other sections/keys.
///
/// Header match is by table **name**, so `[ui] # comment` counts as `[ui]`.
/// Key match is **exact** (not prefix).
pub fn set_table_key(text: &str, table: &str, key: &str, value: &str, quoted: bool) -> String {
    let header = format!("[{table}]");
    let line_val = if quoted {
        format!(
            "{key} = \"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("{key} = {value}")
    };
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let mut in_table = false;
    let mut table_start: Option<usize> = None;
    for i in 0..lines.len() {
        let trimmed = lines[i].trim();
        if let Some((is_array, name)) = parse_table_header(trimmed) {
            if !is_array && name == table {
                in_table = true;
                table_start = Some(i);
            } else if in_table {
                lines.insert(i, line_val);
                return finish_join_always_nl(text, &lines);
            } else {
                in_table = false;
            }
            continue;
        }
        if in_table && assignment_key(trimmed) == Some(key) {
            lines[i] = line_val;
            return finish_join(text, &lines);
        }
    }
    if let Some(start) = table_start {
        lines.insert(start + 1, line_val);
        return finish_join_always_nl(text, &lines);
    }
    let block = format!("\n{header}\n{line_val}\n");
    let base = text.trim_end();
    if base.is_empty() {
        format!("{header}\n{line_val}\n")
    } else {
        format!("{base}{block}")
    }
}

/// Upsert `[table] key = true|false`.
pub fn set_table_bool(text: &str, table: &str, key: &str, value: bool) -> String {
    set_table_key(text, table, key, bool_lit(value), false)
}

/// Pin App agent-home so Grok does **not** merge Claude/Cursor MCP catalogs.
///
/// Without this, `~/.claude.json` mcpServers (Playwright, open-websearch, …)
/// are imported by default and stall first-tool discovery for ~30s.
pub fn ensure_compat_mcp_disabled(text: &str) -> String {
    let t = set_table_bool(text, "compat.claude", "mcps", false);
    set_table_bool(&t, "compat.cursor", "mcps", false)
}

/// Write `[compat.claude/cursor] mcps = false` into independent agent-home.
pub fn apply_compat_mcp_disabled(session_data_mode: &str) -> Result<(), String> {
    update_config_toml_if_independent(session_data_mode, ensure_compat_mcp_disabled).map(|_| ())
}

/// Upsert `[table] key = "value"`.
pub fn set_table_string(text: &str, table: &str, key: &str, value: &str) -> String {
    set_table_key(text, table, key, value, true)
}

/// Parse a TOML bool literal (`true` / `false`).
pub fn parse_toml_bool(raw: &str) -> Option<bool> {
    match raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_ascii_lowercase()
        .as_str()
    {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Parse a TOML string / bare value (strip surrounding quotes; drop inline `#`).
pub fn parse_toml_scalar(raw: &str) -> String {
    let s = raw.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        return s[1..s.len() - 1].to_string();
    }
    s.split('#').next().unwrap_or(s).trim().to_string()
}

fn value_for_key_in_scope<'a>(text: &'a str, table: Option<&str>, key: &str) -> Option<&'a str> {
    let mut current: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((is_array, name)) = parse_table_header(trimmed) {
            // Array tables are not used for simple get helpers.
            current = if is_array { None } else { Some(name) };
            continue;
        }
        let Some(k) = assignment_key(trimmed) else {
            continue;
        };
        if k != key {
            continue;
        }
        let in_scope = match table {
            None => current.is_none(),
            Some(t) => current == Some(t),
        };
        if in_scope {
            let eq = trimmed.find('=')?;
            return Some(trimmed[eq + 1..].trim());
        }
    }
    None
}

/// Read top-level bool when present.
#[allow(dead_code)]
pub fn get_top_level_bool(text: &str, key: &str) -> Option<bool> {
    value_for_key_in_scope(text, None, key).and_then(parse_toml_bool)
}

/// Read `[table].key` bool when present.
pub fn get_table_bool(text: &str, table: &str, key: &str) -> Option<bool> {
    value_for_key_in_scope(text, Some(table), key).and_then(parse_toml_bool)
}

/// Read top-level string / scalar when present.
#[allow(dead_code)]
pub fn get_top_level_string(text: &str, key: &str) -> Option<String> {
    value_for_key_in_scope(text, None, key)
        .map(parse_toml_scalar)
        .filter(|s| !s.is_empty())
}

/// Read `[table].key` string / scalar when present.
pub fn get_table_string(text: &str, table: &str, key: &str) -> Option<String> {
    value_for_key_in_scope(text, Some(table), key)
        .map(parse_toml_scalar)
        .filter(|s| !s.is_empty())
}

// ── Duplicate-key scan / heal ────────────────────────────────────────────────

/// Scope id for duplicate detection.
///
/// - Top-level: `""`
/// - Named table `[ui]`: `"ui"` (all `[ui]` sections share one scope — TOML
///   merges them and rejects duplicate keys across fragments)
/// - Array table instance `[[hooks]]`: `"[[hooks]]#N"` so each element is
///   independent (keys may repeat across elements)
fn scope_id_for_header(
    is_array: bool,
    name: &str,
    array_ordinals: &mut HashMap<String, usize>,
) -> String {
    if is_array {
        let n = array_ordinals.entry(name.to_string()).or_insert(0);
        let id = format!("[[{name}]]#{n}");
        *n += 1;
        id
    } else {
        name.to_string()
    }
}

/// Count duplicate assignments: same key twice in the same table scope.
///
/// Returns `(duplicate_assignment_lines, examples)` where examples are
/// `"scope.key"` strings (capped).
pub fn count_duplicate_assignments(text: &str) -> (usize, Vec<String>) {
    let mut scope = String::new();
    let mut array_ordinals: HashMap<String, usize> = HashMap::new();
    let mut last: HashMap<(String, String), usize> = HashMap::new();
    let mut dups = 0usize;
    let mut examples: Vec<String> = Vec::new();

    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if let Some((is_array, name)) = parse_table_header(trimmed) {
            scope = scope_id_for_header(is_array, name, &mut array_ordinals);
            continue;
        }
        let Some(key) = assignment_key(trimmed) else {
            continue;
        };
        let map_key = (scope.clone(), key.to_string());
        if last.insert(map_key, i).is_some() {
            dups += 1;
            if examples.len() < 8 {
                let label = if scope.is_empty() {
                    key.to_string()
                } else {
                    format!("{scope}.{key}")
                };
                if !examples.contains(&label) {
                    examples.push(label);
                }
            }
        }
    }
    (dups, examples)
}

/// Drop earlier duplicate assignments; keep the **last** occurrence per
/// `(table_scope, key)`. Comments, blanks, headers, and unique keys are
/// preserved in order. Valid configs with no dups return an identical string.
pub fn dedupe_assignment_keys(text: &str) -> (String, usize) {
    let (dup_count, _) = count_duplicate_assignments(text);
    if dup_count == 0 {
        return (text.to_string(), 0);
    }

    // Pass 1: last line index for each (scope, key).
    let mut scope = String::new();
    let mut array_ordinals: HashMap<String, usize> = HashMap::new();
    let mut last: HashMap<(String, String), usize> = HashMap::new();
    let lines: Vec<&str> = text.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some((is_array, name)) = parse_table_header(trimmed) {
            scope = scope_id_for_header(is_array, name, &mut array_ordinals);
            continue;
        }
        if let Some(key) = assignment_key(trimmed) {
            last.insert((scope.clone(), key.to_string()), i);
        }
    }

    // Pass 2: emit lines, skip non-last duplicate assignments.
    scope.clear();
    array_ordinals.clear();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut removed = 0usize;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some((is_array, name)) = parse_table_header(trimmed) {
            scope = scope_id_for_header(is_array, name, &mut array_ordinals);
            out.push(line);
            continue;
        }
        if let Some(key) = assignment_key(trimmed) {
            let map_key = (scope.clone(), key.to_string());
            if last.get(&map_key).copied() != Some(i) {
                removed += 1;
                continue;
            }
        }
        out.push(line);
    }

    let mut joined = out.join("\n");
    if (text.ends_with('\n') || text.is_empty()) && !joined.ends_with('\n') {
        joined.push('\n');
    }
    (joined, removed)
}

/// Result of [`ensure_agent_home_config_sane`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigHealReport {
    /// True when the on-disk file was rewritten.
    pub changed: bool,
    /// Absolute path of agent-home config (if resolved).
    pub path: Option<String>,
    /// Number of duplicate assignment lines removed.
    pub removed_duplicates: usize,
    /// Backup path when a heal write occurred.
    pub backup_path: Option<String>,
    /// Short reason when skipped / no-op.
    pub note: &'static str,
}

impl ConfigHealReport {
    fn skipped(note: &'static str) -> Self {
        Self {
            changed: false,
            path: None,
            removed_duplicates: 0,
            backup_path: None,
            note,
        }
    }
}

/// How many times a non-array table header is repeated after its first use.
///
/// The TOML crate rejects a second `[models]` even when the keys do not
/// overlap. Array tables (`[[hooks]]`) are separate elements and are ignored.
pub fn count_repeated_standard_headers(text: &str) -> usize {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut extra = 0usize;
    for line in text.lines() {
        let Some((is_array, name)) = parse_table_header(line.trim()) else {
            continue;
        };
        if is_array {
            continue;
        }
        let n = seen.entry(name.to_string()).or_insert(0);
        if *n > 0 {
            extra += 1;
        }
        *n += 1;
    }
    extra
}

/// Fold later copies of a standard table into the first one.
///
/// Keys stay in that table. A second `[models]` after `[model.pollinations]`
/// must not leave `max_retries` inside the provider table. Array tables are
/// left as separate elements. Returns `(text, headers_removed)`.
pub fn collapse_repeated_standard_tables(text: &str) -> (String, usize) {
    if count_repeated_standard_headers(text) == 0 {
        return (text.to_string(), 0);
    }

    struct Seg {
        header: Option<String>,
        array: bool,
        body: Vec<String>,
    }

    let mut segs: Vec<Seg> = vec![Seg {
        header: None,
        array: false,
        body: Vec::new(),
    }];
    for line in text.lines() {
        if let Some((is_array, name)) = parse_table_header(line.trim()) {
            segs.push(Seg {
                header: Some(name.to_string()),
                array: is_array,
                body: Vec::new(),
            });
            continue;
        }
        segs.last_mut().unwrap().body.push(line.to_string());
    }

    let mut first_of: HashMap<String, usize> = HashMap::new();
    let mut extras: Vec<(usize, Vec<String>)> = Vec::new();
    let mut removed = 0usize;
    for (i, seg) in segs.iter().enumerate() {
        if seg.array {
            continue;
        }
        let Some(name) = seg.header.clone() else {
            continue;
        };
        if let Some(&first) = first_of.get(&name) {
            extras.push((first, seg.body.clone()));
            removed += 1;
        } else {
            first_of.insert(name, i);
        }
    }
    let mut drop_idx: HashMap<usize, ()> = HashMap::new();
    for (first, extra) in extras {
        // Re-find the source index: body was cloned before any mutation.
        segs[first].body.extend(extra);
    }
    for (i, seg) in segs.iter().enumerate() {
        if seg.array {
            continue;
        }
        let Some(name) = &seg.header else {
            continue;
        };
        if first_of.get(name).copied() != Some(i) {
            drop_idx.insert(i, ());
        }
    }

    let mut out: Vec<String> = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        if drop_idx.contains_key(&i) {
            continue;
        }
        if let Some(name) = &seg.header {
            let brackets = if seg.array {
                format!("[[{name}]]")
            } else {
                format!("[{name}]")
            };
            out.push(brackets);
        }
        out.extend(seg.body.iter().cloned());
    }
    let mut joined = out.join("\n");
    if (text.ends_with('\n') || text.is_empty()) && !joined.ends_with('\n') {
        joined.push('\n');
    }
    (joined, removed)
}

/// If independent agent-home `config.toml` has duplicate keys, backup and
/// dedupe (keep last). **Valid files are never written.**
///
/// Shared mode: no-op. Missing / empty file: no-op.
pub fn ensure_agent_home_config_sane(session_data_mode: &str) -> Result<ConfigHealReport, String> {
    if normalize_mode(session_data_mode) == "shared" {
        return Ok(ConfigHealReport::skipped("shared_mode"));
    }

    with_config_write_lock(|| {
        let path = match resolve_writable_config_path(session_data_mode) {
            Ok(p) => p,
            Err(_) => return Ok(ConfigHealReport::skipped("path_refused")),
        };
        if !path.is_file() {
            return Ok(ConfigHealReport {
                changed: false,
                path: Some(path.display().to_string()),
                removed_duplicates: 0,
                backup_path: None,
                note: "missing",
            });
        }
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.trim().is_empty() {
            return Ok(ConfigHealReport {
                changed: false,
                path: Some(path.display().to_string()),
                removed_duplicates: 0,
                backup_path: None,
                note: "empty",
            });
        }

        let (dup_count, examples) = count_duplicate_assignments(&existing);
        let repeated_headers = count_repeated_standard_headers(&existing);
        if dup_count == 0 && repeated_headers == 0 {
            return Ok(ConfigHealReport {
                changed: false,
                path: Some(path.display().to_string()),
                removed_duplicates: 0,
                backup_path: None,
                note: "ok",
            });
        }

        let (deduped, removed_keys) = dedupe_assignment_keys(&existing);
        let (healed, removed_headers) = collapse_repeated_standard_tables(&deduped);
        let removed = removed_keys + removed_headers;
        if removed == 0 || healed == existing {
            return Ok(ConfigHealReport {
                changed: false,
                path: Some(path.display().to_string()),
                removed_duplicates: 0,
                backup_path: None,
                note: "dedupe_noop",
            });
        }
        let (still, _) = count_duplicate_assignments(&healed);
        let still_headers = count_repeated_standard_headers(&healed);
        if still > 0 || still_headers > 0 {
            return Err(format!(
                "agent-home config.toml still has duplicate keys after heal (examples: {})",
                examples.join(", ")
            ));
        }

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = path.with_file_name(format!("config.toml.bak-heal-{nanos}"));
        fs::copy(&path, &backup).map_err(|e| format!("backup config.toml: {e}"))?;
        fs::write(&path, &healed).map_err(|e| format!("write healed config.toml: {e}"))?;

        tracing::warn!(
            target: "agent_home_config",
            path = %path.display(),
            backup = %backup.display(),
            removed,
            examples = %examples.join(","),
            "healed agent-home config.toml duplicate keys (kept last assignment)"
        );

        Ok(ConfigHealReport {
            changed: true,
            path: Some(path.display().to_string()),
            removed_duplicates: removed,
            backup_path: Some(backup.display().to_string()),
            note: "healed",
        })
    })
}

/// Strict write: read → transform → write agent-home config.toml.
/// Shared mode → `Err` via [`resolve_writable_config_path`].
pub fn update_config_toml(
    session_data_mode: &str,
    transform: impl FnOnce(&str) -> String,
) -> Result<PathBuf, String> {
    with_config_write_lock(|| {
        let path = resolve_writable_config_path(session_data_mode)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create agent-home: {e}"))?;
        }
        let existing = fs::read_to_string(&path).unwrap_or_default();
        let next = transform(&existing);
        fs::write(&path, next).map_err(|e| format!("write config: {e}"))?;
        Ok(path)
    })
}

/// Soft-skip shared: `Ok(None)` without touching disk.
/// Independent: apply transform and return `Ok(Some(path))`.
///
/// Prefer this for AppSettings sync helpers that keep the toggle in App state
/// when session mode is shared.
pub fn update_config_toml_if_independent(
    session_data_mode: &str,
    transform: impl FnOnce(&str) -> String,
) -> Result<Option<PathBuf>, String> {
    if normalize_mode(session_data_mode) == "shared" {
        return Ok(None);
    }
    update_config_toml(session_data_mode, transform).map(Some)
}

#[cfg(test)]
mod tests;
