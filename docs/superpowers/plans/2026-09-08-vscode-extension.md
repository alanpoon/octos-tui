# Octoscode VS Code Extension Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `octos-org.octoscode`, a VS Code extension that launches the existing `octoscode` TUI in the integrated terminal, published to the VS Code Marketplace and Open VSX.

**Architecture:** A TypeScript extension at `editors/vscode/` with three pure, dependency-injected modules (binary resolution, launch-argument assembly, install-command mapping) wrapped by a thin `vscode`-facing layer (commands, terminal profile provider, output channel). No Rust changes — the CLI's existing `--cwd` / `--mode` flags are the whole integration surface.

**Tech Stack:** TypeScript 5, esbuild (bundle), vitest (unit), `@vscode/test-cli` + `@vscode/test-electron` (integration), `@vscode/vsce` + `ovsx` (publish), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-08-vscode-extension-design.md`

---

## Conventions for this plan

- All paths are repo-relative. Node commands run from `editors/vscode/` unless stated otherwise.
- Work happens on branch `feat/vscode-extension` (already created; the spec commit `82f99f1` is its tip).
- **UI string language: English.** The spec sketched notification labels in Chinese as shorthand; VS Code extensions are conventionally English, and the repo's default locale is `locales/en.yml`. All user-facing strings in this plan are English. If you want Chinese, every string lives in `src/strings.ts` (Task 6) and swapping them is a one-file change.
- The repo has **no root `LICENSE` file** though `Cargo.toml` declares `Apache-2.0`. `vsce package` warns about this but does not fail. The plan sets `"license": "Apache-2.0"` in `package.json` and leaves adding a root `LICENSE` file out of scope — flag it to the human at the end.

---

## File Structure

| Path | Responsibility |
|---|---|
| `editors/vscode/package.json` | Manifest: id, contributes, settings, scripts, deps |
| `editors/vscode/tsconfig.json` | TS config |
| `editors/vscode/esbuild.mjs` | Bundle `src/extension.ts` → `dist/extension.js` |
| `editors/vscode/vitest.config.ts` | Unit test config (excludes integration) |
| `editors/vscode/.vscode-test.mjs` | `@vscode/test-cli` config for the integration smoke test |
| `editors/vscode/.vscodeignore` | Keep sources/tests out of the `.vsix` |
| `editors/vscode/eslint.config.mjs` | Lint |
| `editors/vscode/README.md` | Marketplace listing body |
| `editors/vscode/icon.png` | 128×128 Marketplace icon |
| `editors/vscode/src/launchArgs.ts` | **Pure.** cwd selection + argv assembly |
| `editors/vscode/src/installCommands.ts` | **Pure.** platform → install actions |
| `editors/vscode/src/resolveBinary.ts` | **Pure (deps injected).** 4-tier resolution ladder |
| `editors/vscode/src/strings.ts` | All user-facing strings |
| `editors/vscode/src/log.ts` | Output-channel wrapper |
| `editors/vscode/src/vscodeDeps.ts` | Builds real `ResolveDeps` from node/fs/child_process |
| `editors/vscode/src/extension.ts` | `activate`/`deactivate`, commands, profile provider, notifications |
| `editors/vscode/test/unit/*.test.ts` | vitest specs for the three pure modules |
| `editors/vscode/test/integration/extension.test.ts` | Activation smoke test |
| `.github/workflows/vscode-ci.yml` | Lint + test + package, path-filtered |
| `.github/workflows/vscode-release.yml` | `vscode-v*` tag → publish to both marketplaces |
| `.github/workflows/release.yml:44-45` | **Modify.** Exclude `vscode-v*` from the cargo-dist tag trigger |
| `.gitignore` | **Modify.** Ignore `node_modules/`, `dist/`, `*.vsix` |
| `README.md` | **Modify.** Marketplace block in the 📦 Install section |

The three pure modules never `import 'vscode'` — that is what makes them unit-testable under plain vitest, and it is the single most important structural rule in this plan.

---

## Chunk 1: Scaffold and tooling

### Task 1: Node package scaffold

**Files:**
- Create: `editors/vscode/package.json`, `editors/vscode/tsconfig.json`, `editors/vscode/eslint.config.mjs`, `editors/vscode/esbuild.mjs`, `editors/vscode/vitest.config.ts`, `editors/vscode/.vscodeignore`
- Modify: `.gitignore`

- [ ] **Step 1: Create `editors/vscode/package.json`**

```json
{
  "name": "octoscode",
  "displayName": "Octoscode",
  "description": "Run the Octos AI coding assistant in the VS Code integrated terminal.",
  "version": "0.1.0",
  "publisher": "octos-org",
  "license": "Apache-2.0",
  "icon": "icon.png",
  "repository": {
    "type": "git",
    "url": "https://github.com/octos-org/octoscode.git",
    "directory": "editors/vscode"
  },
  "homepage": "https://github.com/octos-org/octoscode#readme",
  "bugs": { "url": "https://github.com/octos-org/octoscode/issues" },
  "categories": ["Other"],
  "keywords": ["octos", "ai", "terminal", "coding assistant"],
  "engines": { "vscode": "^1.85.0" },
  "extensionKind": ["workspace"],
  "main": "./dist/extension.js",
  "activationEvents": ["onTerminalProfile:octoscode.terminal-profile"],
  "contributes": {
    "commands": [
      { "command": "octoscode.start", "title": "Start", "category": "Octoscode" },
      { "command": "octoscode.startMock", "title": "Start (Mock Demo)", "category": "Octoscode" },
      { "command": "octoscode.doctor", "title": "Doctor", "category": "Octoscode" },
      { "command": "octoscode.installHelp", "title": "Install Help", "category": "Octoscode" }
    ],
    "keybindings": [
      { "command": "octoscode.start", "key": "ctrl+alt+o", "mac": "cmd+alt+o" }
    ],
    "terminalProfiles": [
      { "id": "octoscode.terminal-profile", "title": "Octoscode", "icon": "sparkle" }
    ],
    "configuration": {
      "title": "Octoscode",
      "properties": {
        "octoscode.path": {
          "type": "string",
          "default": "",
          "scope": "machine-overridable",
          "markdownDescription": "Absolute path to the `octoscode` binary. Leave empty to detect it automatically."
        },
        "octoscode.args": {
          "type": "array",
          "items": { "type": "string" },
          "default": [],
          "description": "Extra arguments appended to every octoscode launch."
        },
        "octoscode.cwd": {
          "type": "string",
          "enum": ["workspaceRoot", "activeFileDir", "none"],
          "enumDescriptions": [
            "Use the workspace folder (the one owning the active editor in a multi-root workspace).",
            "Use the directory of the active editor's file.",
            "Do not pass --cwd; octoscode picks its own default."
          ],
          "default": "workspaceRoot",
          "description": "What octoscode receives as --cwd."
        },
        "octoscode.showMissingBinaryPrompt": {
          "type": "boolean",
          "default": true,
          "description": "Show install guidance when the octoscode binary cannot be found."
        }
      }
    }
  },
  "scripts": {
    "build": "node esbuild.mjs",
    "watch": "node esbuild.mjs --watch",
    "compile": "tsc --noEmit",
    "lint": "eslint src test",
    "test:unit": "vitest run",
    "test:integration": "vscode-test",
    "test": "npm run test:unit && npm run test:integration",
    "package": "npm run build && vsce package --out octoscode.vsix",
    "vscode:prepublish": "npm run build"
  },
  "devDependencies": {
    "@types/mocha": "^10.0.10",
    "@types/node": "^20.19.0",
    "@types/vscode": "^1.85.0",
    "@vscode/test-cli": "^0.0.10",
    "@vscode/test-electron": "^2.4.1",
    "@vscode/vsce": "^3.2.0",
    "esbuild": "^0.24.0",
    "eslint": "^9.17.0",
    "ovsx": "^0.10.1",
    "typescript": "^5.7.0",
    "typescript-eslint": "^8.18.0",
    "vitest": "^2.1.8"
  }
}
```

Two details that are easy to get wrong: `"extensionKind": ["workspace"]` is what makes Remote SSH / WSL resolve the **remote** binary, and `@types/vscode` must not exceed `engines.vscode`.

- [ ] **Step 2: Create `editors/vscode/tsconfig.json`**

```json
{
  "compilerOptions": {
    "module": "Node16",
    "moduleResolution": "Node16",
    "target": "ES2022",
    "lib": ["ES2022"],
    "outDir": "out",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "sourceMap": true,
    "skipLibCheck": true
  },
  "include": ["src", "test"]
}
```

- [ ] **Step 3: Create `editors/vscode/esbuild.mjs`**

```js
import esbuild from 'esbuild';

const watch = process.argv.includes('--watch');

const ctx = await esbuild.context({
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'dist/extension.js',
  format: 'cjs',
  platform: 'node',
  target: 'node20',
  // `vscode` is provided by the host at runtime and must never be bundled.
  external: ['vscode'],
  sourcemap: !watch ? false : 'inline',
  minify: !watch,
  logLevel: 'info',
});

if (watch) {
  await ctx.watch();
} else {
  await ctx.rebuild();
  await ctx.dispose();
}
```

- [ ] **Step 4: Create `editors/vscode/vitest.config.ts`**

```ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['test/unit/**/*.test.ts'],
    environment: 'node',
  },
});
```

The `include` is load-bearing: the integration test imports `vscode` and would blow up under vitest.

- [ ] **Step 5: Create `editors/vscode/eslint.config.mjs`**

```js
import tseslint from 'typescript-eslint';

