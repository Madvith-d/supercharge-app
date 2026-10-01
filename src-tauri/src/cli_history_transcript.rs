//! Read-only terminal transcript replay. `updates.jsonl` owns UI history.
//!
//! Schema verified against Supercharge 1.3.24 (d7a1754e63c2): shell
//! `session/storage/mod.rs`, `acp_session_impl/{updates,turn}.rs`, and
//! `extensions/notification.rs`; ACP 0.10.4 / protocol-schema 0.11.4.
//! No journal writes, asset extraction, path grants, or CLI execution occur here.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::store::ChatMessageStored;

mod content;
mod legacy;
#[cfg(test)]
mod tests;
mod updates;

const MAX_TRANSCRIPT_BYTES: u64 = 128 * 1024 * 1024;

/// Rebuild from disk on every read, including after truncation or rewind.
/// Missing both files is an error; an existing empty updates file is authoritative.
/// Unknown events/content and malformed complete records fail without falling back.
pub fn read_transcript(dir: &Path) -> Result<Vec<ChatMessageStored>, String> {
    match read_snapshot(&dir.join("updates.jsonl"))? {
        Some(raw) => updates::parse(dir, records(&raw, "updates.jsonl")?),
        None => match read_snapshot(&dir.join("chat_history.jsonl"))? {
            Some(raw) => legacy::parse(dir, records(&raw, "chat_history.jsonl")?),
            None => Err("CLI transcript missing: no updates.jsonl or chat_history.jsonl".into()),
        },
    }
}

fn read_snapshot(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let entry = match std::fs::symlink_metadata(path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("stat CLI transcript: {}", error.kind())),
    };
    let target = if entry.file_type().is_symlink() {
        std::fs::metadata(path).map_err(|e| format!("stat CLI transcript target: {}", e.kind()))?
    } else {
        entry
    };
    // Reject pipes/devices before opening; broken links must not trigger fallback.
    if !target.is_file() {
        return Err("CLI transcript is not a regular file".into());
    }
    let file = File::open(path).map_err(|e| format!("read CLI transcript: {}", e.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("stat CLI transcript: {}", e.kind()))?;
    if !metadata.is_file() {
        return Err("CLI transcript is not a regular file".into());
    }
    if metadata.len() > MAX_TRANSCRIPT_BYTES {
        return Err("CLI transcript exceeds 128 MiB read limit".into());
    }
    let mut raw = Vec::new();
    // Snapshot the opening length: concurrent appends cannot extend this read forever.
    file.take(metadata.len())
        .read_to_end(&mut raw)
        .map_err(|e| format!("read CLI transcript: {}", e.kind()))?;
    Ok(Some(raw))
}

struct Record {
    value: Value,
    offset: usize,
    line: usize,
}

impl Record {
    fn error(&self, message: &str) -> String {
        // Never include source content, tool arguments, or serde's token diagnostics.
        format!("CLI transcript line {}: {message}", self.line)
    }
}

fn records(raw: &[u8], source: &str) -> Result<Vec<Record>, String> {
    let mut result = Vec::new();
    let mut offset = 0;
    for (index, bytes) in raw.split_inclusive(|b| *b == b'\n').enumerate() {
        let terminated = bytes.ends_with(b"\n");
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            match serde_json::from_slice(bytes) {
                Ok(value) => result.push(Record {
                    value,
                    offset,
                    line: index + 1,
                }),
                Err(error) if !terminated && error.is_eof() => break,
                Err(_)
                    if !terminated
                        && std::str::from_utf8(bytes).is_err_and(|e| e.error_len().is_none()) =>
                {
                    break
                }
                Err(_) => return Err(format!("invalid {source} JSON at line {}", index + 1)),
            }
        }
        offset += bytes.len();
    }
    Ok(result)
}

fn timestamp(record: &Record, params: &Value) -> Result<DateTime<Utc>, String> {
    if let Some(value) = params.pointer("/_meta/agentTimestampMs") {
        return value
            .as_i64()
            .and_then(DateTime::from_timestamp_millis)
            .ok_or_else(|| record.error("invalid agentTimestampMs"));
    }
    if let Some(value) = record.value.get("timestamp") {
        let parsed = if let Some(seconds) = value.as_i64() {
            DateTime::from_timestamp(seconds, 0)
        } else {
            value
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
        };
        return parsed.ok_or_else(|| record.error("invalid timestamp"));
    }
    for key in ["created_at", "createdAt"] {
        if let Some(value) = record.value.get(key) {
            return value
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc))
                .ok_or_else(|| record.error("invalid created_at"));
        }
    }
    // Old logs have no timestamp. Never manufacture a different time on each read.
    Ok(DateTime::<Utc>::UNIX_EPOCH)
}

fn message(
    dir: &Path,
    source: &str,
    record: &Record,
    params: &Value,
    role: &str,
) -> Result<ChatMessageStored, String> {
    let mut hash = Sha256::new();
    for part in [
        dir.to_string_lossy().as_ref(),
        source,
        role,
        &record.offset.to_string(),
        params
            .pointer("/_meta/eventId")
            .and_then(Value::as_str)
            .unwrap_or(""),
    ] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    Ok(ChatMessageStored {
        id: format!("cli-{:x}", hash.finalize()),
        role: role.into(),
        content: String::new(),
        thought: None,
        created_at: timestamp(record, params)?,
        is_error: false,
        attachments: None,
        marker: None,
    })
}
