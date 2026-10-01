//! Source transcript caches are separate from the App-owned messages journal.

use super::*;

#[derive(Serialize, Deserialize)]
struct TranscriptCache {
    source_home: String,
    relative_dir: String,
    revision: String,
    messages: Vec<ChatMessageStored>,
}

fn cache_path(id: &str) -> PathBuf {
    crate::paths::session_dir(id).join("cli_transcript.json")
}

fn journal_path(id: &str) -> PathBuf {
    crate::paths::session_dir(id).join("messages.json")
}

fn cached(id: &str, source: &CliSessionSource) -> Option<TranscriptCache> {
    let cache: TranscriptCache = serde_json::from_slice(&fs::read(cache_path(id)).ok()?).ok()?;
    (cache.source_home == source.source_home && cache.relative_dir == source.relative_dir)
        .then_some(cache)
}

fn owns_journal(meta: &SessionMeta) -> bool {
    owns_journal_with(meta, journal_path(&meta.id).exists())
}

pub(super) fn owns_journal_with(meta: &SessionMeta, journal_exists: bool) -> bool {
    meta.cli_source.as_ref().is_some_and(|source| source.app_owned)
        || meta.fork_agent_session
        || meta.fork_rewind_prompt_index.is_some()
        // An intentionally empty rewind journal is also authoritative.
        || journal_exists
}

/// `None` means a normal App chat. Source chats always bypass mode-based reconcile.
/// Cache refresh never writes messages.json, including during an active App turn.
pub fn read_messages(id: &str) -> Result<Option<Vec<ChatMessageStored>>, String> {
    let Some(meta) = store::load_sessions_index()
        .into_iter()
        .find(|meta| meta.id == id)
    else {
        return Ok(None);
    };
    read_messages_with_meta(&meta)
}

/// Reuse a caller's index snapshot; final ownership/source validation remains locked.
pub fn read_messages_with_meta(
    meta: &SessionMeta,
) -> Result<Option<Vec<ChatMessageStored>>, String> {
    let id = meta.id.as_str();
    let Some(source) = meta.cli_source.as_ref() else {
        return Ok(None);
    };
    if owns_journal(meta) {
        return Ok(Some(store::load_messages(id)));
    }
    let previous = cached(id, source);
    let parsed: Result<(String, Vec<ChatMessageStored>), String> = (|| {
        let dir = resolve_source(source)?;
        if !["updates.jsonl", "chat_history.jsonl"]
            .iter()
            .any(|name| dir.join(name).is_file())
        {
            return Ok((file_revision(&dir), Vec::new()));
        }
        let revision = file_revision(&dir);
        if let Some(cache) = &previous {
            if cache.revision == revision {
                return Ok((revision, cache.messages.clone()));
            }
        }
        let messages = crate::cli_history_transcript::read_transcript(&dir)?;
        // A writer may replace or append the transcript while it is being parsed.
        if resolve_source(source)? != dir || file_revision(&dir) != revision {
            return Err("CLI transcript changed while being read; retry shortly".into());
        }
        Ok((revision, messages))
    })();
    let (revision, messages) = match parsed {
        Ok(result) => result,
        Err(error) => {
            let cache = previous.as_ref().ok_or(error)?;
            (cache.revision.clone(), cache.messages.clone())
        }
    };
    // Cache hits also recheck ownership/source under the journal writers' index lock.
    store::update_sessions_index(|list| {
        let current = list
            .iter()
            .find(|meta| meta.id == id)
            .ok_or("Session was removed")?;
        if owns_journal(current) {
            return Ok(Some(store::load_messages(id)));
        }
        if current.cli_source.as_ref().map(CliSessionSource::key) != Some(source.key()) {
            return Err("CLI history source changed while being read".into());
        }
        if previous
            .as_ref()
            .is_none_or(|cache| cache.revision != revision)
        {
            if resolve_source(source).ok().map(|dir| file_revision(&dir)) != Some(revision.clone())
            {
                return cached(id, source)
                    .map(|cache| Some(cache.messages))
                    .ok_or_else(|| {
                        "CLI transcript changed before cache commit; retry shortly".into()
                    });
            }
            let cache = TranscriptCache {
                source_home: source.source_home.clone(),
                relative_dir: source.relative_dir.clone(),
                revision,
                messages: messages.clone(),
            };
            let raw = serde_json::to_vec(&cache).map_err(|e| e.to_string())?;
            crate::store_lock::write_bytes_replace(&cache_path(id), &raw)?;
        }
        Ok(Some(messages))
    })
}

/// Initialize the App journal from the exact native snapshot selected for execution.
pub fn materialize_execution(id: &str, directory: &Path) -> Result<(), String> {
    if journal_path(id).exists() {
        return Ok(());
    }
    let messages = crate::cli_history_transcript::read_transcript(directory)?;
    store::update_sessions_index(|list| {
        let meta = list
            .iter_mut()
            .find(|meta| meta.id == id)
            .ok_or("Session was removed")?;
        if journal_path(id).exists() {
            return Ok(());
        }
        let raw = serde_json::to_vec(&messages).map_err(|e| e.to_string())?;
        crate::store_lock::write_bytes_replace(&journal_path(id), &raw)?;
        if let Some(source) = meta.cli_source.as_mut() {
            source.app_owned = true;
        }
        Ok(())
    })
}

/// Claim a source snapshot before App execution or an intentional journal edit.
/// This does not authorize a cwd, choose a provider, or resume/copy CLI state.
pub fn materialize_for_app(id: &str) -> Result<Vec<ChatMessageStored>, String> {
    let messages = read_messages(id)?.unwrap_or_else(|| store::load_messages(id));
    store::update_sessions_index(|list| {
        let meta = list
            .iter_mut()
            .find(|meta| meta.id == id)
            .ok_or("Session was removed")?;
        if meta.cli_source.is_none() || owns_journal(meta) {
            if let Some(source) = meta.cli_source.as_mut() {
                source.app_owned = true;
            }
            return Ok(store::load_messages(id));
        }
        let raw = serde_json::to_vec(&messages).map_err(|e| e.to_string())?;
        crate::store_lock::write_bytes_replace(&journal_path(id), &raw)?;
        if let Some(source) = meta.cli_source.as_mut() {
            source.app_owned = true;
        }
        Ok(messages)
    })
}