export default tseslint.config(
  ...tseslint.configs.recommended,
  {
    rules: {
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
    },
  },
  { ignores: ['dist', 'out', 'node_modules'] },
);
```

- [ ] **Step 6: Create `editors/vscode/.vscodeignore`**

```
.vscode-test/**
.vscode-test.mjs
src/**
test/**
out/**
node_modules/**
esbuild.mjs
eslint.config.mjs
tsconfig.json
vitest.config.ts
**/*.map
**/*.ts
!dist/extension.js
```

- [ ] **Step 7: Add ignores to the root `.gitignore`**

Append:

```
# VS Code extension (editors/vscode)
editors/vscode/node_modules/
editors/vscode/dist/
editors/vscode/out/
editors/vscode/.vscode-test/
*.vsix
```

- [ ] **Step 8: Install dependencies and verify the toolchain runs**

```bash
cd editors/vscode && npm install
```

Expected: creates `node_modules/` and `package-lock.json`, exits 0.

```bash
cd editors/vscode && npm run compile
```

Expected: PASS (no `src/` yet, so `tsc --noEmit` succeeds trivially).

- [ ] **Step 9: Commit**

```bash
git add editors/vscode/package.json editors/vscode/package-lock.json \
        editors/vscode/tsconfig.json editors/vscode/esbuild.mjs \
        editors/vscode/vitest.config.ts editors/vscode/eslint.config.mjs \
        editors/vscode/.vscodeignore .gitignore
git commit -m "chore(vscode): scaffold the extension package"
```

---

## Chunk 2: Pure modules (TDD)

Every task in this chunk follows the same rhythm: failing test → run it → minimal implementation → run it → commit.

### Task 2: Launch-argument assembly

**Files:**
- Create: `editors/vscode/src/launchArgs.ts`
- Test: `editors/vscode/test/unit/launchArgs.test.ts`

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, expect, it } from 'vitest';
import { buildLaunchArgs, type LaunchContext } from '../../src/launchArgs.js';

const base: LaunchContext = {
  cwdMode: 'workspaceRoot',
  workspaceFolders: [],
  extraArgs: [],
};

describe('buildLaunchArgs', () => {
  it('passes the only workspace folder as --cwd', () => {
    expect(buildLaunchArgs({ ...base, workspaceFolders: ['/repo'] }))
      .toEqual(['--cwd', '/repo']);
  });

  it('prefers the folder owning the active editor in a multi-root workspace', () => {
    const args = buildLaunchArgs({
      ...base,
      workspaceFolders: ['/first', '/second'],
      activeFolder: '/second',
    });
    expect(args).toEqual(['--cwd', '/second']);
  });

  it('falls back to the first folder when no editor is active', () => {
    expect(buildLaunchArgs({ ...base, workspaceFolders: ['/first', '/second'] }))
      .toEqual(['--cwd', '/first']);
  });

  it('omits --cwd entirely when no folder is open', () => {
    expect(buildLaunchArgs(base)).toEqual([]);
  });

  it('uses the active file directory in activeFileDir mode', () => {
    const args = buildLaunchArgs({
      ...base,
      cwdMode: 'activeFileDir',
      workspaceFolders: ['/repo'],
      activeFileDir: '/repo/src/deep',
    });
    expect(args).toEqual(['--cwd', '/repo/src/deep']);
  });

  it('falls back to the workspace folder when activeFileDir has no active file', () => {
    const args = buildLaunchArgs({
      ...base,
      cwdMode: 'activeFileDir',
      workspaceFolders: ['/repo'],
    });
    expect(args).toEqual(['--cwd', '/repo']);
  });

  it('omits --cwd in none mode even with a folder open', () => {
    expect(buildLaunchArgs({ ...base, cwdMode: 'none', workspaceFolders: ['/repo'] }))
      .toEqual([]);
  });

  it('adds --mode mock in mock mode', () => {
    expect(buildLaunchArgs({ ...base, workspaceFolders: ['/repo'], mock: true }))
      .toEqual(['--cwd', '/repo', '--mode', 'mock']);
  });

  it('appends user args last so they can override earlier flags', () => {
    const args = buildLaunchArgs({
      ...base,
      workspaceFolders: ['/repo'],
      extraArgs: ['--no-splash', '--profile-id', 'work'],
    });
    expect(args).toEqual(['--cwd', '/repo', '--no-splash', '--profile-id', 'work']);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd editors/vscode && npm run test:unit`
Expected: FAIL — `Failed to resolve import "../../src/launchArgs.js"`.

- [ ] **Step 3: Write the implementation**

Create `editors/vscode/src/launchArgs.ts`:

```ts
/** How the extension picks the directory handed to `octoscode --cwd`. */
export type CwdMode = 'workspaceRoot' | 'activeFileDir' | 'none';

