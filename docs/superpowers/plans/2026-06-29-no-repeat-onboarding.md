# No-Repeat Onboarding Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist `"onboarding-done": true` to the config file when the onboarding wizard completes, so subsequent app launches skip the wizard auto-open.

**Architecture:** Four-part change — (1) add `onboarding_done` field to config structs + a `save_onboarding_done` write helper, (2) seed the flag into `AppState` at launch and preserve it across snapshot replays, (3) guard `maybe_open_onboarding_on_first_launch` and the Esc trap against the flag, (4) call `persist_onboarding_done` inside `SessionOpened` when the onboarding wizard tears down.

**Tech Stack:** Rust, serde_json (already in use for config merge-write), existing test helpers in `store.rs` and `cli.rs`.

**Spec:** `docs/superpowers/specs/2026-06-29-no-repeat-onboarding-design.md`

---

## Chunk 1: Config structs + `save_onboarding_done`

### Task 1: Add field to `CliFileConfig`, `Cli`, and `from_args`

**Files:**
- Modify: `src/cli.rs:257-258` (after `vim_mode` field in `CliFileConfig`)
- Modify: `src/cli.rs:141-142` (after `vim_mode` field in `Cli`)
- Modify: `src/cli.rs:354-358` (end of `Ok(Self { ... })` in `from_args`)

- [ ] **Step 1: Write the failing test in `src/cli.rs`**

Add inside the `#[cfg(test)] mod tests` block (after the existing tests, around line 650+):

```rust
#[test]
fn onboarding_done_round_trips_through_config_file() {
    let path = write_config("onboarding-done", r#"{ "onboarding-done": true, "theme": "claude" }"#);
    let cfg = super::load_config_file(&path).expect("valid config loads");
    assert_eq!(cfg.onboarding_done, Some(true), "onboarding-done must be read back");
    assert_eq!(cfg.theme, Some(ThemeName::Claude), "other keys must survive");
    // snake_case alias also accepted
    let path2 = write_config("onboarding-done-snake", r#"{ "onboarding_done": true }"#);
    let cfg2 = super::load_config_file(&path2).expect("snake alias parses");
    assert_eq!(cfg2.onboarding_done, Some(true));
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&path2);
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p octos-tui onboarding_done_round_trips_through_config_file 2>&1 | tail -20
```

Expected: compile error — `CliFileConfig` has no field `onboarding_done`.

- [ ] **Step 3: Add `onboarding_done` to `CliFileConfig` and `Cli`**

In `src/cli.rs`, after the `vim_mode` field in `CliFileConfig` (line 258):

```rust
    #[serde(alias = "onboarding_done")]
    pub onboarding_done: Option<bool>,
```

After the `vim_mode: bool` field in `Cli` (line 142):

```rust
    /// True when onboarding was already completed in a prior launch.
    pub onboarding_done: bool,
```

In `from_args`, at the end of `Ok(Self { ... })`, after `vim_mode: ...` (line ~357):

```rust
            onboarding_done: file_config.onboarding_done.unwrap_or(false),
```

- [ ] **Step 4: Run test to confirm it passes**

```bash
cargo test -p octos-tui onboarding_done_round_trips_through_config_file 2>&1 | tail -10
```

Expected: `test cli::tests::onboarding_done_round_trips_through_config_file ... ok`

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat(cli): add onboarding_done field to CliFileConfig and Cli"
```

---

### Task 2: Add `save_onboarding_done` function + tests

**Files:**
- Modify: `src/cli.rs` — add `pub fn save_onboarding_done` after `save_ui_settings` (around line 482)
- Modify: `src/cli.rs` — add two tests in `mod tests`

- [ ] **Step 1: Write the failing tests**

Add in the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn save_onboarding_done_writes_flag_and_load_reads_it_back() {
    use super::{save_onboarding_done, load_config_file};
    let path = write_config("save-onboarding-done", r#"{ "theme": "claude" }"#);
    save_onboarding_done(&path).expect("save succeeds");
    let cfg = load_config_file(&path).expect("reload succeeds");
    assert_eq!(cfg.onboarding_done, Some(true), "onboarding-done must be persisted");
    assert_eq!(cfg.theme, Some(ThemeName::Claude), "existing keys must survive");
    let _ = fs::remove_file(&path);
}

#[test]
fn save_onboarding_done_creates_file_when_absent() {
    use super::{save_onboarding_done, load_config_file};
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("octos-tui-absent-{nonce}.json"));
    assert!(!path.exists(), "file must not exist yet");
    save_onboarding_done(&path).expect("creates file on first save");
    let cfg = load_config_file(&path).expect("reads back");
    assert_eq!(cfg.onboarding_done, Some(true));
    let _ = fs::remove_file(&path);
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cargo test -p octos-tui save_onboarding_done 2>&1 | tail -10
```

