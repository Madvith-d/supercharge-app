<h1 align="center">Supercharge App</h1>
<p align="center"><strong>building beyond limits</strong></p>
<p align="center"><em>A native desktop workbench for the Supercharge CLI.</em></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey" alt="Platforms" />
  <img src="https://img.shields.io/badge/Tauri-2-orange" alt="Tauri 2" />
</p>

## Overview

Supercharge App is the desktop interface for the local [`supercharge`](https://github.com/iotserver24/supercharge-releases) coding-agent CLI. The desktop shell communicates with `supercharge agent stdio` through the Agent Client Protocol (ACP); model requests, tools, permissions, sessions, skills, and provider behavior remain owned by the CLI.

Credit: original desktop workbench by [RongleCat](https://github.com/RongleCat).

## Features

- Multi-project chat workspace with concurrent sessions and background streaming.
- Structured assistant text, reasoning, tool calls, plans, permissions, and ask-user prompts.
- Live model discovery from the configured Supercharge CLI.
- OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages provider formats.
- Project files, code editor, Git diffs, worktrees, terminal, and rich media/document previews.
- MCP servers, skills, plugins, hooks, workflows, agents, memory, and scheduled tasks.
- Local session API, mobile browser mirror, SSH workflows, and Remote IM integrations.
- Local, Openverse, and Pexels wallpaper sources plus custom OpenAI-compatible dictation.
- macOS, Windows, and Linux desktop packages.

## Requirements

- Supercharge CLI 1.x available as `supercharge` or `supercharge-pager`.
- Node.js 22.22.2 or newer for development.
- pnpm 9.x.
- Rust and the platform dependencies required by Tauri 2.

Install the CLI:

```bash
curl -fsSL https://raw.githubusercontent.com/iotserver24/supercharge-releases/main/scripts/install.sh | bash
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/iotserver24/supercharge-releases/main/scripts/install.ps1 | iex
```

## Development

```bash
pnpm install --frozen-lockfile
pnpm dev
```

Frontend-only preview:

```bash
pnpm dev:ui
```

Validation:

```bash
pnpm typecheck
pnpm lint
pnpm test
pnpm build:ui
cd src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The desktop discovers the CLI through `SUPERCHARGE_BIN`, the app’s configured binary path, colocated binaries, standard install locations, and `PATH`. Shared sessions use `SUPERCHARGE_HOME` or `~/.supercharge`; independent mode uses the app-owned Supercharge home.

## Local data and migration

The application uses its own Supercharge data namespace. On first run it can copy compatible non-secret project, session, layout, automation, extension, attachment, skin, and local wallpaper data from an older desktop home if one is present. Existing destination data wins, source data is not deleted, and credentials are never copied automatically.

## Security

Provider credentials and conversation history are sensitive. Do not commit `secrets.json`, authentication files, app data, or generated session contents. See [SECURITY.md](SECURITY.md).

## Upstream contributors

<!-- CONTRIBUTORS:START -->
Thanks to everyone who has contributed to Grok App. All human GitHub contributors (by commit count, updated 2026-10-01).

<p align="center">
  <a href="https://github.com/RongleCat" title="RongleCat"><img src="https://github.com/RongleCat.png?size=96" width="72" height="72" alt="RongleCat" style="border-radius:50%" /></a>
  <a href="https://github.com/sonnemusk" title="sonnemusk"><img src="https://github.com/sonnemusk.png?size=96" width="72" height="72" alt="sonnemusk" style="border-radius:50%" /></a>
  <a href="https://github.com/zhangxaochen" title="zhangxaochen"><img src="https://github.com/zhangxaochen.png?size=96" width="72" height="72" alt="zhangxaochen" style="border-radius:50%" /></a>
  <a href="https://github.com/shiaho777" title="shiaho777"><img src="https://github.com/shiaho777.png?size=96" width="72" height="72" alt="shiaho777" style="border-radius:50%" /></a>
  <a href="https://github.com/yclenove" title="yclenove"><img src="https://github.com/yclenove.png?size=96" width="72" height="72" alt="yclenove" style="border-radius:50%" /></a>
  <a href="https://github.com/AlexZander85" title="AlexZander85"><img src="https://github.com/AlexZander85.png?size=96" width="72" height="72" alt="AlexZander85" style="border-radius:50%" /></a>
  <a href="https://github.com/Dmao233" title="Dmao233"><img src="https://github.com/Dmao233.png?size=96" width="72" height="72" alt="Dmao233" style="border-radius:50%" /></a>
  <a href="https://github.com/pengqian-lu" title="pengqian-lu"><img src="https://github.com/pengqian-lu.png?size=96" width="72" height="72" alt="pengqian-lu" style="border-radius:50%" /></a>
  <a href="https://github.com/ynjmxn" title="ynjmxn"><img src="https://github.com/ynjmxn.png?size=96" width="72" height="72" alt="ynjmxn" style="border-radius:50%" /></a>
  <a href="https://github.com/Yy-702" title="Yy-702"><img src="https://github.com/Yy-702.png?size=96" width="72" height="72" alt="Yy-702" style="border-radius:50%" /></a>
  <a href="https://github.com/erict16" title="erict16"><img src="https://github.com/erict16.png?size=96" width="72" height="72" alt="erict16" style="border-radius:50%" /></a>
  <a href="https://github.com/enderzcx" title="enderzcx"><img src="https://github.com/enderzcx.png?size=96" width="72" height="72" alt="enderzcx" style="border-radius:50%" /></a>
  <a href="https://github.com/jason920612" title="jason920612"><img src="https://github.com/jason920612.png?size=96" width="72" height="72" alt="jason920612" style="border-radius:50%" /></a>
  <a href="https://github.com/oykb58246" title="oykb58246"><img src="https://github.com/oykb58246.png?size=96" width="72" height="72" alt="oykb58246" style="border-radius:50%" /></a>
  <a href="https://github.com/ChenYCL" title="ChenYCL"><img src="https://github.com/ChenYCL.png?size=96" width="72" height="72" alt="ChenYCL" style="border-radius:50%" /></a>
  <a href="https://github.com/a70win-wq" title="a70win-wq"><img src="https://github.com/a70win-wq.png?size=96" width="72" height="72" alt="a70win-wq" style="border-radius:50%" /></a>
  <a href="https://github.com/1parado" title="1parado"><img src="https://github.com/1parado.png?size=96" width="72" height="72" alt="1parado" style="border-radius:50%" /></a>
  <a href="https://github.com/sutongwuyanzu" title="sutongwuyanzu"><img src="https://github.com/sutongwuyanzu.png?size=96" width="72" height="72" alt="sutongwuyanzu" style="border-radius:50%" /></a>
  <a href="https://github.com/KarnaughK" title="KarnaughK"><img src="https://github.com/KarnaughK.png?size=96" width="72" height="72" alt="KarnaughK" style="border-radius:50%" /></a>
  <a href="https://github.com/Ksndj" title="Ksndj"><img src="https://github.com/Ksndj.png?size=96" width="72" height="72" alt="Ksndj" style="border-radius:50%" /></a>
  <a href="https://github.com/lunar-me" title="lunar-me"><img src="https://github.com/lunar-me.png?size=96" width="72" height="72" alt="lunar-me" style="border-radius:50%" /></a>
  <a href="https://github.com/ericyiu9819" title="ericyiu9819"><img src="https://github.com/ericyiu9819.png?size=96" width="72" height="72" alt="ericyiu9819" style="border-radius:50%" /></a>
  <a href="https://github.com/falser101" title="falser101"><img src="https://github.com/falser101.png?size=96" width="72" height="72" alt="falser101" style="border-radius:50%" /></a>
  <a href="https://github.com/salasebas" title="salasebas"><img src="https://github.com/salasebas.png?size=96" width="72" height="72" alt="salasebas" style="border-radius:50%" /></a>
  <a href="https://github.com/Sdefendre" title="Sdefendre"><img src="https://github.com/Sdefendre.png?size=96" width="72" height="72" alt="Sdefendre" style="border-radius:50%" /></a>
  <a href="https://github.com/Thibaultjaigu" title="Thibaultjaigu"><img src="https://github.com/Thibaultjaigu.png?size=96" width="72" height="72" alt="Thibaultjaigu" style="border-radius:50%" /></a>
  <a href="https://github.com/yuhaouno" title="yuhaouno"><img src="https://github.com/yuhaouno.png?size=96" width="72" height="72" alt="yuhaouno" style="border-radius:50%" /></a>
  <a href="https://github.com/2530185073" title="2530185073"><img src="https://github.com/2530185073.png?size=96" width="72" height="72" alt="2530185073" style="border-radius:50%" /></a>
  <a href="https://github.com/86208620" title="86208620"><img src="https://github.com/86208620.png?size=96" width="72" height="72" alt="86208620" style="border-radius:50%" /></a>
  <a href="https://github.com/apple-ouyang" title="apple-ouyang"><img src="https://github.com/apple-ouyang.png?size=96" width="72" height="72" alt="apple-ouyang" style="border-radius:50%" /></a>
  <a href="https://github.com/fannnzhang" title="fannnzhang"><img src="https://github.com/fannnzhang.png?size=96" width="72" height="72" alt="fannnzhang" style="border-radius:50%" /></a>
  <a href="https://github.com/hermes87666" title="hermes87666"><img src="https://github.com/hermes87666.png?size=96" width="72" height="72" alt="hermes87666" style="border-radius:50%" /></a>
  <a href="https://github.com/jchacker5" title="jchacker5"><img src="https://github.com/jchacker5.png?size=96" width="72" height="72" alt="jchacker5" style="border-radius:50%" /></a>
  <a href="https://github.com/Johnny-dot" title="Johnny-dot"><img src="https://github.com/Johnny-dot.png?size=96" width="72" height="72" alt="Johnny-dot" style="border-radius:50%" /></a>
  <a href="https://github.com/KronixDev" title="KronixDev"><img src="https://github.com/KronixDev.png?size=96" width="72" height="72" alt="KronixDev" style="border-radius:50%" /></a>
  <a href="https://github.com/MaxxxDong" title="MaxxxDong"><img src="https://github.com/MaxxxDong.png?size=96" width="72" height="72" alt="MaxxxDong" style="border-radius:50%" /></a>
  <a href="https://github.com/praxstack" title="praxstack"><img src="https://github.com/praxstack.png?size=96" width="72" height="72" alt="praxstack" style="border-radius:50%" /></a>
  <a href="https://github.com/rkhrkh" title="rkhrkh"><img src="https://github.com/rkhrkh.png?size=96" width="72" height="72" alt="rkhrkh" style="border-radius:50%" /></a>
  <a href="https://github.com/RocStone" title="RocStone"><img src="https://github.com/RocStone.png?size=96" width="72" height="72" alt="RocStone" style="border-radius:50%" /></a>
  <a href="https://github.com/Sixmin" title="Sixmin"><img src="https://github.com/Sixmin.png?size=96" width="72" height="72" alt="Sixmin" style="border-radius:50%" /></a>
  <a href="https://github.com/sk1935" title="sk1935"><img src="https://github.com/sk1935.png?size=96" width="72" height="72" alt="sk1935" style="border-radius:50%" /></a>
  <a href="https://github.com/tisrop" title="tisrop"><img src="https://github.com/tisrop.png?size=96" width="72" height="72" alt="tisrop" style="border-radius:50%" /></a>
  <a href="https://github.com/XancelZC" title="XancelZC"><img src="https://github.com/XancelZC.png?size=96" width="72" height="72" alt="XancelZC" style="border-radius:50%" /></a>
  <a href="https://github.com/Yux-c" title="Yux-c"><img src="https://github.com/Yux-c.png?size=96" width="72" height="72" alt="Yux-c" style="border-radius:50%" /></a>
</p>

[Full contributors graph →](https://github.com/RongleCat/grok-app/graphs/contributors)
<!-- CONTRIBUTORS:END -->

## License

MIT. See [LICENSE](LICENSE). Credit: original workbench by [RongleCat](https://github.com/RongleCat).