export interface LaunchContext {
  cwdMode: CwdMode;
  /** Workspace folder paths in VS Code's order; empty when no folder is open. */
  workspaceFolders: string[];
  /** Directory of the active editor's file, if any. */
  activeFileDir?: string;
  /** Workspace folder owning the active editor, if any. */
  activeFolder?: string;
  /** Value of the `octoscode.args` setting. */
  extraArgs: string[];
  /** Launch the canned demo backend instead of the real one. */
  mock?: boolean;
}

/** The workspace folder to use: the active editor's, else the first one. */
function workspaceFolder(ctx: LaunchContext): string | undefined {
  return ctx.activeFolder ?? ctx.workspaceFolders[0];
}

export function resolveCwd(ctx: LaunchContext): string | undefined {
  switch (ctx.cwdMode) {
    case 'none':
      return undefined;
    case 'activeFileDir':
      return ctx.activeFileDir ?? workspaceFolder(ctx);
    case 'workspaceRoot':
      return workspaceFolder(ctx);
  }
}

export function buildLaunchArgs(ctx: LaunchContext): string[] {
  const args: string[] = [];
  const cwd = resolveCwd(ctx);
  if (cwd !== undefined) {
    args.push('--cwd', cwd);
  }
  if (ctx.mock === true) {
    args.push('--mode', 'mock');
  }
  // User args go last on purpose: a later flag wins in clap, so this is the
  // escape hatch for overriding anything the extension chose above.
  args.push(...ctx.extraArgs);
  return args;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd editors/vscode && npm run test:unit`
Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

```bash
git add editors/vscode/src/launchArgs.ts editors/vscode/test/unit/launchArgs.test.ts
git commit -m "feat(vscode): assemble octoscode launch arguments"
```

### Task 3: Install-command mapping

**Files:**
- Create: `editors/vscode/src/installCommands.ts`
- Test: `editors/vscode/test/unit/installCommands.test.ts`

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, expect, it } from 'vitest';
import { DOCS_URL, installActionsFor } from '../../src/installCommands.js';

describe('installActionsFor', () => {
  it('leads with the npm install command on every platform', () => {
    for (const platform of ['darwin', 'linux', 'win32'] as const) {
      const first = installActionsFor(platform)[0];
      expect(first?.kind).toBe('run');
      expect(first?.command).toBe('npm install -g @octos-org/octoscode');
    }
  });

  it('offers Homebrew on macOS and Linux', () => {
    for (const platform of ['darwin', 'linux'] as const) {
      const brew = installActionsFor(platform).find((a) => a.command?.startsWith('brew'));
      expect(brew?.kind).toBe('copy');
      expect(brew?.command).toContain('brew tap octos-org/octoscode');
      expect(brew?.command).toContain('brew install octos-org/octoscode/octoscode');
    }
  });

  it('does not offer Homebrew on Windows', () => {
    const actions = installActionsFor('win32');
    expect(actions.some((a) => a.command?.startsWith('brew'))).toBe(false);
  });

  it('always ends with the docs link and the manual-path escape hatch', () => {
    const actions = installActionsFor('linux');
    expect(actions.at(-2)).toMatchObject({ kind: 'open', url: DOCS_URL });
    expect(actions.at(-1)).toMatchObject({ kind: 'setting' });
  });

  it('gives every action a non-empty label', () => {
    for (const action of installActionsFor('darwin')) {
      expect(action.label.length).toBeGreaterThan(0);
    }
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd editors/vscode && npm run test:unit`
Expected: FAIL — cannot resolve `../../src/installCommands.js`.

- [ ] **Step 3: Write the implementation**

Create `editors/vscode/src/installCommands.ts`:

```ts
export type InstallPlatform = 'darwin' | 'linux' | 'win32';

/**
 * What the extension should do when the user picks this notification button.
 * - `run`     open a terminal and execute `command`
 * - `copy`    put `command` on the clipboard
 * - `open`    open `url` in a browser
 * - `setting` reveal the `octoscode.path` setting
 */
export type InstallActionKind = 'run' | 'copy' | 'open' | 'setting';

export interface InstallAction {
  label: string;
  kind: InstallActionKind;
  command?: string;
  url?: string;
}

export const DOCS_URL = 'https://github.com/octos-org/octoscode#-install';

const NPM_COMMAND = 'npm install -g @octos-org/octoscode';

// Homebrew needs the tap first: this repo is its own tap.
const BREW_COMMAND =
  'brew tap octos-org/octoscode https://github.com/octos-org/octoscode && ' +
  'brew install octos-org/octoscode/octoscode';

export function installActionsFor(platform: InstallPlatform): InstallAction[] {
  const actions: InstallAction[] = [
    { label: 'Install with npm', kind: 'run', command: NPM_COMMAND },
  ];

  // Homebrew is copy-only rather than run: `brew tap` can prompt, and we do not
  // want to drive an interactive install from a notification button.
  if (platform !== 'win32') {
    actions.push({ label: 'Copy Homebrew command', kind: 'copy', command: BREW_COMMAND });
  }

  actions.push({ label: 'Open install docs', kind: 'open', url: DOCS_URL });
  actions.push({ label: 'Set path\u2026', kind: 'setting' });
  return actions;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd editors/vscode && npm run test:unit`
Expected: PASS, 14 tests total.

- [ ] **Step 5: Commit**

```bash
git add editors/vscode/src/installCommands.ts editors/vscode/test/unit/installCommands.test.ts
git commit -m "feat(vscode): map platform to octoscode install actions"
```

### Task 4: Binary resolution ladder

This is the module that decides whether the extension works at all on a GUI-launched VS Code. Read the spec's "Binary resolution" section before starting.

**Files:**
- Create: `editors/vscode/src/resolveBinary.ts`
- Test: `editors/vscode/test/unit/resolveBinary.test.ts`

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, expect, it, vi } from 'vitest';
import { resolveOctoscode, type ResolveDeps } from '../../src/resolveBinary.js';

/** Build deps where only the listed paths are executable and probes fail. */
function deps(overrides: Partial<ResolveDeps> & { executable?: string[] } = {}): ResolveDeps {
  const executable = new Set(overrides.executable ?? []);
  return {
    platform: 'darwin',
    env: {},
    homedir: () => '/home/dev',
    isExecutable: vi.fn(async (p: string) => executable.has(p)),
    probe: vi.fn(async () => null),
    ...overrides,
  };
}

