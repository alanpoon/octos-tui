# Design: Onboarding Should Not Auto-Open More Than Once

**Date:** 2026-06-29  
**Branch:** `should_not_onboard_multiple_times`  
**Status:** Approved

---

## Problem

`maybe_open_onboarding_on_first_launch` is called every time the server sends a capabilities event. Its guard is:

```rust
if !self.state.sessions.is_empty() || self.state.menu_stack.is_active() {
    return;
}
```

On every fresh app launch, `sessions` starts empty (no connection yet), so the wizard auto-opens whenever the server advertises a profile-creation method — even after the user already completed onboarding in a previous run.

## Scope

Fix the **app-relaunch** scenario only: after a user successfully completes onboarding (a session opens), subsequent launches must not auto-open the wizard again.

Out of scope: wizard opened manually via `/wizard` or `/onboard` is unaffected.

---

## Design

### Section 1 — Data model

**`CliFileConfig` (`cli.rs`)**

Add one new optional field that round-trips through the JSON config file:

```rust
#[serde(alias = "onboarding_done")]
pub onboarding_done: Option<bool>,
```

**`Cli` (`cli.rs`)**

Add a plain bool populated in `from_args`:

```rust
pub onboarding_done: bool,
// in from_args:
onboarding_done: file_config.onboarding_done.unwrap_or(false),
```

**`AppState` (`model.rs`)**

Add a runtime flag, initialized to `false` in `AppState::new`:

```rust
pub onboarding_done: bool,
```

---

### Section 2 — Reading the flag at launch

In `event_loop.rs`, alongside the other local-only field seeds:

```rust
store.state.onboarding_done = cli.onboarding_done;
```

In the snapshot-replay block (`apply_event`, Snapshot arm), preserve it like `theme`, `config_path`, etc.:

```rust
let onboarding_done = self.state.onboarding_done;
// ... after state = AppState::from_snapshot(snapshot) ...
state.onboarding_done = onboarding_done;
```

---

### Section 3 — Guard the wizard auto-open

In `maybe_open_onboarding_on_first_launch`:

```rust
fn maybe_open_onboarding_on_first_launch(&mut self) {
    if !self.state.sessions.is_empty()
        || self.state.menu_stack.is_active()
        || self.state.onboarding_done
    {
        return;
    }
    // ... rest unchanged
}
```

In `onboarding_in_progress` (used by the Esc trap):

```rust
fn onboarding_in_progress(&self) -> bool {
    self.state.sessions.is_empty() && !self.state.onboarding_done
}
```

This ensures a manually-opened wizard after prior onboarding is not subject to the Esc lock.

---

### Section 4 — Persisting the flag on completion

**`cli.rs`** — new function using the same merge-and-write pattern as `save_ui_settings`:

```rust
pub fn save_onboarding_done(path: &Path) -> Result<()> {
    // Read existing JSON (or start from empty object if file absent)
    // Insert "onboarding-done": true
    // Create parent dir if needed, write back
}
```

**`store.rs`** — new private method:

```rust
fn persist_onboarding_done(&mut self) {
    self.state.onboarding_done = true;
    let path = self.state.config_path.clone()
        .or_else(crate::cli::default_config_path);
    if let Some(path) = path {
        let _ = crate::cli::save_onboarding_done(&path); // best-effort
    }
}
```

**Call site** — in `UiNotification::SessionOpened`, right after the Issue #4 teardown:

```rust
if self.active_menu_is_onboarding() {
    self.close_all_menus();
    self.state.focus = FocusPane::Composer;
    self.persist_onboarding_done();   // ← new
}
```

---

## Testing

- **Unit test (store.rs):** `second_launch_does_not_auto_open_onboarding_when_done_flag_set` — build a `protocol_store_without_sessions()`, set `state.onboarding_done = true`, send a capabilities event with solo-profile-create support, assert no menu opens.
- **Unit test (cli.rs):** `save_onboarding_done_writes_flag_and_load_reads_it_back` — write to a temp path, reload, assert `onboarding_done == Some(true)` and other existing keys are preserved.
- **Existing tests** must continue to pass (the guard is additive; `onboarding_done` defaults to `false`).

---

## Non-goals

- No CLI flag for `--onboarding-done` (no reason to expose this).
- No status-bar notification when the flag is written (it is a silent background write, same as the flag-write behaviour used elsewhere).
- If the config file is absent and `HOME` is unset, the flag cannot persist. This is the same known limitation as `/saveconfig` and is acceptable.
