//! Source-aware adapters for the existing Account CLI history APIs.

use super::*;
use crate::cli_sessions::CliSessionSummary;

pub fn list() -> Vec<CliSessionSummary> {
    let roots = allowed_source_homes();
    summaries(&discover(&roots), &store::load_sessions_index())
}

pub(super) fn summaries(
    found: &[DiscoveredSession],
    index: &[SessionMeta],
) -> Vec<CliSessionSummary> {
    let existing_ids: HashSet<_> = index.iter().map(|meta| meta.id.as_str()).collect();
    let mut projected = index.to_vec();
    merge_discovered(&mut projected, found, &BTreeSet::new());
    let links: HashMap<_, _> = projected
        .iter()
        .filter(|meta| existing_ids.contains(meta.id.as_str()) && !is_continuation_copy(meta))
        .filter_map(|meta| Some((meta.cli_source.as_ref()?.key(), meta.id.clone())))
        .collect();
    let mut rows: Vec<_> = found
        .iter()
        .map(|row| {
            let dir = Path::new(&row.source.source_home).join(&row.source.relative_dir);
            let summary = read_summary(&dir);
            let app_session_id = links.get(&row.source.key()).cloned();
            CliSessionSummary {
                agent_session_id: row.source.agent_session_id.clone(),
                title: row.title.clone(),
                cwd: row.source.cwd.clone(),
                updated_at: row.updated_at.to_rfc3339(),
                dir: dir.to_string_lossy().into_owned(),
                num_messages: summary
                    .get("num_chat_messages")
                    .or_else(|| summary.get("num_messages"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    .min(u32::MAX as u64) as u32,
                already_linked: app_session_id.is_some(),
                app_session_id,
                source_home: row.source.source_home.clone(),
                first_prompt: None,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.dir.cmp(&b.dir))
    });
    rows
}

pub(super) fn select_source<'a>(
    found: &'a [DiscoveredSession],
    id: &str,
    dir: Option<&str>,
) -> Result<&'a DiscoveredSession, String> {
    let id = crate::cli_sessions::validate_agent_session_id(id)?;
    let canonical = dir
        .filter(|dir| !dir.trim().is_empty())
        .map(|dir| fs::canonicalize(dir).map_err(|e| e.to_string()))
        .transpose()?;
    let mut matches = found.iter().filter(|row| {
        row.source.agent_session_id == id
            && canonical.as_ref().is_none_or(|dir| {
                Path::new(&row.source.source_home).join(&row.source.relative_dir) == *dir
            })
    });
    let row = matches
        .next()
        .ok_or("CLI session source not found in allowed homes")?;
    if matches.next().is_some() {
        return Err("Ambiguous CLI session id; pass the directory from the history list".into());
    }
    Ok(row)
}

/// Explicit import restores a source link, but does not claim it for execution.
pub fn import(
    id: &str,
    dir: Option<&str>,
    project_id: Option<String>,
) -> Result<SessionMeta, String> {
    let roots = allowed_source_homes();
    let found = discover(&roots);
    let selected = select_source(&found, id, dir)?;
    resolve_source_in(&selected.source, &roots)?;
    let project_id = project_id.filter(|id| !id.trim().is_empty());
    if let Some(id) = &project_id {
        if !store::load_projects_metadata()
            .iter()
            .any(|project| &project.id == id)
        {
            return Err("Project not found".into());
        }
    }
    let linked = store::update_sessions_index(|index| {
        Ok(link_selected(index, &found, selected, project_id))
    })?;
    store::update_sessions_index(|index| {
        if !index.iter().any(|meta| {
            meta.id == linked.id
                && meta.cli_source.as_ref().map(CliSessionSource::key)
                    == Some(selected.source.key())
        }) {
            return Err("Session was removed while importing".into());
        }
        let mut suppressed = load_suppressions(&suppression_path())?;
        if suppressed.remove(&selected.source.key()) {
            crate::store_lock::write_bytes_replace(
                &suppression_path(),
                &serde_json::to_vec(&suppressed).map_err(|e| e.to_string())?,
            )?;
        }
        Ok(linked)
    })
}