describe('resolveOctoscode', () => {
  it('uses the configured path and does not probe anything else', async () => {
    const d = deps({ executable: ['/custom/octoscode'], env: { PATH: '/usr/bin' } });
    const out = await resolveOctoscode('/custom/octoscode', d);
    expect(out).toEqual({ kind: 'found', path: '/custom/octoscode', source: 'setting' });
    expect(d.probe).not.toHaveBeenCalled();
  });

  it('reports a bad setting instead of silently falling through', async () => {
    const d = deps({ executable: ['/usr/bin/octoscode'], env: { PATH: '/usr/bin' } });
    expect(await resolveOctoscode('/typo/octoscode', d))
      .toEqual({ kind: 'bad-setting', path: '/typo/octoscode' });
  });

  it('finds the binary on PATH', async () => {
    const d = deps({ executable: ['/usr/local/bin/octoscode'], env: { PATH: '/nope:/usr/local/bin' } });
    expect(await resolveOctoscode('', d))
      .toEqual({ kind: 'found', path: '/usr/local/bin/octoscode', source: 'path' });
  });

  it('falls back to a login-shell probe when PATH misses', async () => {
    const d = deps({
      executable: ['/opt/npm/bin/octoscode'],
      env: { PATH: '/usr/bin', SHELL: '/bin/zsh' },
      probe: vi.fn(async () => '/opt/npm/bin/octoscode\n'),
    });
    const out = await resolveOctoscode('', d);
    expect(out).toEqual({ kind: 'found', path: '/opt/npm/bin/octoscode', source: 'login-shell' });
    expect(d.probe).toHaveBeenCalledWith('/bin/zsh', ['-lic', 'command -v octoscode'], 2000);
  });

  it('ignores a login-shell answer that is not executable', async () => {
    const d = deps({
      executable: ['/opt/homebrew/bin/octoscode'],
      env: { PATH: '/usr/bin' },
      probe: vi.fn(async () => '/stale/octoscode\n'),
    });
    expect(await resolveOctoscode('', d))
      .toEqual({ kind: 'found', path: '/opt/homebrew/bin/octoscode', source: 'known-dir' });
  });

  it('scans known install directories last', async () => {
    const d = deps({ executable: ['/home/dev/.cargo/bin/octoscode'], env: { PATH: '/usr/bin' } });
    expect(await resolveOctoscode('', d))
      .toEqual({ kind: 'found', path: '/home/dev/.cargo/bin/octoscode', source: 'known-dir' });
  });

  it('honours npm_config_prefix as a known directory', async () => {
    const d = deps({
      executable: ['/opt/nvm/bin/octoscode'],
      env: { PATH: '/usr/bin', npm_config_prefix: '/opt/nvm' },
    });
    expect(await resolveOctoscode('', d))
      .toEqual({ kind: 'found', path: '/opt/nvm/bin/octoscode', source: 'known-dir' });
  });

  it('reports not-found when every tier misses', async () => {
    expect(await resolveOctoscode('', deps({ env: { PATH: '/usr/bin' } })))
      .toEqual({ kind: 'not-found' });
  });

  it('uses the .exe name, ; separator and `where` probe on Windows', async () => {
    const d = deps({
      platform: 'win32',
      executable: ['C:\\tools\\octoscode.exe'],
      env: { Path: 'C:\\nope;C:\\tools' },
    });
    expect(await resolveOctoscode('', d))
      .toEqual({ kind: 'found', path: 'C:\\tools\\octoscode.exe', source: 'path' });

    const d2 = deps({
      platform: 'win32',
      executable: ['C:\\npm\\octoscode.exe'],
      env: { Path: 'C:\\nope' },
      probe: vi.fn(async () => 'C:\\npm\\octoscode.exe\r\n'),
    });
    const out = await resolveOctoscode('', d2);
    expect(out).toEqual({ kind: 'found', path: 'C:\\npm\\octoscode.exe', source: 'login-shell' });
    expect(d2.probe).toHaveBeenCalledWith('where', ['octoscode'], 2000);
  });

  it('takes the first line when `where` returns several hits', async () => {
    const d = deps({
      platform: 'win32',
      executable: ['C:\\a\\octoscode.exe', 'C:\\b\\octoscode.exe'],
      env: { Path: 'C:\\nope' },
      probe: vi.fn(async () => 'C:\\a\\octoscode.exe\r\nC:\\b\\octoscode.exe\r\n'),
    });
    expect(await resolveOctoscode('', d))
      .toMatchObject({ path: 'C:\\a\\octoscode.exe' });
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd editors/vscode && npm run test:unit`
Expected: FAIL — cannot resolve `../../src/resolveBinary.js`.

- [ ] **Step 3: Write the implementation**

Create `editors/vscode/src/resolveBinary.ts`:

```ts
export interface ResolveDeps {
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
  homedir(): string;
  /** True when `path` exists and is executable by the current user. */
  isExecutable(path: string): Promise<boolean>;
  /** Run a command, returning stdout, or null on failure/timeout. */
  probe(command: string, args: string[], timeoutMs: number): Promise<string | null>;
}

export type ResolveSource = 'setting' | 'path' | 'login-shell' | 'known-dir';

export type ResolveOutcome =
  | { kind: 'found'; path: string; source: ResolveSource }
  | { kind: 'bad-setting'; path: string }
  | { kind: 'not-found' };

const PROBE_TIMEOUT_MS = 2000;

function exeName(platform: NodeJS.Platform): string {
  return platform === 'win32' ? 'octoscode.exe' : 'octoscode';
}

function join(dir: string, name: string, platform: NodeJS.Platform): string {
  const sep = platform === 'win32' ? '\\' : '/';
  return dir.endsWith(sep) ? `${dir}${name}` : `${dir}${sep}${name}`;
}

function pathEntries(deps: ResolveDeps): string[] {
  // Windows env lookup is case-insensitive in the real process env, but a
  // plain object in tests is not — check both spellings.
  const raw = deps.env.PATH ?? deps.env.Path ?? '';
  const sep = deps.platform === 'win32' ? ';' : ':';
  return raw.split(sep).filter((entry) => entry.length > 0);
}

/** Directories package managers use that a GUI-launched VS Code often misses. */
function knownDirs(deps: ResolveDeps): string[] {
  const home = deps.homedir();
  if (deps.platform === 'win32') {
    const dirs: string[] = [];
    if (deps.env.APPDATA !== undefined) dirs.push(`${deps.env.APPDATA}\\npm`);
    dirs.push(`${home}\\.cargo\\bin`);
    return dirs;
  }
  const dirs = ['/opt/homebrew/bin', '/usr/local/bin', `${home}/.local/bin`, `${home}/.cargo/bin`];
  const prefix = deps.env.npm_config_prefix ?? deps.env.NPM_CONFIG_PREFIX;
  if (prefix !== undefined && prefix.length > 0) {
    dirs.push(`${prefix}/bin`);
  }
  return dirs;
}

async function loginShellCandidate(deps: ResolveDeps): Promise<string | null> {
  const out =
    deps.platform === 'win32'
      ? await deps.probe('where', ['octoscode'], PROBE_TIMEOUT_MS)
      : await deps.probe(
          deps.env.SHELL ?? '/bin/bash',
          ['-lic', 'command -v octoscode'],
          PROBE_TIMEOUT_MS,
        );
  if (out === null) return null;
  const first = out.split(/\r?\n/).map((line) => line.trim()).find((line) => line.length > 0);
  return first ?? null;
}

/**
 * Locate the `octoscode` binary, walking the four tiers in the design spec:
 * setting -> PATH -> login-shell probe -> known install directories.
 */
export async function resolveOctoscode(
  configuredPath: string,
  deps: ResolveDeps,
): Promise<ResolveOutcome> {
  const configured = configuredPath.trim();
  if (configured.length > 0) {
    // An explicit setting is never silently overridden: a typo must surface as
    // a typo, not as "it mysteriously used a different binary".
    return (await deps.isExecutable(configured))
      ? { kind: 'found', path: configured, source: 'setting' }
      : { kind: 'bad-setting', path: configured };
  }

  const name = exeName(deps.platform);

  for (const dir of pathEntries(deps)) {
    const candidate = join(dir, name, deps.platform);
    if (await deps.isExecutable(candidate)) {
      return { kind: 'found', path: candidate, source: 'path' };
    }
  }

  const fromShell = await loginShellCandidate(deps);
  if (fromShell !== null && (await deps.isExecutable(fromShell))) {
    return { kind: 'found', path: fromShell, source: 'login-shell' };
  }

  for (const dir of knownDirs(deps)) {
    const candidate = join(dir, name, deps.platform);
    if (await deps.isExecutable(candidate)) {
      return { kind: 'found', path: candidate, source: 'known-dir' };
    }
  }

  return { kind: 'not-found' };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd editors/vscode && npm run test:unit`
Expected: PASS, 24 tests total.

- [ ] **Step 5: Run lint and the type check**

Run: `cd editors/vscode && npm run lint && npm run compile`
Expected: both exit 0.

- [ ] **Step 6: Commit**

```bash
git add editors/vscode/src/resolveBinary.ts editors/vscode/test/unit/resolveBinary.test.ts
git commit -m "feat(vscode): resolve the octoscode binary across four tiers"
```

---

## Chunk 3: VS Code integration

### Task 5: Real dependency adapters and the output channel

**Files:**
- Create: `editors/vscode/src/vscodeDeps.ts`, `editors/vscode/src/log.ts`

- [ ] **Step 1: Create `editors/vscode/src/log.ts`**

```ts
import * as vscode from 'vscode';

let channel: vscode.OutputChannel | undefined;

export function initLog(context: vscode.ExtensionContext): void {
  channel = vscode.window.createOutputChannel('Octoscode');
  context.subscriptions.push(channel);
}

export function log(message: string): void {
  channel?.appendLine(message);
}

export function showLog(): void {
  channel?.show(true);
}
```

- [ ] **Step 2: Create `editors/vscode/src/vscodeDeps.ts`**

```ts
import { execFile } from 'node:child_process';
import { constants } from 'node:fs';
import { access } from 'node:fs/promises';
import { homedir } from 'node:os';
import type { ResolveDeps } from './resolveBinary.js';
import { log } from './log.js';

export function nodeResolveDeps(): ResolveDeps {
  return {
    platform: process.platform,
    env: process.env,
    homedir,
    async isExecutable(path) {
      try {
        await access(path, constants.X_OK);
        return true;
      } catch {
        return false;
      }
    },
    probe(command, args, timeoutMs) {
      return new Promise((resolve) => {
        // A login shell runs the user's profile, which is exactly the point:
        // it is where nvm / brew / cargo put themselves on PATH. It can also
        // hang, hence the timeout and the swallowed error.
        execFile(command, args, { timeout: timeoutMs, encoding: 'utf8' }, (error, stdout) => {
          if (error) {
            log(`probe failed: ${command} ${args.join(' ')} -> ${error.message}`);
            resolve(null);
            return;
          }
          resolve(stdout);
        });
      });
    },
  };
}
```

- [ ] **Step 3: Verify it type-checks**

Run: `cd editors/vscode && npm run compile && npm run lint`
Expected: both exit 0.

- [ ] **Step 4: Commit**

```bash
git add editors/vscode/src/log.ts editors/vscode/src/vscodeDeps.ts
git commit -m "feat(vscode): add output channel and node resolver adapters"
```

### Task 6: User-facing strings

**Files:**
- Create: `editors/vscode/src/strings.ts`

- [ ] **Step 1: Create `editors/vscode/src/strings.ts`**

```ts
export const strings = {
  missingBinary:
    'Octoscode was not found. Install it, or set "octoscode.path" to the binary.',
  badSetting: (path: string) =>
    `The "octoscode.path" setting points at ${path}, which is not an executable file.`,
  nonZeroExit: (code: number) => `Octoscode exited with code ${code}.`,
  showOutput: 'Show Output',
  dontShowAgain: "Don't Show Again",
  terminalName: 'Octoscode',
  installingTerminalName: 'Install Octoscode',
} as const;
```

Every user-visible string in the extension lives here. Translating the extension means editing this one file.

- [ ] **Step 2: Commit**

```bash
git add editors/vscode/src/strings.ts
git commit -m "feat(vscode): centralize user-facing strings"
```

### Task 7: Extension entry point

**Files:**
- Create: `editors/vscode/src/extension.ts`

- [ ] **Step 1: Create `editors/vscode/src/extension.ts`**

```ts
import * as vscode from 'vscode';
import { buildLaunchArgs, type CwdMode, type LaunchContext } from './launchArgs.js';
import { installActionsFor, type InstallAction, type InstallPlatform } from './installCommands.js';
import { resolveOctoscode, type ResolveOutcome } from './resolveBinary.js';
import { nodeResolveDeps } from './vscodeDeps.js';
import { initLog, log, showLog } from './log.js';
import { strings } from './strings.js';

const PROFILE_ID = 'octoscode.terminal-profile';

/** Resolution is cached per session; a settings change clears it. */
let cached: ResolveOutcome | undefined;

function config(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration('octoscode');
}

async function resolveCached(): Promise<ResolveOutcome> {
  if (cached !== undefined) {
    return cached;
  }
  const outcome = await resolveOctoscode(config().get<string>('path', ''), nodeResolveDeps());
  log(`binary resolution: ${JSON.stringify(outcome)}`);
  cached = outcome;
  return outcome;
}

function launchContext(mock: boolean): LaunchContext {
  const folders = vscode.workspace.workspaceFolders ?? [];
  const active = vscode.window.activeTextEditor?.document.uri;
  const activeFolder =
    active !== undefined ? vscode.workspace.getWorkspaceFolder(active) : undefined;
  const activeFileDir =
    active?.scheme === 'file'
      ? vscode.Uri.joinPath(active, '..').fsPath
      : undefined;

  return {
    cwdMode: config().get<CwdMode>('cwd', 'workspaceRoot'),
    workspaceFolders: folders.map((folder) => folder.uri.fsPath),
    activeFolder: activeFolder?.uri.fsPath,
    activeFileDir,
    extraArgs: config().get<string[]>('args', []),
    mock,
  };
}

function terminalOptions(binary: string, args: string[]): vscode.TerminalOptions {
  return {
    name: strings.terminalName,
    shellPath: binary,
    shellArgs: args,
    iconPath: new vscode.ThemeIcon('sparkle'),
  };
}

async function runAction(action: InstallAction): Promise<void> {
  switch (action.kind) {
    case 'run': {
      const terminal = vscode.window.createTerminal(strings.installingTerminalName);
      terminal.show();
      terminal.sendText(action.command ?? '');
      break;
    }
    case 'copy':
      await vscode.env.clipboard.writeText(action.command ?? '');
      break;
    case 'open':
      await vscode.env.openExternal(vscode.Uri.parse(action.url ?? ''));
      break;
    case 'setting':
      await vscode.commands.executeCommand('workbench.action.openSettings', 'octoscode.path');
      break;
  }
}

async function showInstallHelp(message: string): Promise<void> {
  const actions = installActionsFor(process.platform as InstallPlatform);
  const picked = await vscode.window.showWarningMessage(
    message,
    ...actions.map((action) => action.label),
    strings.dontShowAgain,
  );
  if (picked === undefined) {
    return;
  }
  if (picked === strings.dontShowAgain) {
    await config().update('showMissingBinaryPrompt', false, vscode.ConfigurationTarget.Global);
    return;
  }
  const action = actions.find((candidate) => candidate.label === picked);
  if (action !== undefined) {
    await runAction(action);
  }
}

/** Resolve the binary, surfacing install guidance when that fails. */
async function requireBinary(): Promise<string | undefined> {
  const outcome = await resolveCached();
  if (outcome.kind === 'found') {
    return outcome.path;
  }
  // A bad explicit setting is always worth a notification: the user typed it,
  // so silence would be baffling. A plain miss respects the suppression flag.
  const message =
    outcome.kind === 'bad-setting' ? strings.badSetting(outcome.path) : strings.missingBinary;
  if (outcome.kind === 'bad-setting' || config().get<boolean>('showMissingBinaryPrompt', true)) {
    await showInstallHelp(message);
  }
  return undefined;
}

async function start(mock: boolean): Promise<void> {
  const binary = await requireBinary();
  if (binary === undefined) {
    return;
  }
  const args = buildLaunchArgs(launchContext(mock));
  log(`launching: ${binary} ${args.join(' ')}`);
  vscode.window.createTerminal(terminalOptions(binary, args)).show();
}

async function doctor(): Promise<void> {
  const binary = await requireBinary();
  if (binary === undefined) {
    return;
  }
  vscode.window.createTerminal(terminalOptions(binary, ['doctor'])).show();
}

export function activate(context: vscode.ExtensionContext): void {
  initLog(context);

  context.subscriptions.push(
    vscode.commands.registerCommand('octoscode.start', () => start(false)),
    vscode.commands.registerCommand('octoscode.startMock', () => start(true)),
    vscode.commands.registerCommand('octoscode.doctor', () => doctor()),
    vscode.commands.registerCommand('octoscode.installHelp', () =>
      showInstallHelp(strings.missingBinary),
    ),
  );

  context.subscriptions.push(
    vscode.window.registerTerminalProfileProvider(PROFILE_ID, {
      async provideTerminalProfile() {
        const binary = await requireBinary();
        if (binary === undefined) {
          return undefined;
        }
        const args = buildLaunchArgs(launchContext(false));
        log(`profile launching: ${binary} ${args.join(' ')}`);
        return new vscode.TerminalProfile(terminalOptions(binary, args));
      },
    }),
  );

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration('octoscode.path')) {
        cached = undefined;
      }
    }),
  );

  context.subscriptions.push(
    vscode.window.onDidCloseTerminal(async (terminal) => {
      const code = terminal.exitStatus?.code;
      if (terminal.name !== strings.terminalName || code === undefined || code === 0) {
        return;
      }
      const picked = await vscode.window.showErrorMessage(
        strings.nonZeroExit(code),
        strings.showOutput,
      );
      if (picked === strings.showOutput) {
        showLog();
      }
    }),
  );
}

export function deactivate(): void {
  cached = undefined;
}
```

- [ ] **Step 2: Verify it builds and type-checks**

Run: `cd editors/vscode && npm run compile && npm run lint && npm run build`
Expected: all three exit 0; `dist/extension.js` exists.

- [ ] **Step 3: Commit**

```bash
git add editors/vscode/src/extension.ts
git commit -m "feat(vscode): wire commands, terminal profile and error handling"
```

### Task 8: Activation smoke test

**Files:**
- Create: `editors/vscode/.vscode-test.mjs`, `editors/vscode/test/integration/extension.test.ts`
- Modify: `editors/vscode/package.json` (add a `pretest:integration` script)

- [ ] **Step 1: Create `editors/vscode/.vscode-test.mjs`**

```js
import { defineConfig } from '@vscode/test-cli';

export default defineConfig({
  files: 'out/test/integration/**/*.test.js',
  version: 'stable',
  mocha: { timeout: 60000 },
});
```

- [ ] **Step 2: Add the compile step that produces `out/`**

In `editors/vscode/package.json`, add to `scripts`:

```json
"pretest:integration": "tsc -p tsconfig.json --noEmit false"
```

`@vscode/test-cli` runs plain JS, so the integration test must be compiled to `out/` first. (`npm run compile` stays `--noEmit` for fast type checks.)

- [ ] **Step 3: Write the failing test**

Create `editors/vscode/test/integration/extension.test.ts`:

```ts
import * as assert from 'node:assert';
import * as vscode from 'vscode';

const EXTENSION_ID = 'octos-org.octoscode';

suite('octoscode extension', () => {
  test('activates', async () => {
    const extension = vscode.extensions.getExtension(EXTENSION_ID);
    assert.ok(extension, `extension ${EXTENSION_ID} is not installed`);
    await extension.activate();
    assert.strictEqual(extension.isActive, true);
  });

  test('registers every contributed command', async () => {
    const registered = await vscode.commands.getCommands(true);
    for (const command of [
      'octoscode.start',
      'octoscode.startMock',
      'octoscode.doctor',
      'octoscode.installHelp',
    ]) {
      assert.ok(registered.includes(command), `${command} was not registered`);
    }
  });

  test('contributes the terminal profile', () => {
    const extension = vscode.extensions.getExtension(EXTENSION_ID);
    const profiles = extension?.packageJSON.contributes.terminalProfiles;
    assert.ok(
      profiles.some((p: { id: string }) => p.id === 'octoscode.terminal-profile'),
      'terminal profile is not contributed',
    );
  });
});
```

Note the test asserts the *contribution*, not the provider registration — VS Code exposes no API to introspect registered profile providers, and asserting activation plus contribution covers the failure this test exists to catch.

- [ ] **Step 4: Run the test to verify the harness works**

Run: `cd editors/vscode && npm run build && npm run test:integration`
Expected: PASS, 3 tests. On a headless Linux box prefix with `xvfb-run -a`.

If it fails with "extension is not installed", the usual cause is a `package.json` `name`/`publisher` mismatch against `EXTENSION_ID`.

- [ ] **Step 5: Commit**

```bash
git add editors/vscode/.vscode-test.mjs editors/vscode/test/integration/extension.test.ts editors/vscode/package.json
git commit -m "test(vscode): smoke-test extension activation"
```

### Task 9: Manual verification in a real VS Code

Automated tests cannot confirm that the TUI actually renders. Do this once by hand.

- [ ] **Step 1: Launch the Extension Development Host**

```bash
cd editors/vscode && npm run build
```

Then open `editors/vscode` in VS Code and press `F5`.

- [ ] **Step 2: Verify each surface**

- Command palette → `Octoscode: Start` → the TUI splash renders in a terminal named "Octoscode".
- Terminal panel `+` dropdown → "Octoscode" appears and launches the same thing.
- `Cmd+Alt+O` / `Ctrl+Alt+O` launches it.
- `Octoscode: Start (Mock Demo)` opens the mock backend.
- Set `octoscode.path` to a nonexistent path → the bad-setting notification appears with the install buttons.
- Quit the TUI → the terminal closes with it, leaving no shell prompt.

- [ ] **Step 3: Record the result**

No commit. If anything above fails, fix it before continuing and note what changed.

---

## Chunk 4: Release pipeline and docs

### Task 10: Stop `vscode-v*` tags from triggering the cargo-dist release

**Files:**
- Modify: `.github/workflows/release.yml:41-45`

- [ ] **Step 1: Read the current trigger**

```bash
sed -n '41,46p' .github/workflows/release.yml
```

Expected output:

```yaml
on:
  pull_request:
  push:
    tags:
      - '**[0-9]+.[0-9]+.[0-9]+*'
```

- [ ] **Step 2: Exclude the extension tag namespace**

Replace those lines with:

```yaml
on:
  pull_request:
  push:
    tags:
      - '**[0-9]+.[0-9]+.[0-9]+*'
      # HAND-EDIT (see the dist-workspace.toml note on allow-dirty): the VS Code
      # extension in editors/vscode/ releases on its own `vscode-v*` tags, which
      # match the version glob above. Without this exclusion every extension
      # release starts a dist run that fails, because `vscode` is not a Cargo
      # package in this workspace. Re-apply after any deliberate `dist init`.
      - '!vscode-v*'
```

`publish-homebrew.yml` triggers on `'v[0-9]+.[0-9]+.[0-9]+*'`, which `vscode-v0.1.0` does not match — leave it alone.

- [ ] **Step 3: Verify the YAML still parses**

```bash
ruby -ryaml -e 'p YAML.load_file(".github/workflows/release.yml")[true]["push"]["tags"]'
```

Expected: `["**[0-9]+.[0-9]+.[0-9]+*", "!vscode-v*"]`

(The `[true]` key is not a typo — YAML parses the bare key `on` as a boolean.
Ruby is used rather than Python because PyYAML is not installed on this
machine; `python3 -c "import yaml"` fails.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: keep vscode-v* tags out of the cargo-dist release trigger"
```

### Task 11: Extension CI workflow

**Files:**
- Create: `.github/workflows/vscode-ci.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: VS Code Extension CI

# Path-filtered on purpose: this is a Node job in a Rust repo, and Rust-only
# PRs should not pay for it. Kept separate from ci.yml for the same reason —
# adding a paths filter there would change when the Rust jobs run.
on:
  pull_request:
    paths:
      - 'editors/vscode/**'
      - '.github/workflows/vscode-ci.yml'
  push:
    branches: [main]
    paths:
      - 'editors/vscode/**'
      - '.github/workflows/vscode-ci.yml'

concurrency:
  group: vscode-ci-${{ github.ref }}
  cancel-in-progress: true

defaults:
  run:
    working-directory: editors/vscode

jobs:
  build:
    name: lint + test + package
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v6
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: npm
          cache-dependency-path: editors/vscode/package-lock.json
      - run: npm ci
      - run: npm run lint
      - run: npm run compile
      - run: npm run test:unit
      - run: npm run build
      # The integration test drives a real VS Code, which needs a display.
      - run: xvfb-run -a npm run test:integration
      - run: npx @vscode/vsce package --out octoscode.vsix
      - uses: actions/upload-artifact@v4
        with:
          name: octoscode-vsix
          path: editors/vscode/octoscode.vsix
```

- [ ] **Step 2: Verify the YAML parses**

```bash
ruby -ryaml -e 'YAML.load_file(".github/workflows/vscode-ci.yml"); puts "ok"'
```

Expected: `ok`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/vscode-ci.yml
git commit -m "ci: add VS Code extension lint/test/package workflow"
```

### Task 12: Release workflow

**Files:**
- Create: `.github/workflows/vscode-release.yml`

- [ ] **Step 1: Create the workflow**

```yaml
name: Publish VS Code Extension

# Triggered by `vscode-v<semver>` tags, a namespace deliberately excluded from
# release.yml's cargo-dist trigger. The extension version is decoupled from the
# CLI version: `vscode-v0.1.0` has nothing to do with octoscode 0.3.x.
on:
  push:
    tags:
      - 'vscode-v[0-9]+.[0-9]+.[0-9]+*'
  workflow_dispatch:
    inputs:
      tag:
        description: 'Tag to publish (e.g. vscode-v0.1.0)'
        required: true

defaults:
  run:
    working-directory: editors/vscode

jobs:
  publish:
    name: package + publish
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.inputs.tag || github.ref }}

      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: npm
          cache-dependency-path: editors/vscode/package-lock.json

      - name: Check the tag matches package.json
        run: |
          TAG="${{ github.event.inputs.tag || github.ref_name }}"
          TAG_VERSION="${TAG#vscode-v}"
          PKG_VERSION="$(node -p "require('./package.json').version")"
          if [ "$TAG_VERSION" != "$PKG_VERSION" ]; then
            echo "tag $TAG implies version $TAG_VERSION but package.json says $PKG_VERSION" >&2
            exit 1
          fi
          echo "version=$PKG_VERSION" >> "$GITHUB_OUTPUT"
          # A semver prerelease suffix (-rc.1, -beta.2) publishes to the
          # pre-release channel on both marketplaces, never to stable.
          if [ "${TAG_VERSION#*-}" != "$TAG_VERSION" ]; then
            echo "PRE_RELEASE_FLAG=--pre-release" >> "$GITHUB_ENV"
          else
            echo "PRE_RELEASE_FLAG=" >> "$GITHUB_ENV"
          fi

      - run: npm ci
      - run: npm run lint
      - run: npm run test:unit
      - run: npm run build
      - run: xvfb-run -a npm run test:integration

      - name: Package
        run: npx @vscode/vsce package $PRE_RELEASE_FLAG --out octoscode.vsix

      - uses: actions/upload-artifact@v4
        with:
          name: octoscode-vsix
          path: editors/vscode/octoscode.vsix

      # Both publish steps need secrets that do not exist yet. Until the
      # octos-org publisher is registered on both marketplaces and the secrets
      # are set, these fail and the .vsix artifact above is the deliverable.
      - name: Publish to the VS Code Marketplace
        env:
          VSCE_PAT: ${{ secrets.VSCE_PAT }}
        run: npx @vscode/vsce publish $PRE_RELEASE_FLAG --packagePath octoscode.vsix

      - name: Publish to Open VSX
        env:
          OVSX_PAT: ${{ secrets.OVSX_PAT }}
        run: npx ovsx publish octoscode.vsix $PRE_RELEASE_FLAG -p "$OVSX_PAT"
