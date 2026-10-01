![plugin-window-state](https://github.com/tauri-apps/plugins-workspace/raw/v2/plugins/window-state/banner.png)

Save window positions and sizes and restore them when the app is reopened.

## Local fork provenance

Copied from the crates.io `tauri-plugin-window-state` **2.4.1** archive (the app's
locked version). Upstream revision: `e7a68fa63755603b9fa12d28e077eea645551d24`,
`plugins/window-state`; see `.cargo_vcs_info.json`. Original licenses,
`Cargo.toml.orig`, guest bindings, permissions, and other upstream assets are retained.

Local changes: cloneable internal window state; an opt-in
`Builder::with_background_save(shutdown_timeout)` mode; main-thread dispatch for
save/restore callers in that mode; and one ordered, coalescing writer using
same-directory temporary files and atomic replacement (`tempfile`). JSON fields,
restore rules, flags, and default synchronous save behavior are unchanged.

In background mode, explicit Rust/IPC saves return after main-thread capture and
enqueue, **not durability**. Native queries never run on the disk worker, and
capture releases the cache mutex before native reads or submission. Save and
restore calls made off-main wait for main-thread execution without cache locks;
they require a responsive event loop and must not be joined from the UI thread.
Disk failures are logged and retried every 100 ms (or sooner on another save),
with newer pending captures replacing older pending/failed captures. Exit uses
the same writer, stops accepting captures, and waits at most the configured drain
timeout after its final capture. An in-flight OS write cannot be cancelled and
may finish after timeout; a kill/crash can lose queued geometry. Atomic replacement
preserves the previous file on failed writes but does not promise directory-entry
durability across power loss. Default synchronous saves retain upstream direct
writes. Tests live in `src/writer_tests.rs` and must be run explicitly for this
crate (for example, `cargo test --manifest-path src-tauri/Cargo.toml -p
tauri-plugin-window-state --lib` from the application root).

| Platform | Supported |
| -------- | --------- |
| Linux    | ✓         |
| Windows  | ✓         |
| macOS    | ✓         |
| Android  | x         |
| iOS      | x         |

## Install

_This plugin requires a Rust version of at least **1.77.2**_

There are three general methods of installation that we can recommend.

1. Use crates.io and npm (easiest, and requires you to trust that our publishing pipeline worked)
2. Pull sources directly from Github using git tags / revision hashes (most secure)
3. Git submodule install this repo in your tauri project and then use file protocol to ingest the source (most secure, but inconvenient to use)

Install the Core plugin by adding the following to your `Cargo.toml` file:

`src-tauri/Cargo.toml`

```toml
[dependencies]
tauri-plugin-window-state = "2.0.0"
# alternatively with Git:
tauri-plugin-window-state = { git = "https://github.com/tauri-apps/plugins-workspace", branch = "v2" }
```

You can install the JavaScript Guest bindings using your preferred JavaScript package manager:

```sh
pnpm add @tauri-apps/plugin-window-state
# or
npm add @tauri-apps/plugin-window-state
# or
yarn add @tauri-apps/plugin-window-state
```

## Usage

First you need to register the core plugin with Tauri:

`src-tauri/src/lib.rs`

```rust
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Afterwards all windows will remember their state when the app is being closed and will restore to their previous state on the next launch.

Optionally you can also tell the plugin to save the state of all open window to disk by using the `save_window_state()` method exposed by the `AppHandleExt` trait:

```rust
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

// `tauri::AppHandle` now has the following additional method
app.save_window_state(StateFlags::all()); // will save the state of all open windows to disk
```

or through Javascript

```javascript
import { saveWindowState, StateFlags } from '@tauri-apps/plugin-window-state'

saveWindowState(StateFlags.ALL)
```

To manually restore a windows state from disk you can call the `restore_state()` method exposed by the `WindowExt` trait:

```rust
use tauri_plugin_window_state::{WindowExt, StateFlags};

// all `Window` types now have the following additional method
window.restore_state(StateFlags::all()); // will restore the windows state from disk
```

or through Javascript

```javascript
import {
  restoreStateCurrent,
  StateFlags
} from '@tauri-apps/plugin-window-state'

restoreStateCurrent(StateFlags.ALL)
```

## Contributing

PRs accepted. Please make sure to read the Contributing Guide before making a pull request.

## Partners

<table>
  <tbody>
    <tr>
      <td align="center" valign="middle">
        <a href="https://crabnebula.dev" target="_blank">
          <img src="https://github.com/tauri-apps/plugins-workspace/raw/v2/.github/sponsors/crabnebula.svg" alt="CrabNebula" width="283">
        </a>
      </td>
    </tr>
  </tbody>
</table>

For the complete list of sponsors please visit our [website](https://tauri.app#sponsors) and [Open Collective](https://opencollective.com/tauri).

## License

Code: (c) 2015 - Present - The Tauri Programme within The Commons Conservancy.

MIT or MIT/Apache 2.0 where applicable.
