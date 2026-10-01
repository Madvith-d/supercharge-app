use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_app_home(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "grok-agent-home-cfg-{}-{}-{}",
        label,
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn refuse_shared_path_resolve() {
    let err = resolve_writable_config_path("shared").unwrap_err();
    assert!(err.contains("shared"), "{err}");
    assert!(err.contains("refused") || err.contains("~/.grok"), "{err}");
    // Case-insensitive.
    assert!(resolve_writable_config_path("SHARED").is_err());
}

#[test]
fn parse_table_header_trailing_comment() {
    assert_eq!(parse_table_header("[ui]"), Some((false, "ui")));
    assert_eq!(parse_table_header("[ui] # note"), Some((false, "ui")));
    assert_eq!(
        parse_table_header("[compat.claude]"),
        Some((false, "compat.claude"))
    );
    assert_eq!(parse_table_header("[[hooks]]"), Some((true, "hooks")));
    assert_eq!(parse_table_header("[[hooks]] # x"), Some((true, "hooks")));
    assert!(parse_table_header("[ui] junk").is_none());
    assert!(parse_table_header("not a header").is_none());
}

#[test]
fn assignment_key_exact() {
    assert_eq!(assignment_key("yolo = true"), Some("yolo"));
    assert_eq!(assignment_key("yolo_mode = false"), Some("yolo_mode"));
    assert_eq!(assignment_key("# yolo = true"), None);
    assert_eq!(assignment_key("[ui]"), None);
}

#[test]
fn set_and_get_top_level_bool() {
    let t = set_top_level_bool("", "auto_wake_enabled", true);
    assert!(t.contains("auto_wake_enabled = true"));
    assert_eq!(get_top_level_bool(&t, "auto_wake_enabled"), Some(true));

    let existing = "[ui]\nyolo = false\n\n[subagents]\nenabled = true\n";
    let next = set_top_level_bool(existing, "workflows_enabled", false);
    assert_eq!(get_top_level_bool(&next, "workflows_enabled"), Some(false));
    let ui_pos = next.find("[ui]").unwrap();
    let key_pos = next.find("workflows_enabled").unwrap();
    assert!(key_pos < ui_pos);
    assert!(next.contains("yolo = false"));

    let again = set_top_level_bool(&next, "workflows_enabled", true);
    assert_eq!(get_top_level_bool(&again, "workflows_enabled"), Some(true));
    assert_eq!(again.matches("workflows_enabled").count(), 1);
}

#[test]
fn set_table_key_exact_not_prefix() {
    // Historical bug: starts_with("yolo") rewrote yolo_mode and left a
    // second yolo= line → CLI "duplicate key" at spawn.
    let base = "[ui]\npermission_mode = \"always-approve\"\nyolo_mode = false\nyolo = true\n";
    let next = set_table_bool(base, "ui", "yolo", true);
    assert!(next.contains("yolo_mode = false"), "{next}");
    assert_eq!(next.matches("yolo =").count(), 1, "{next}");
    assert!(next.contains("yolo = true"), "{next}");
    assert_eq!(count_duplicate_assignments(&next).0, 0);
}

#[test]
fn set_table_key_matches_header_with_comment() {
    let base = "[ui] # perms\nyolo = false\n";
    let next = set_table_string(base, "ui", "permission_mode", "always-approve");
    // Must update existing [ui] fragment — not append a second [ui].
    assert_eq!(
        next.lines()
            .filter(|l| parse_table_header(l.trim()).is_some_and(|(_, n)| n == "ui"))
            .count(),
        1,
        "{next}"
    );
    assert!(
        next.contains("permission_mode = \"always-approve\""),
        "{next}"
    );
    assert!(next.contains("yolo = false"), "{next}");
    assert_eq!(count_duplicate_assignments(&next).0, 0, "{next}");
}

#[test]
fn ensure_compat_mcp_disabled_sets_both() {
    let t = ensure_compat_mcp_disabled("");
    assert_eq!(get_table_bool(&t, "compat.claude", "mcps"), Some(false));
    assert_eq!(get_table_bool(&t, "compat.cursor", "mcps"), Some(false));
    let again = ensure_compat_mcp_disabled(&t);
    assert_eq!(get_table_bool(&again, "compat.claude", "mcps"), Some(false));
}

#[test]
fn set_and_get_table_bool_and_string() {
    let t = set_table_bool("", "memory", "enabled", true);
    assert!(t.contains("[memory]"));
    assert!(t.contains("enabled = true"));
    assert_eq!(get_table_bool(&t, "memory", "enabled"), Some(true));

    let t2 = set_table_string(&t, "ui", "permission_mode", "acceptEdits");
    assert_eq!(
        get_table_string(&t2, "ui", "permission_mode").as_deref(),
        Some("acceptEdits")
    );
    assert_eq!(get_table_bool(&t2, "memory", "enabled"), Some(true));

    let t3 = set_table_bool(&t2, "memory", "enabled", false);
    assert_eq!(get_table_bool(&t3, "memory", "enabled"), Some(false));
    assert_eq!(t3.matches("enabled =").count(), 1);
}

#[test]
fn top_level_string_roundtrip() {
    let t = set_top_level_string("", "note", "plain");
    assert!(t.contains("note = \"plain\""));
    assert_eq!(get_top_level_string(&t, "note").as_deref(), Some("plain"));
}

#[test]
fn dedupe_keeps_last_and_preserves_unique() {
    let bad = "\
todo_gate_enabled = false
[ui]
permission_mode = \"always-approve\"
yolo = true
yolo = false
[subagents]
enabled = true
";
    assert!(count_duplicate_assignments(bad).0 >= 1);
    let (healed, removed) = dedupe_assignment_keys(bad);
    assert!(removed >= 1);
    assert_eq!(count_duplicate_assignments(&healed).0, 0, "{healed}");
    assert!(healed.contains("yolo = false"), "{healed}");
    assert!(!healed.contains("yolo = true") || healed.matches("yolo =").count() == 1);
    assert!(healed.contains("permission_mode"), "{healed}");
    assert!(healed.contains("[subagents]"), "{healed}");
    // Idempotent on clean text.
    let (again, r2) = dedupe_assignment_keys(&healed);
    assert_eq!(r2, 0);
    assert_eq!(again, healed);
}

#[test]
fn dedupe_does_not_merge_array_table_elements() {
    let text = "\
[[hooks]]
event = \"a\"
command = \"x\"
[[hooks]]
event = \"b\"
command = \"y\"
";
    assert_eq!(count_duplicate_assignments(text).0, 0);
    let (out, removed) = dedupe_assignment_keys(text);
    assert_eq!(removed, 0);
    assert_eq!(out, text);
}

#[test]
fn collapse_second_models_table_keeps_keys_on_models() {
    let bad = "\
[models]
d = false
[model.pollinations]
model = \"openai/x\"
[models]
max_retries = 12
default = \"pollinations\"
";
    assert_eq!(count_duplicate_assignments(bad).0, 0);
    assert_eq!(count_repeated_standard_headers(bad), 1);
    let (healed, removed) = collapse_repeated_standard_tables(bad);
    assert_eq!(removed, 1);
    assert_eq!(count_repeated_standard_headers(&healed), 0, "{healed}");
    let models_at = healed.find("[models]").unwrap();
    let provider_at = healed.find("[model.pollinations]").unwrap();
    let retries_at = healed.find("max_retries = 12").unwrap();
    let default_at = healed.find("default = \"pollinations\"").unwrap();
    assert!(
        models_at < retries_at && retries_at < provider_at,
        "{healed}"
    );
    assert!(
        provider_at < default_at || default_at < provider_at,
        "{healed}"
    );
    assert!(default_at < provider_at, "{healed}");
}

fn dedupe_merges_split_ui_tables() {
    // Two [ui] fragments with same key — TOML treats as one table.
    let bad = "[ui]\nyolo = false\n\n[ui]\nyolo = true\npermission_mode = \"x\"\n";
    assert!(count_duplicate_assignments(bad).0 >= 1);
    let (healed, removed) = dedupe_assignment_keys(bad);
    assert!(removed >= 1);
    assert_eq!(count_duplicate_assignments(&healed).0, 0, "{healed}");
    assert!(healed.contains("yolo = true"), "{healed}");
    assert!(healed.contains("permission_mode"), "{healed}");
}

#[test]
fn ensure_sane_noops_on_valid_and_heals_dups() {
    let _guard = crate::paths::APP_HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = temp_app_home("heal");
    std::env::set_var("GROK_APP_HOME", &home);
    let agent_home = home.join("agent-home");
    fs::create_dir_all(&agent_home).unwrap();
    let cfg = agent_home.join("config.toml");

    // Valid → no write.
    fs::write(&cfg, "[ui]\nyolo = true\n").unwrap();
    let before = fs::read_to_string(&cfg).unwrap();
    let r = ensure_agent_home_config_sane("independent").unwrap();
    assert!(!r.changed);
    assert_eq!(r.note, "ok");
    assert_eq!(fs::read_to_string(&cfg).unwrap(), before);

    // Dup → heal + backup.
    fs::write(
        &cfg,
        "[ui]\nyolo = true\nyolo = false\npermission_mode = \"always-approve\"\n",
    )
    .unwrap();
    let r2 = ensure_agent_home_config_sane("independent").unwrap();
    assert!(r2.changed);
    assert!(r2.removed_duplicates >= 1);
    assert!(r2.backup_path.is_some());
    let after = fs::read_to_string(&cfg).unwrap();
    assert_eq!(count_duplicate_assignments(&after).0, 0, "{after}");
    assert!(after.contains("yolo = false"), "{after}");

    // Shared → skip.
    let r3 = ensure_agent_home_config_sane("shared").unwrap();
    assert!(!r3.changed);
    assert_eq!(r3.note, "shared_mode");

    std::env::remove_var("GROK_APP_HOME");
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn shared_soft_skip_and_independent_write() {
    let _guard = crate::paths::APP_HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let home = temp_app_home("write");
    std::env::set_var("GROK_APP_HOME", &home);

    assert_eq!(
        update_config_toml_if_independent("shared", |t| set_top_level_bool(
            t,
            "auto_wake_enabled",
            true
        ))
        .unwrap(),
        None
    );
    // Strict refuse.
    assert!(update_config_toml("shared", |t| t.to_string()).is_err());

    let path = update_config_toml_if_independent("independent", |t| {
        set_top_level_bool(t, "auto_wake_enabled", true)
    })
    .unwrap()
    .expect("path");
    assert!(path.ends_with("config.toml"));
    let disk = fs::read_to_string(&path).unwrap();
    assert_eq!(get_top_level_bool(&disk, "auto_wake_enabled"), Some(true));

    std::env::remove_var("GROK_APP_HOME");
    let _ = fs::remove_dir_all(&home);
}