```

- [ ] **Step 2: Verify the YAML parses**

```bash
ruby -ryaml -e 'YAML.load_file(".github/workflows/vscode-release.yml"); puts "ok"'
```

Expected: `ok`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/vscode-release.yml
git commit -m "ci: publish the VS Code extension to Marketplace and Open VSX"
```

### Task 13: Marketplace icon

**Files:**
- Create: `editors/vscode/icon.png`

- [ ] **Step 1: Produce a 128×128 PNG**

The Marketplace requires an icon; `vsce package` fails when `package.json` names one that does not exist. Generate a simple wordmark matching the repo's OCTOS block-letter branding:

```bash
cd editors/vscode && python3 -c "
from PIL import Image, ImageDraw
img = Image.new('RGBA', (128, 128), (13, 17, 23, 255))
d = ImageDraw.Draw(img)
d.ellipse((24, 24, 104, 104), outline=(126, 231, 199, 255), width=10)
img.save('icon.png')
"
```

If Pillow is unavailable, any 128×128 PNG works for now — flag to the human that the icon is a placeholder awaiting real artwork.

- [ ] **Step 2: Verify dimensions**

```bash
cd editors/vscode && python3 -c "from PIL import Image; print(Image.open('icon.png').size)"
```

Expected: `(128, 128)`

