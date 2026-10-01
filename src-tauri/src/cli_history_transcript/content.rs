use base64::Engine;
use serde_json::Value;

use crate::store::{ChatMessageStored, MessageAttachmentStored};

/// ACP blocks are typed; arbitrary objects are never interpreted as message text.
pub(super) fn block(value: &Value, row: &mut ChatMessageStored) -> Result<(), &'static str> {
    match value.get("type").and_then(Value::as_str) {
        Some("text") => row.content.push_str(required_text(value, "text")?),
        Some("resource_link") => {
            // Editor context links are not user attachments.
            if value.pointer("/_meta/source").and_then(Value::as_str) != Some("editor") {
                attach_uri(
                    row,
                    required_text(value, "uri")?,
                    value.get("name").and_then(Value::as_str),
                )?;
            }
        }
        Some("resource") => {
            let resource = value.get("resource").ok_or("missing embedded resource")?;
            attach_uri(row, required_text(resource, "uri")?, None)?;
        }
        Some("image") | Some("audio") => {
            if let Some(uri) = value
                .get("uri")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                attach_uri(row, uri, None)?;
            } else {
                let mime = required_text(value, "mimeType")?;
                let expected = if value["type"] == "image" {
                    "image/"
                } else {
                    "audio/"
                };
                if !mime.starts_with(expected)
                    || mime.len() == expected.len()
                    || !mime
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/-+._".contains(&b))
                {
                    return Err("invalid inline attachment MIME type");
                }
                let data = required_text(value, "data")?;
                if data.is_empty()
                    || base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .is_err()
                {
                    return Err("invalid inline attachment base64");
                }
                attach(
                    row,
                    format!("data:{mime};base64,{data}"),
                    Some(&format!("inline.{}", mime.trim_start_matches(expected))),
                );
            }
        }
        _ => return Err("unsupported ACP content block"),
    }
    Ok(())
}

pub(super) fn required_text<'a>(value: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or("missing or invalid string field")
}

fn attach_uri(
    row: &mut ChatMessageStored,
    uri: &str,
    name: Option<&str>,
) -> Result<(), &'static str> {
    let url = url::Url::parse(uri).map_err(|_| "unsupported attachment URI")?;
    let path = match url.scheme() {
        "https" | "http" if url.host_str().is_some() => uri.to_string(),
        "file" if url.host_str().is_none() || url.host_str() == Some("localhost") => {
            let path = percent_encoding::percent_decode_str(url.path())
                .decode_utf8()
                .map_err(|_| "invalid attachment path encoding")?
                .into_owned();
            if path.contains('\0') {
                return Err("invalid attachment path");
            }
            if path.as_bytes().get(2) == Some(&b':') {
                path[1..].to_string()
            } else {
                path
            }
        }
        _ => return Err("unsupported attachment URI"),
    };
    attach(row, path, name);
    Ok(())
}

fn attach(row: &mut ChatMessageStored, path: String, name: Option<&str>) {
    let name = name
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string());
    let attachments = row.attachments.get_or_insert_with(Vec::new);
    if !attachments.iter().any(|a| a.path == path) {
        attachments.push(MessageAttachmentStored {
            path,
            name,
            is_dir: false,
        });
    }
}

pub(super) fn hidden(value: &Value) -> bool {
    ["/_meta/hideFromScrollback", "/_meta/hostTurn"]
        .iter()
        .any(|p| value.pointer(p).and_then(Value::as_bool) == Some(true))
}

pub(super) fn synthetic(value: &Value) -> bool {
    [
        "/synthetic_reason",
        "/syntheticReason",
        "/_meta/synthetic_reason",
        "/metadata/synthetic_reason",
    ]
    .iter()
    .filter_map(|p| value.pointer(p).and_then(Value::as_str))
    .any(|s| {
        let s = s.to_ascii_lowercase();
        s.contains("instruction") || s.contains("reminder")
    })
}

/// Strip model-only wrappers after chunk assembly, including split XML tags.
pub(super) fn clean_user(row: &mut ChatMessageStored) -> bool {
    let mut text = row.content.clone();
    if let Some(start) = text.find("<user_query>") {
        let rest = &text[start + "<user_query>".len()..];
        let Some(end) = rest.find("</user_query>") else {
            return false;
        };
        text = rest[..end].trim().to_string();
    } else {
        for tag in [
            "system-reminder",
            "user_info",
            "fork-context",
            "resume-context",
            "project_instructions",
            "user_instructions",
            "environment_context",
        ] {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            while let Some(start) = text.find(&open) {
                let Some(end) = text[start + open.len()..].find(&close) else {
                    return false;
                };
                text.replace_range(start..start + open.len() + end + close.len(), "");
            }
        }
        // A pending instruction prefix must not become a bubble mid-write.
        let trimmed = text.trim();
        if trimmed.starts_with('<') && !trimmed.contains('>') {
            return false;
        }
    }
    let (text, attachments) = crate::cli_sessions::parse_at_path_attachments(&text);
    row.content = text.trim().to_string();
    for attachment in attachments {
        attach(row, attachment.path, Some(&attachment.name));
    }
    !row.content.is_empty() || row.attachments.as_ref().is_some_and(|a| !a.is_empty())
}

pub(super) fn tool_content(value: &Value, row: &mut ChatMessageStored) -> Result<(), &'static str> {
    let items = value.as_array().ok_or("invalid tool content collection")?;
    for item in items {
        if !row.content.is_empty() {
            row.content.push('\n');
        }
        match item.get("type").and_then(Value::as_str) {
            Some("content") => block(
                item.get("content").ok_or("missing tool content block")?,
                row,
            )?,
            Some("diff") => {
                row.content.push_str(required_text(item, "path")?);
                row.content.push('\n');
                if let Some(old) = item.get("oldText").and_then(Value::as_str) {
                    row.content.push_str(old);
                    row.content.push('\n');
                }
                row.content.push_str(required_text(item, "newText")?);
            }
            // A terminal id is a reference, not persisted terminal output.
            Some("terminal") => {
                required_text(item, "terminalId")?;
            }
            _ => return Err("unsupported tool content block"),
        }
    }
    Ok(())
}
