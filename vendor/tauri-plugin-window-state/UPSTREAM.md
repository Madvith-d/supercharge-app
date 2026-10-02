# Vendored window-state plugin

Source: the crates.io package `tauri-plugin-window-state` version **2.4.1**, from
<https://github.com/tauri-apps/plugins-workspace>. The original MIT and Apache-2.0
licenses are retained alongside the upstream crate contents.

## Local changes

- An opt-in `Builder::with_background_save` mode captures window state on the
  event-loop thread and submits an owned snapshot to one disk writer.
- The writer coalesces pending snapshots, replaces the state file atomically,
  retries failures with capped exponential backoff, and limits repeated warnings.
- Plugin exit captures the final state and drains the same writer for a bounded
  interval. A filesystem operation already in progress cannot be cancelled.
- Native geometry getters run without holding the cache lock in this mode.
  Background serialization and file I/O receive no window handles or cache guards.
- The synchronous mode, JSON schema, and restore logic remain compatible with
  upstream. `tempfile` supports same-directory atomic replacement.
- An empty workspace declaration isolates this crate from outer workspaces.

The application opts in with a 750 ms shutdown drain. A forced process termination
can lose an uncommitted geometry snapshot; failed replacement preserves the
previous valid file.

## Validation

Run from the application's `src-tauri` directory so the application lockfile and
resolved dependencies are used:

```sh
cargo test -p tauri-plugin-window-state --locked
cargo clippy -p tauri-plugin-window-state --all-targets --locked -- -D warnings
```

The application CI runs these tests separately from application tests, including
Windows test-executable manifest activation. The tests cover ordering, coalescing,
write failures, atomic replacement, bounded shutdown, retry backoff, and upstream
state-file round trips.