- [ ] **Step 3: Commit**

```bash
git add editors/vscode/icon.png
git commit -m "chore(vscode): add marketplace icon"
```

### Task 14: Documentation

**Files:**
- Create: `editors/vscode/README.md`
- Modify: `README.md` (the 📦 Install section)

- [ ] **Step 1: Create `editors/vscode/README.md`**

This becomes the Marketplace listing body, so write it for someone who has never seen the repo:

```markdown
# Octoscode for VS Code

Run [Octos](https://github.com/octos-org/octos) — an AI coding assistant in the
spirit of Claude Code and Codex — inside the VS Code integrated terminal.

This extension is a launcher. The interface is the `octoscode` terminal app
itself; the extension finds it, starts it in the right directory, and gets out
of the way.

## Requirements

The `octoscode` binary must be installed:

```bash
npm install -g @octos-org/octoscode
```

Other install methods (Homebrew, shell installer, Cargo) are in the
[project README](https://github.com/octos-org/octoscode#-install). The extension
detects the binary automatically; if detection fails, set `octoscode.path`.

## Usage

- **Command palette** → `Octoscode: Start`
- **Terminal panel** → the `+` dropdown → **Octoscode**
- **Keyboard** → `Cmd+Alt+O` (macOS) / `Ctrl+Alt+O`

`Octoscode: Start (Mock Demo)` opens a canned demo connected to nothing.
`Octoscode: Doctor` runs the environment diagnostic.

## Settings

| Setting | Default | Description |
|---|---|---|
| `octoscode.path` | `""` | Absolute path to the binary; empty means auto-detect |
| `octoscode.args` | `[]` | Extra arguments appended to every launch |
| `octoscode.cwd` | `workspaceRoot` | What is passed as `--cwd`: `workspaceRoot`, `activeFileDir`, or `none` |
| `octoscode.showMissingBinaryPrompt` | `true` | Show install guidance when the binary is missing |

## Remote development

The extension runs on the workspace side, so under Remote SSH, WSL, and Dev
Containers it launches the **remote** `octoscode`.

## Troubleshooting

Run `Octoscode: Doctor`, and check the **Octoscode** output channel — it logs
every step of binary detection.
```

