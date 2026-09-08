# VS Code extension for octoscode — design

Date: 2026-09-08
Status: approved (design), not yet implemented

## Goal

Ship an installable VS Code extension, `octos-org.octoscode`, that launches the
existing `octoscode` TUI inside VS Code's integrated terminal. Publish it to the
VS Code Marketplace and to Open VSX (so Cursor / Windsurf / VSCodium users can
install it too).

Non-goal: a native VS Code client. The TUI is the UI. The extension is a host,
not a rewrite.

## Scope boundary

New TypeScript package at `editors/vscode/`. **No Rust changes.** The CLI already
exposes everything the extension needs: `--cwd DIR`, `--mode`, `--profile-id`,
`--no-splash` (see `src/cli.rs`).

The extension does exactly three things:

1. resolve the `octoscode` binary,
2. launch it in the integrated terminal with the right working directory,
3. guide the user to an install command when the binary is missing.

## User-visible surface

**Terminal profile.** `contributes.terminalProfiles` declares a profile titled
"Octoscode"; `window.registerTerminalProfileProvider` implements it. The profile
appears in the terminal panel's `+` dropdown, which is how VS Code natively
expresses "this terminal is a specific tool". Discoverability is the reason to
prefer this over a command-only extension — the command path is a strict subset
of the provider path, so the extra cost is a few dozen lines.

**Commands.**

| Command id | Title | Behaviour |
|---|---|---|
| `octoscode.start` | Octoscode: Start | Launch the TUI in the resolved cwd |
| `octoscode.startMock` | Octoscode: Start (Mock Demo) | Same, plus `--mode mock` |
| `octoscode.doctor` | Octoscode: Doctor | Run `octoscode doctor` in a terminal |
| `octoscode.installHelp` | Octoscode: Install Help | Show the install guidance |

**Keybinding.** `Ctrl+Alt+O` / `Cmd+Alt+O` bound to `octoscode.start`. Low
conflict surface, not zero; it is a default, and users can rebind.

**Settings.**

| Setting | Type | Default | Purpose |
|---|---|---|---|
| `octoscode.path` | string | `""` | Explicit binary path; escape hatch when detection fails |
| `octoscode.args` | string[] | `[]` | Extra args appended to every launch |
| `octoscode.cwd` | enum: `workspaceRoot` \| `activeFileDir` \| `none` | `workspaceRoot` | What `--cwd` gets |
| `octoscode.showMissingBinaryPrompt` | boolean | `true` | Suppress the install notification |

**Explicitly out of scope for the MVP:** status bar item, sidebar view, any
webview, sending editor selection/diagnostics to the TUI, version-compatibility
checks between extension and CLI.

## Binary resolution

This is the only part with real difficulty. In a GUI-launched VS Code, the
extension host's `PATH` frequently omits the npm global bin directory and
Homebrew's prefix — the dominant failure mode for a PATH-only strategy. So
resolution walks an ordered ladder, logging each step to an `Octoscode` output
channel for diagnosis:

1. the `octoscode.path` setting
2. the extension host's `PATH`
3. a login-shell probe: `$SHELL -lic 'command -v octoscode'` (Windows: `where
   octoscode`), 2-second timeout, result cached for the session
4. known install directories: `/opt/homebrew/bin`, `/usr/local/bin`,
   `~/.local/bin`, `~/.cargo/bin`, the npm global prefix
5. all failed → the missing-binary flow below

Once an absolute path is known, launch via
`window.createTerminal({ shellPath, shellArgs })` rather than spawning a shell
and `sendText`-ing a command line. The TUI is then the terminal's own process:
quitting the TUI closes the terminal, and no shell prompt is left behind.

`package.json` sets `"extensionKind": ["workspace"]` so that under Remote SSH,
WSL, and Dev Containers the extension resolves the **remote** binary, not the
local one.

## Error handling

- **Binary missing** → notification with four actions: *npm 安装* (opens a
  terminal running `npm i -g @octos-org/octoscode`), *复制 Homebrew 命令*, *打开
  安装文档*, *指定路径…* (opens the `octoscode.path` setting). Suppressible via
  `octoscode.showMissingBinaryPrompt`.
- **No folder open** → omit `--cwd` and launch anyway.
- **Multi-root workspace** → prefer the root owning the active editor; otherwise
  the first root.
- **Non-zero exit** → message with a "查看输出" action pointing at the output
  channel.

## Release pipeline

### Tag-pattern collision (must be fixed as part of this work)

`.github/workflows/release.yml` triggers on tags matching
`'**[0-9]+.[0-9]+.[0-9]+*'`. A `vscode-v0.1.0` tag **matches that pattern** and
would start a cargo-dist run that then fails, because `vscode` is not a Cargo
package in the workspace. The fix is one line: change that workflow's tag list to

```yaml
tags:
  - '**[0-9]+.[0-9]+.[0-9]+*'
  - '!vscode-v*'
```

documented in-file per the existing hand-edit comment convention. `release.yml`
is already deliberately hand-edited and `dist-workspace.toml` sets
`allow-dirty = ["ci"]`, so this does not fight `dist plan`.

`publish-homebrew.yml` triggers on `'v[0-9]+.[0-9]+.[0-9]+*'`, which
`vscode-v0.1.0` does not match — no change needed there.

### New workflows

- **`.github/workflows/vscode-release.yml`** — triggered by `vscode-v*` tags and
  `workflow_dispatch`. Steps: checkout → setup-node 20 → `npm ci` → test →
  `vsce package` → upload the `.vsix` as a workflow artifact → `vsce publish`
  (`VSCE_PAT`) → `ovsx publish` (`OVSX_PAT`). A tag carrying an `-rc.N` suffix
  passes `--pre-release` to both publishers.
- **`.github/workflows/vscode-ci.yml`** — `paths: ['editors/vscode/**', …]`
  filtered; runs lint, tests, and `vsce package`. Kept separate from `ci.yml` so
  Rust-only PRs do not pay for a Node job.

### Identity and versioning

Publisher is hard-coded to `octos-org`; extension id `octos-org.octoscode`;
extension version starts at `0.1.0` and is **decoupled** from the CLI version.
Registering the publisher on Azure DevOps and open-vsx.org, and populating
`VSCE_PAT` / `OVSX_PAT`, is a human follow-up. Until then the publish steps fail
for want of secrets while `vsce package` still produces a `.vsix` that can be
downloaded from the workflow artifacts and installed by hand.

## Testing

- **Unit** (vitest, with `fs`/`exec` injected as fakes): the four-tier binary
  resolution ladder; argument assembly across the three `cwd` modes, appended
  `octoscode.args`, and mock mode; the per-platform install-command mapping.
- **Integration smoke** (`@vscode/test-electron`): activate the extension and
  assert that all four commands and the terminal profile provider registered.
  One case; it guards against the "packaged extension does not load" class of
  failure.

## Documentation

- `editors/vscode/README.md` doubles as the Marketplace listing body.
- The root `README.md` "📦 Install" section gains a Marketplace block styled to
  match the existing npm / Homebrew blocks.

## Repository hygiene

`.gitignore` gains `editors/vscode/node_modules/`, `editors/vscode/dist/`, and
`*.vsix`.