Expected: compile error — `save_onboarding_done` is not defined.

- [ ] **Step 3: Implement `save_onboarding_done`**

Add in `src/cli.rs` between the closing `}` of `save_ui_settings` (line 482) and `pub fn parse_websocket_url` (line 484):

```rust
/// Persist the `onboarding-done` flag into the config file, merging
/// with any existing content exactly as `save_ui_settings` does.
/// A missing file is created; all other existing keys are preserved.
pub fn save_onboarding_done(path: &Path) -> Result<()> {
    let mut root = match fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Ok(contents) => serde_json::from_str::<serde_json::Value>(&contents)
            .wrap_err_with(|| format!("failed to parse TUI config {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Err(error) => {
            return Err(error)
                .wrap_err_with(|| format!("failed to read TUI config {}", path.display()));
        }
    };
    root.as_object_mut()
        .ok_or_else(|| eyre!("TUI config {} is not a JSON object", path.display()))?
        .insert("onboarding-done".into(), true.into());
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("failed to create config dir {}", parent.display()))?;
    }
    let mut serialized =
        serde_json::to_string_pretty(&root).wrap_err("failed to serialize TUI config")?;
    serialized.push('\n');
    fs::write(path, serialized)
        .wrap_err_with(|| format!("failed to write TUI config {}", path.display()))
}
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test -p octos-tui save_onboarding_done 2>&1 | tail -10
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat(cli): add save_onboarding_done config helper"
```

---

## Chunk 2: AppState field + launch seeding + snapshot replay

### Task 3: Add `onboarding_done` to `AppState`

**Files:**
- Modify: `src/model.rs:3273` — add field after `onboarding: OnboardingWizardState`
- Modify: `src/model.rs:4882` — initialize in `AppState::new_with_panes`

- [ ] **Step 1: Add the field**

In `src/model.rs`, after `pub onboarding: OnboardingWizardState,` (line 3273):

```rust
    /// Set when the first-launch onboarding wizard completes successfully.
    /// Persisted to the config file; read back at next launch so the wizard
    /// does not auto-open again.
    pub onboarding_done: bool,
```

In `AppState::new_with_panes` in the `Self { ... }` initializer block (around line 4882, after `onboarding: OnboardingWizardState::default()`):

```rust
            onboarding_done: false,
```

- [ ] **Step 2: Confirm it compiles**

```bash
cargo build -p octos-tui 2>&1 | grep "^error" | head -20
```

Expected: no errors (the field has a `Default`-compatible initializer).

- [ ] **Step 3: Commit**

```bash
git add src/model.rs
git commit -m "feat(model): add onboarding_done field to AppState"
```

---

### Task 4: Seed from CLI and preserve across snapshot replays

**Files:**
- Modify: `src/event_loop.rs:103-106` — add seed line alongside other local-only fields
- Modify: `src/store.rs:4660-4699` — snapshot replay block, save + restore `onboarding_done`

- [ ] **Step 1: Seed in `event_loop.rs`**

In `src/event_loop.rs`, after the line `store.state.vim_mode = cli.vim_mode;` (around line 106):

```rust
    // Seed the onboarding-done flag from the config file so the wizard does
    // not auto-open again after a successful first-launch completion.
    store.state.onboarding_done = cli.onboarding_done;
```

- [ ] **Step 2: Preserve across snapshot replays in `store.rs`**

In the snapshot replay block (`apply_event`, `AppUiEvent::Snapshot` arm), find where other local-only fields are saved (around line 4660). Add alongside `vim_mode`:

```rust
                let onboarding_done = self.state.onboarding_done;
```

Then after `state.vim_mode = vim_mode;` (around line 4696), add:

```rust
                state.onboarding_done = onboarding_done;
```

- [ ] **Step 3: Confirm it compiles and existing tests still pass**

```bash
cargo test -p octos-tui 2>&1 | tail -20
```

Expected: all existing tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/event_loop.rs src/store.rs
git commit -m "feat(store): seed and preserve onboarding_done across snapshot replays"
```

---

## Chunk 3: Guard the wizard auto-open + Esc trap

### Task 5: Guard `maybe_open_onboarding_on_first_launch`

**Files:**
- Modify: `src/store.rs:4928-4949`

- [ ] **Step 1: Write the failing test**

Add in the `#[cfg(test)]` block of `src/store.rs`, near the other `first_launch_*` tests (after line 10248):

```rust
#[test]
fn second_launch_does_not_auto_open_onboarding_when_done_flag_set() {
    let mut store = protocol_store_without_sessions();
    store.state.onboarding_done = true;

    store.apply_client_event(ClientEvent::Capabilities(CapabilitiesClientEvent {
        result: crate::model::ConfigCapabilitiesListResult {
            capabilities: UiProtocolCapabilities::new(
                &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
                &[],
            ),
        },
        message: "capabilities".into(),
    }));

    assert!(
        store.state.active_menu.is_none(),
        "onboarding must not auto-open when onboarding_done is true"
    );
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p octos-tui second_launch_does_not_auto_open_onboarding_when_done_flag_set 2>&1 | tail -10
```

Expected: test fails — the wizard opens even though `onboarding_done` is true.

- [ ] **Step 3: Add the guard in `maybe_open_onboarding_on_first_launch`**

In `src/store.rs`, update the function (around line 4929):

```rust
    fn maybe_open_onboarding_on_first_launch(&mut self) {
        if !self.state.sessions.is_empty()
            || self.state.menu_stack.is_active()
            || self.state.onboarding_done
        {
            return;
        }
        // ... rest unchanged
```

- [ ] **Step 4: Run test to confirm it passes**

```bash
cargo test -p octos-tui second_launch_does_not_auto_open_onboarding_when_done_flag_set 2>&1 | tail -10
```

Expected: ok.

- [ ] **Step 5: Commit**

```bash
git add src/store.rs
git commit -m "feat(store): skip onboarding auto-open when onboarding_done is set"
```

---

### Task 6: Update the Esc trap (`onboarding_in_progress`)

**Files:**
- Modify: `src/store.rs:2760-2762`

- [ ] **Step 1: Write the failing test**

Add in the `#[cfg(test)]` block near the `esc_on_root_onboarding_menu_*` tests (after line 9383):

```rust
#[test]
fn esc_on_root_onboarding_menu_allowed_when_onboarding_already_done() {
    // A user who already completed onboarding can re-open the wizard manually
    // (e.g. via /onboard). In that case onboarding_done is true, so the Esc
    // trap must NOT engage — the user can dismiss the wizard normally.
    let mut store = protocol_store_without_sessions();
    store.state.onboarding_done = true;
    store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
    assert!(store.active_menu_id_is(crate::menu::registry::MENU_ONBOARD));

    let closed = store.handle_menu_escape();

    assert!(closed, "Esc must close the wizard when onboarding_done is true");
    assert!(
        !store.state.menu_stack.is_active(),
        "root wizard must be gone after Esc when onboarding already completed"
    );
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test -p octos-tui esc_on_root_onboarding_menu_allowed_when_onboarding_already_done 2>&1 | tail -10
```

Expected: test fails — Esc is a no-op because `onboarding_in_progress` still returns true.

- [ ] **Step 3: Update `onboarding_in_progress`**

In `src/store.rs`, update (around line 2760):

```rust
    fn onboarding_in_progress(&self) -> bool {
        self.state.sessions.is_empty() && !self.state.onboarding_done
    }
```

- [ ] **Step 4: Run both the new test AND the existing Esc trap tests**

```bash
cargo test -p octos-tui esc_on_root_onboarding_menu 2>&1 | tail -15
```

