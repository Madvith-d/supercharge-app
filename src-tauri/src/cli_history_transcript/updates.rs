use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use super::{content, message, Record};
use crate::store::ChatMessageStored;

struct Event<'a> {
    record: &'a Record,
    params: &'a Value,
    update: &'a Value,
    kind: &'a str,
    extension: bool,
}

impl<'a> Event<'a> {
    fn new(record: &'a Record) -> Result<Self, String> {
        let (params, extension) = match record.value.get("method") {
            Some(Value::String(method))
                if method == "session/update" || method == "_x.ai/session/update" =>
            {
                (
                    record
                        .value
                        .get("params")
                        .ok_or_else(|| record.error("missing notification params"))?,
                    method == "_x.ai/session/update",
                )
            }
            None => (&record.value, false),
            _ => return Err(record.error("unsupported notification method")),
        };
        params
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| record.error("missing or invalid sessionId"))?;
        let update = params
            .get("update")
            .ok_or_else(|| record.error("missing update"))?;
        let kind = content::required_text(update, "sessionUpdate").map_err(|e| record.error(e))?;
        Ok(Self {
            record,
            params,
            update,
            kind,
            extension,
        })
    }

    fn prompt_index(&self) -> Result<Option<u64>, String> {
        self.update
            .pointer("/_meta/promptIndex")
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| self.record.error("invalid promptIndex"))
            })
            .transpose()
    }
}

/// Match the CLI's progressive UserRunTurnTracker, not visible bubble counts.
fn survivors(records: &[Record]) -> Result<Vec<Event<'_>>, String> {
    let mut result = Vec::new();
    let mut starts = Vec::new();
    let mut seen_marker = false;
    let mut in_user = false;
    let mut current_pi = None;
    let mut session_id = None;
    for record in records {
        let event = Event::new(record)?;
        let sid = event.params["sessionId"].as_str().unwrap_or_default();
        if session_id.is_some_and(|id| id != sid) {
            return Err(record.error("mixed session IDs"));
        }
        session_id = Some(sid);
        if event.extension && event.kind == "rewind_marker" {
            let target = event
                .update
                .get("target_prompt_index")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| record.error("invalid rewind target"))?;
            // Out-of-range rewinds are no-ops in the CLI's replay algorithm.
            result.truncate(starts.get(target).copied().unwrap_or(result.len()));
            starts.truncate(target);
            in_user = false;
            current_pi = None;
            continue;
        }
        if !event.extension
            && event.kind == "user_message_chunk"
            && event
                .update
                .pointer("/_meta/hostTurn")
                .and_then(Value::as_bool)
                != Some(true)
        {
            let pi = event.prompt_index()?;
            seen_marker |= pi.is_some();
            let new_run = !in_user || (seen_marker && pi != current_pi);
            if new_run && (!seen_marker || pi.is_some()) {
                starts.push(result.len());
            }
            if new_run {
                current_pi = pi;
            }
            in_user = true;
        } else {
            in_user = false;
            current_pi = None;
        }
        result.push(event);
    }
    Ok(result)
}

struct OpenMessage {
    index: usize,
    role: &'static str,
    message_id: Option<String>,
    prompt_index: Option<u64>,
    hidden: bool,
    synthetic: bool,
}

fn finish(
    open: &mut Option<OpenMessage>,
    rows: &mut [ChatMessageStored],
    dropped: &mut HashSet<usize>,
) {
    if let Some(open) = open.take() {
        let row = &mut rows[open.index];
        if open.role == "user"
            && (open.hidden
                || (open.synthetic && !row.content.contains("<user_query>"))
                || !content::clean_user(row))
        {
            dropped.insert(open.index);
        }
    }
}