pub(super) fn link_selected(
    index: &mut Vec<SessionMeta>,
    found: &[DiscoveredSession],
    selected: &DiscoveredSession,
    project_id: Option<String>,
) -> SessionMeta {
    let mut projected = index.clone();
    merge_discovered(&mut projected, found, &BTreeSet::new());
    let mut meta = projected
        .into_iter()
        .find(|meta| {
            !is_continuation_copy(meta)
                && meta
                    .cli_source
                    .as_ref()
                    .is_some_and(|source| source.key() == selected.source.key())
        })
        .expect("discovered source was projected");
    if let Some(existing) = index.iter_mut().find(|existing| existing.id == meta.id) {
        *existing = meta.clone();
    } else {
        meta.project_id = project_id;
        index.push(meta.clone());
    }
    meta
}

pub fn import_all(limit: usize, legacy_deleted: &[String]) -> Result<Vec<SessionMeta>, String> {
    let found = discover(&allowed_source_homes());
    let mut counts = HashMap::<&str, usize>::new();
    for row in &found {
        *counts.entry(&row.source.agent_session_id).or_default() += 1;
    }
    store::update_sessions_index(|index| {
        let suppressed = load_suppressions(&suppression_path())?;
        let old_ids: HashSet<_> = index.iter().map(|meta| meta.id.clone()).collect();
        let mut projected = index.clone();
        merge_discovered(&mut projected, &found, &suppressed);
        let mut imported = Vec::new();
        for meta in projected {
            if let Some(existing) = index.iter_mut().find(|existing| existing.id == meta.id) {
                *existing = meta;
                continue;
            }
            let source = meta.cli_source.as_ref().expect("new source row");
            let legacy_suppressed = counts.get(source.agent_session_id.as_str()) == Some(&1)
                && legacy_deleted.contains(&source.agent_session_id);
            if !old_ids.contains(&meta.id) && !legacy_suppressed && imported.len() < limit {
                index.push(meta.clone());
                imported.push(meta);
            }
        }
        Ok(imported)
    })
}

pub fn latest(cwd: &str) -> Option<CliSessionSummary> {
    if cwd.trim().is_empty() {
        return None;
    }
    list().into_iter().find(|row| {
        row.cwd
            .as_deref()
            .is_some_and(|candidate| cwd_matches(candidate, cwd))
    })
}

pub(super) fn cwd_matches(a: &str, b: &str) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => {
            #[cfg(windows)]
            {
                crate::cli_sessions::cwd_paths_match(a, b)
            }
            #[cfg(not(windows))]
            {
                Path::new(a) == Path::new(b)
            }
        }
    }
}

/// Refuse known active owners. CLI write locks alone do not prove inactivity.
pub(super) fn reject_active_source(source: &CliSessionSource, dir: &Path) -> Result<(), String> {
    let registry = Path::new(&source.source_home).join("active_sessions.json");
    match fs::symlink_metadata(&registry) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > SUMMARY_LIMIT {
                return Err("Cannot verify CLI session ownership".into());
            }
            let entries: Vec<serde_json::Value> =
                serde_json::from_slice(&fs::read(&registry).map_err(|e| e.to_string())?)
                    .map_err(|_| "Cannot verify CLI session ownership".to_string())?;
            if entries.iter().any(|entry| {
                entry.get("session_id").and_then(serde_json::Value::as_str)
                    == Some(&source.agent_session_id)
            }) {
                return Err("CLI session is registered as active; close it before deletion".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    if dir.join("turn_lease.json").exists() {
        return Err("CLI session has a turn lease; close it before deletion".into());
    }
    Ok(())
}

pub fn delete(id: &str, dir: Option<&str>) -> Result<(), String> {
    let roots = allowed_source_homes();
    let found = discover(&roots);
    let source = &select_source(&found, id, dir)?.source;
    let target = resolve_source_in(source, &roots)?;
    reject_active_source(source, &target)?;
    let mut locks = Vec::new();
    for name in [".lock", "app_execution.lock"] {
        let path = target.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err("Unsafe CLI session lock".into());
                }
                let file = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|e| e.to_string())?;
                fs2::FileExt::try_lock_exclusive(&file)
                    .map_err(|_| "CLI session is active".to_string())?;
                locks.push(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    reject_active_source(source, &target)?;
    if resolve_source_in(source, &roots)? != target {
        return Err("CLI session source changed".into());
    }
    fs::remove_dir_all(target).map_err(|e| e.to_string())
}