Expected: all three `esc_on_root_onboarding_menu_*` tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/store.rs
git commit -m "feat(store): relax Esc trap when onboarding already completed"
```

---

## Chunk 4: Persist on completion

### Task 7: Add `persist_onboarding_done` and wire into `SessionOpened`

**Files:**
- Modify: `src/store.rs` — add `fn persist_onboarding_done` (private method on `Store`)
- Modify: `src/store.rs:5684-5687` — call it in the `SessionOpened` teardown block

- [ ] **Step 1: Write a test that `onboarding_done` is set in-memory when wizard completes**

Add near the `session_opened_closes_onboarding_wizard_and_focuses_composer` test (after line 10933):

```rust
#[test]
fn session_opened_sets_onboarding_done_when_wizard_was_active() {
    use octos_core::SessionKey;
    use octos_core::ui_protocol::SessionOpened;

    let mut store = protocol_store_with_methods(&[
        crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        crate::model::APPUI_METHOD_AUTH_STATUS,
    ]);
    store.open_menu(MenuId::from(crate::menu::registry::MENU_ONBOARD));
    assert!(!store.state.onboarding_done, "not done before session opens");

    let opened: SessionOpened = serde_json::from_value(serde_json::json!({
        "session_id": SessionKey("alice:local:tui#coding".into()),
        "active_profile_id": "alice",
    }))
    .expect("payload");
    store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

    assert!(
        store.state.onboarding_done,
        "onboarding_done must be true after the wizard's session opens"
    );
}

#[test]
fn session_opened_does_not_set_onboarding_done_when_no_wizard_was_active() {
    use octos_core::SessionKey;
    use octos_core::ui_protocol::SessionOpened;

    // Non-onboarding session open must not flip the flag.
    let mut store = store_with_empty_session();
    assert!(!store.state.onboarding_done);

    let opened: SessionOpened = serde_json::from_value(serde_json::json!({
        "session_id": SessionKey("alice:local:tui#coding".into()),
        "active_profile_id": "alice",
    }))
    .expect("payload");
    store.apply_event(AppUiEvent::Protocol(UiNotification::SessionOpened(opened)));

    assert!(
        !store.state.onboarding_done,
        "onboarding_done must stay false when no wizard was active"
    );
}
```

- [ ] **Step 2: Run to confirm first test fails, second passes**

```bash
cargo test -p octos-tui session_opened_sets_onboarding_done 2>&1 | tail -10
cargo test -p octos-tui session_opened_does_not_set_onboarding_done 2>&1 | tail -10
```

Expected: first fails, second passes.

- [ ] **Step 3: Add `persist_onboarding_done` and wire the call**

Add `persist_onboarding_done` as a private method on `Store` in `src/store.rs` (near `dispatch_save_config`, around line 1126):

```rust
    fn persist_onboarding_done(&mut self) {
        self.state.onboarding_done = true;
        let path = self
            .state
            .config_path
            .clone()
            .or_else(crate::cli::default_config_path);
        if let Some(path) = path {
            let _ = crate::cli::save_onboarding_done(&path);
        }
    }
```

In the `UiNotification::SessionOpened` handler, update the teardown block (around line 5684):

```rust
                if self.active_menu_is_onboarding() {
                    self.close_all_menus();
                    self.state.focus = FocusPane::Composer;
                    self.persist_onboarding_done();
                }
```

- [ ] **Step 4: Run all new tests and the full suite**

```bash
cargo test -p octos-tui session_opened_sets_onboarding_done 2>&1 | tail -10
cargo test -p octos-tui 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/store.rs
git commit -m "feat(store): persist onboarding_done when first-launch wizard completes"
```

---

## Final: Full test run

- [ ] **Run the complete test suite one last time**

```bash
cargo test -p octos-tui 2>&1 | tail -30
```

Expected: all tests pass, no regressions.

- [ ] **Verify the new tests are present**

```bash
cargo test -p octos-tui 2>&1 | grep -E "onboarding_done|second_launch|esc_on_root.*done|session_opened.*onboarding"
```

Expected: at least 5 new test names printed as `ok`.