pub(super) fn parse(dir: &Path, records: Vec<Record>) -> Result<Vec<ChatMessageStored>, String> {
    let events = survivors(&records)?;
    let mut rows = Vec::new();
    let mut dropped = HashSet::new();
    let mut open: Option<OpenMessage> = None;
    let mut tools: HashMap<String, (usize, Value)> = HashMap::new();
    for event in events {
        let Event {
            record,
            params,
            update,
            kind,
            extension,
        } = &event;
        super::timestamp(record, params)?;
        if *extension {
            match *kind {
                "turn_completed" | "response_started" | "response_completed" => {
                    finish(&mut open, &mut rows, &mut dropped);
                    // agent_result is a turn summary, not an additional assistant message.
                }
                k if ignored_extension(k) => {
                    if open.as_ref().is_some_and(|m| m.role == "user") {
                        finish(&mut open, &mut rows, &mut dropped);
                    }
                }
                _ => return Err(record.error("unsupported extension update")),
            }
            continue;
        }
        match *kind {
            "user_message_chunk" | "agent_message_chunk" | "agent_thought_chunk" => {
                let role = if *kind == "user_message_chunk" {
                    "user"
                } else {
                    "assistant"
                };
                let message_id = update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let pi = event.prompt_index()?;
                let compatible = open.as_ref().is_some_and(|m| {
                    m.role == role
                        && m.message_id == message_id
                        && (role != "user" || m.prompt_index == pi)
                });
                if !compatible {
                    finish(&mut open, &mut rows, &mut dropped);
                    let index = rows.len();
                    rows.push(message(dir, "updates", record, params, role)?);
                    open = Some(OpenMessage {
                        index,
                        role,
                        message_id,
                        prompt_index: pi,
                        hidden: false,
                        synthetic: false,
                    });
                }
                let current = open.as_mut().expect("opened above");
                current.hidden |= content::hidden(update)
                    || content::hidden(params)
                    || content::hidden(&record.value);
                current.synthetic |= content::synthetic(update)
                    || content::synthetic(params)
                    || content::synthetic(&record.value);
                let row = &mut rows[current.index];
                let block = update
                    .get("content")
                    .ok_or_else(|| record.error("missing content block"))?;
                if *kind == "agent_thought_chunk" {
                    if block.get("type").and_then(Value::as_str) != Some("text") {
                        return Err(record.error("unsupported thought content"));
                    }
                    row.thought.get_or_insert_with(String::new).push_str(
                        content::required_text(block, "text").map_err(|e| record.error(e))?,
                    );
                } else {
                    content::block(block, row).map_err(|e| record.error(e))?;
                }
            }
            "tool_call" | "tool_call_update" => {
                finish(&mut open, &mut rows, &mut dropped);
                let id =
                    content::required_text(update, "toolCallId").map_err(|e| record.error(e))?;
                if id.is_empty() {
                    return Err(record.error("empty toolCallId"));
                }
                if !tools.contains_key(id) {
                    let index = rows.len();
                    rows.push(message(dir, "updates", record, params, "assistant")?);
                    tools.insert(id.to_string(), (index, serde_json::json!({})));
                }
                let (index, merged) = tools.get_mut(id).expect("inserted above");
                // ACP updates replace collections. Missing/null fields preserve prior values.
                for (key, value) in update
                    .as_object()
                    .ok_or_else(|| record.error("invalid tool update"))?
                {
                    if value.is_null() {
                        continue;
                    }
                    if key == "_meta" {
                        let fields = value
                            .as_object()
                            .ok_or_else(|| record.error("invalid tool metadata"))?;
                        for (field, value) in fields {
                            merged[key][field] = value.clone();
                        }
                    } else {
                        merged[key] = value.clone();
                    }
                }
                render_tool(merged, &mut rows[*index]).map_err(|e| record.error(e))?;
            }
            "plan"
            | "available_commands_update"
            | "current_mode_update"
            | "config_option_update"
            | "session_info_update"
            | "usage_update" => {
                if open.as_ref().is_some_and(|m| m.role == "user") {
                    finish(&mut open, &mut rows, &mut dropped);
                }
            }
            _ => return Err(record.error("unsupported ACP update")),
        }
    }
    finish(&mut open, &mut rows, &mut dropped);
    Ok(rows
        .into_iter()
        .enumerate()
        .filter_map(|(i, row)| {
            (!dropped.contains(&i)
                && (!row.content.is_empty()
                    || row.thought.as_ref().is_some_and(|s| !s.is_empty())
                    || row.attachments.is_some()))
            .then_some(row)
        })
        .collect())
}

