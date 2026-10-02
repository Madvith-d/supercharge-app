use std::path::Path;

use serde_json::Value;

use super::{content, message, Record};
use crate::store::ChatMessageStored;

/// Compatibility only: model history cannot reproduce terminal chrome or streaming.
pub(super) fn parse(dir: &Path, records: Vec<Record>) -> Result<Vec<ChatMessageStored>, String> {
    let mut rows = Vec::new();
    for record in records {
        let value = &record.value;
        let role = value
            .get("type")
            .or_else(|| value.get("role"))
            .and_then(Value::as_str)
            .ok_or_else(|| record.error("missing legacy message role"))?;
        match role {
            "user" | "assistant" => {
                // Keep the existing importer sanitization as the legacy authority.
                let parsed = crate::cli_sessions::parse_chat_history_text(&value.to_string())
                    .unwrap_or_default();
                let mut row = message(dir, "chat_history", &record, value, role)?;
                if let Some((_, text)) = parsed.into_iter().next() {
                    row.content = text;
                }
                if let Some(blocks) = value.get("content").and_then(Value::as_array) {
                    for block in blocks {
                        match block.get("type").and_then(Value::as_str) {
                            Some("image" | "audio" | "resource" | "resource_link") => {
                                content::block(block, &mut row).map_err(|e| record.error(e))?;
                            }
                            Some("text") | None => {}
                            _ => return Err(record.error("unsupported legacy content block")),
                        }
                    }
                }
                if role == "user" {
                    if content::hidden(value)
                        || (content::synthetic(value)
                            && !value.to_string().contains("<user_query>"))
                    {
                        continue;
                    }
                    if !content::clean_user(&mut row) {
                        continue;
                    }
                }
                if !row.content.is_empty() || row.attachments.is_some() {
                    rows.push(row);
                }
            }
            "tool_result" => {
                let call_id = value
                    .get("tool_call_id")
                    .or_else(|| value.get("toolCallId"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| record.error("missing legacy tool call ID"))?;
                let mut row = message(dir, "chat_history", &record, value, "assistant")?;
                if let Some(body) = value.get("content") {
                    if let Some(text) = body.as_str() {
                        row.content = text.to_string();
                    } else if let Some(blocks) = body.as_array() {
                        for block in blocks {
                            if !row.content.is_empty() {
                                row.content.push('\n');
                            }
                            content::block(block, &mut row).map_err(|e| record.error(e))?;
                        }
                    } else {
                        return Err(record.error("unsupported legacy tool result"));
                    }
                }
                row.is_error = value
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let status = if row.is_error { "failed" } else { "completed" };
                row.content = format!(
                    "tool_step|{status}|tool|{}\n{}",
                    call_id.replace(['\r', '\n', '|'], " "),
                    row.content
                );
                rows.push(row);
            }
            // Raw model-only rows are never terminal user bubbles.
            "system" | "developer" | "tool_call" | "reasoning" => {}
            _ => return Err(record.error("unsupported legacy message role")),
        }
    }
    Ok(rows)
}