- [ ] **Step 2: Add a Marketplace block to the root `README.md`**

In the 📦 Install section, after the PowerShell installer block (around line 111) and before the `octoscode update` paragraph, insert:

```markdown
**🧩 VS Code / Cursor** — the extension launches the TUI in the integrated terminal

```bash
code --install-extension octos-org.octoscode
```

Or search **Octoscode** in the Extensions view. It is also on
[Open VSX](https://open-vsx.org/extension/octos-org/octoscode) for Cursor,
Windsurf, and VSCodium. The extension needs the `octoscode` binary from one of
the methods above — see [`editors/vscode/`](editors/vscode/README.md).
```

- [ ] **Step 3: Commit**

```bash
git add editors/vscode/README.md README.md
git commit -m "docs(vscode): document the extension and its Marketplace install"
```

### Task 15: Full-suite verification

- [ ] **Step 1: Run everything the CI will run**

```bash
cd editors/vscode && npm ci && npm run lint && npm run compile && npm run test:unit && npm run build && npm run test:integration
```

Expected: every step exits 0.

- [ ] **Step 2: Verify the package builds and is not bloated**

```bash
cd editors/vscode && npx @vscode/vsce package --out octoscode.vsix && ls -lh octoscode.vsix
```

Expected: exits 0, `.vsix` well under 1 MB (it contains one bundled JS file, the README, and the icon).

- [ ] **Step 3: Confirm the Rust build is untouched**

```bash
cargo fmt --all --check && cargo check
```

Expected: both exit 0. Nothing in this plan touches Rust, so a failure here means something unrelated is broken.

- [ ] **Step 4: Report to the human**

Summarize and flag explicitly:
1. `VSCE_PAT` and `OVSX_PAT` secrets are not set, and the `octos-org` publisher is not registered on either marketplace — publishing cannot succeed until a human does both.
2. The repo has no root `LICENSE` file despite declaring Apache-2.0; `vsce` warns about this.
3. Whether `icon.png` is real artwork or a placeholder.
4. That the first release requires pushing a `vscode-v0.1.0` tag.

---

## Deferred (explicitly not in this plan)

Per the spec: status bar item, sidebar view, webview UI, sending editor selection or diagnostics to the TUI, and extension↔CLI version-compatibility checks. Do not add them opportunistically.