fn render_tool(update: &Value, row: &mut ChatMessageStored) -> Result<(), &'static str> {
    for key in ["status", "title", "kind"] {
        if update.get(key).is_some_and(|value| !value.is_string()) {
            return Err("invalid tool string field");
        }
    }
    let status = update
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending");
    if !matches!(status, "pending" | "in_progress" | "completed" | "failed") {
        return Err("unsupported tool status");
    }
    let meta = update.pointer("/_meta/x.ai~1tool");
    let name = meta
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .or_else(|| update.get("kind").and_then(Value::as_str))
        .unwrap_or("tool");
    let title = meta
        .and_then(|m| m.get("label"))
        .and_then(Value::as_str)
        .or_else(|| update.get("title").and_then(Value::as_str))
        .unwrap_or(name);
    let line = |s: &str| s.replace(['\r', '\n', '|'], " ");
    row.content.clear();
    row.attachments = None;
    if let Some(content) = update.get("content") {
        content::tool_content(content, row)?;
    }
    if row.content.trim().is_empty() {
        if let Some(raw) = update.get("rawOutput") {
            row.content = raw
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| raw.to_string());
        }
    }
    let mut header = format!("tool_step|{status}|{}|{}", line(name), line(title));
    if let Some(input) = update.get("rawInput") {
        let input = input
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| input.to_string());
        header.push_str("\ninput:");
        header.push_str(&line(&input));
    }
    if !row.content.is_empty() {
        header.push('\n');
        header.push_str(&row.content);
    }
    row.content = header;
    row.is_error = status == "failed";
    Ok(())
}

// Known terminal chrome/control events have no ChatMessageStored representation.
fn ignored_extension(kind: &str) -> bool {
    matches!(
        kind,
        "diff_review"
            | "retry_state"
            | "auto_compact_started"
            | "auto_compact_completed"
            | "auto_compact_failed"
            | "auto_compact_cancelled"
            | "auto_continue_completed"
            | "memory_flush_started"
            | "memory_flush_completed"
            | "memory_capture_activity"
            | "memory_dream_queued"
            | "memory_dream_started"
            | "memory_dream_completed"
            | "memory_session_saved"
            | "feedback_request"
            | "relay_sync_status"
            | "auto_recovery_started"
            | "auto_recovery_exhausted"
            | "hook_annotation"
            | "hook_run_started"
            | "hook_execution"
            | "hooks_changed"
            | "plugins_changed"
            | "plugin_updates_installed"
            | "session_status"
            | "session_summary_generated"
            | "session_recap"
            | "session_recap_unavailable"
            | "last_turn_summary"
            | "compaction_checkpoint"
            | "task_completed"
            | "subagent_spawned"
            | "subagent_progress"
            | "subagent_finished"
            | "task_backgrounded"
            | "background_tasks"
            | "scheduled_task_created"
            | "scheduled_task_fired"
            | "scheduled_task_deleted"
            | "monitor_event"
            | "model_auto_switched"
            | "model_changed"
            | "tool_call_delta_chunk"
            | "image_compressed"
            | "image_dropped"
            | "memory_files"
            | "workflow_updated"
            | "goal_updated"
            | "pending_interaction"
            | "interaction_resolved"
            | "plan_kept"
            | "plan_cleared"
            | "plan_executing"
            | "reasoning_completed"
    )
}
