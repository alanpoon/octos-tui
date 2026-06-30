# Query History Navigation Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Up/Down key navigation through previously-sent messages in the composer, like bash readline history.

**Architecture:** Two fields on `AppState` (`query_history`, `history_index`) drive the feature. Recording happens in `Store::compose_command()`; navigation happens in the `KeyCode::Up/Down` handler for `FocusPane::Composer`; every composer-mutating keystroke (including Vim-mode edits) resets `history_index` to `None`.

**Tech Stack:** Rust, ratatui, crossterm — no new dependencies.

---

## Chunk 1: State fields + history navigation methods

### Task 1: Add `query_history` and `history_index` to `AppState`

**Files:**
- Modify: `src/model.rs:3243` (struct field declarations)
- Modify: `src/model.rs:4854` (`new_with_panes` initializer)

---

- [ ] **Step 1: Write the failing test**

  Add inside the existing `#[cfg(test)] mod tests` block in `src/model.rs`:

  ```rust
  #[test]
  fn appstate_initial_query_history_is_empty() {
      let state = AppState::new(vec![], 0, "ready".into(), None, false);
      assert!(state.query_history.is_empty());
      assert!(state.history_index.is_none());
  }
  ```

- [ ] **Step 2: Run test to confirm it fails**

  ```bash
  cargo test -p octos-tui appstate_initial_query_history_is_empty 2>&1 | tail -5
  ```

  Expected: compile error — field `query_history` does not exist.

- [ ] **Step 3: Add the fields to the `AppState` struct**

  In `src/model.rs`, after line 3243 (`pub composer_drafts: Vec<ComposerDraft>,`), add:

  ```rust
  pub query_history: Vec<String>,
  pub history_index: Option<usize>,
  ```

- [ ] **Step 4: Initialize the fields in `new_with_panes`**

  In `src/model.rs`, after line 4856 (`composer_drafts: Vec::new(),`), add:

  ```rust
  query_history: Vec::new(),
  history_index: None,
  ```

- [ ] **Step 5: Run test to confirm it passes**

  ```bash
  cargo test -p octos-tui appstate_initial_query_history_is_empty 2>&1 | tail -5
  ```

  Expected: `test ... ok`

- [ ] **Step 6: Run full test suite to check no regressions**

  ```bash
  cargo test -p octos-tui 2>&1 | tail -10
  ```

  Expected: all existing tests pass (Rust's exhaustive struct init will produce compile errors for any missed init sites).

- [ ] **Step 7: Commit**

  ```bash
  git add src/model.rs
  git commit -m "feat(model): add query_history and history_index fields to AppState"
  ```

---

### Task 2: Add `history_navigate_up`, `history_navigate_down`, and `clear_history_index` methods to `AppState`

**Files:**
- Modify: `src/model.rs` — add three methods near the other composer helpers (around line 5920)

---

- [ ] **Step 1: Write the failing tests**

  Add inside `#[cfg(test)] mod tests` in `src/model.rs`:

  ```rust
  fn state_with_history(entries: &[&str]) -> AppState {
      let mut state = AppState::new(vec![], 0, "ready".into(), None, false);
      for e in entries {
          state.query_history.push(e.to_string());
      }
      state
  }

  #[test]
  fn history_navigate_up_enters_most_recent_entry() {
      let mut state = state_with_history(&["first", "second"]);
      state.history_navigate_up();
      assert_eq!(state.history_index, Some(1));
      assert_eq!(state.composer, "second");
  }

  #[test]
  fn history_navigate_up_stops_at_oldest_entry() {
      let mut state = state_with_history(&["only"]);
      state.history_navigate_up();
      state.history_navigate_up(); // second press — should not go below 0
      assert_eq!(state.history_index, Some(0));
      assert_eq!(state.composer, "only");
  }

  #[test]
  fn history_navigate_up_does_nothing_when_history_empty() {
      let mut state = AppState::new(vec![], 0, "ready".into(), None, false);
      state.history_navigate_up();
      assert!(state.history_index.is_none());
      assert!(state.composer.is_empty());
  }

  #[test]
  fn history_navigate_down_moves_toward_newer_entry() {
      let mut state = state_with_history(&["first", "second", "third"]);
      state.history_navigate_up(); // index = Some(2) → "third"
      state.history_navigate_up(); // index = Some(1) → "second"
      state.history_navigate_down(); // index = Some(2) → "third"
      assert_eq!(state.history_index, Some(2));
      assert_eq!(state.composer, "third");
  }

  #[test]
  fn history_navigate_down_from_newest_clears_composer() {
      let mut state = state_with_history(&["first", "second"]);
      state.history_navigate_up(); // index = Some(1) → "second"
      state.history_navigate_down(); // past newest → None
      assert!(state.history_index.is_none());
      assert!(state.composer.is_empty());
  }

  #[test]
  fn history_navigate_down_does_nothing_when_not_in_history_mode() {
      let mut state = state_with_history(&["first"]);
      state.set_composer_text("typing");
      state.history_navigate_down(); // history_index is None — no-op
      assert!(state.history_index.is_none());
      assert_eq!(state.composer, "typing");
  }

  #[test]
  fn clear_history_index_resets_to_none() {
      let mut state = state_with_history(&["entry"]);
      state.history_navigate_up();
      assert!(state.history_index.is_some());
      state.clear_history_index();
      assert!(state.history_index.is_none());
      // composer text is preserved — clear_history_index does not clear composer
      assert_eq!(state.composer, "entry");
  }
  ```

- [ ] **Step 2: Run tests to confirm they fail**

  ```bash
  cargo test -p octos-tui history_navigate 2>&1 | tail -5
  cargo test -p octos-tui clear_history_index 2>&1 | tail -5
  ```

  Expected: compile errors — methods don't exist.

- [ ] **Step 3: Implement the three methods in `src/model.rs`**

  Add after `set_composer_text` (around line 5920), alongside the other composer helpers:

  ```rust
  pub fn history_navigate_up(&mut self) {
      if self.query_history.is_empty() {
          return;
      }
      let next = match self.history_index {
          None => self.query_history.len() - 1,
          Some(0) => 0, // already at oldest — stay
          Some(i) => i - 1,
      };
      self.history_index = Some(next);
      let text = self.query_history[next].clone();
      self.set_composer_text(text);
  }

  pub fn history_navigate_down(&mut self) {
      let Some(i) = self.history_index else { return };
      if i + 1 >= self.query_history.len() {
          // past newest → back to new-input slot
          self.history_index = None;
          self.set_composer_text("");
      } else {
          self.history_index = Some(i + 1);
          let text = self.query_history[i + 1].clone();
          self.set_composer_text(text);
      }
  }

  pub fn clear_history_index(&mut self) {
      self.history_index = None;
  }
  ```

- [ ] **Step 4: Run tests to confirm they pass**

  ```bash
  cargo test -p octos-tui history_navigate 2>&1 | tail -10
  cargo test -p octos-tui clear_history_index 2>&1 | tail -5
  ```

  Expected: all `ok`.

- [ ] **Step 5: Run full suite**

  ```bash
  cargo test -p octos-tui 2>&1 | tail -10
  ```

  Expected: all passing.

- [ ] **Step 6: Commit**

  ```bash
  git add src/model.rs
  git commit -m "feat(model): add history_navigate_up/down and clear_history_index to AppState"
  ```

---

## Chunk 2: Recording history in `store.rs`

### Task 3: Push to `query_history` in `compose_command`

**Files:**
- Modify: `src/store.rs:241–291` (`compose_command`)

---

- [ ] **Step 1: Write the failing tests**

  Add inside `#[cfg(test)] mod tests` in `src/store.rs`. `store_with_empty_session` creates a store with one idle session (no active turn, no live reply). Use it for immediate-send and slash/bang tests.

  ```rust
  #[test]
  fn compose_command_records_plain_prompt_in_history() {
      let mut store = store_with_empty_session();
      store.state.composer = "tell me a joke".into();
      let _ = store.compose_command();
      assert_eq!(store.state.query_history, vec!["tell me a joke"]);
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn compose_command_does_not_record_slash_command() {
      let mut store = store_with_empty_session();
      store.state.composer = "/help".into();
      let _ = store.compose_command();
      assert!(store.state.query_history.is_empty());
  }

  #[test]
  fn compose_command_does_not_record_bang_command() {
      let mut store = store_with_empty_session();
      store.state.composer = "!ls".into();
      let _ = store.compose_command();
      assert!(store.state.query_history.is_empty());
  }

  #[test]
  fn compose_command_resets_history_index_after_slash_when_index_was_some() {
      let mut store = store_with_empty_session();
      store.state.query_history.push("prior".into());
      store.state.history_index = Some(0);
      store.state.composer = "/help".into();
      let _ = store.compose_command();
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn compose_command_resets_history_index_after_bang_when_index_was_some() {
      let mut store = store_with_empty_session();
      store.state.query_history.push("prior".into());
      store.state.history_index = Some(0);
      store.state.composer = "!ls".into();
      let _ = store.compose_command();
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn compose_command_records_staged_prompt_when_turn_active() {
      // Exercises the staged-message path (active turn → pending_messages).
      let session = octos_core::SessionView {
          id: octos_core::SessionKey("local:test".into()),
          title: "test".into(),
          profile_id: Some("coding".into()),
          messages: vec![],
          tasks: vec![],
          live_reply: Some(octos_core::LiveReply {
              turn_id: octos_core::TurnId::new(),
              text: "streaming…".into(),
          }),
      };
      let mut store = Store {
          state: AppState::new(vec![session], 0, "ready".into(), None, false),
      };
      store.state.composer = "follow up question".into();
      let _ = store.compose_command();
      assert_eq!(store.state.query_history, vec!["follow up question"]);
  }
  ```

  > **Note:** The no-active-session path (path 3: `active_session().is_none()` + `onboarding_finish_command()` succeeds) is not unit-tested here. It requires constructing a complete onboarding state (provider config, session key, capabilities event), which is complex enough to defer — the code change is a one-liner in the same pattern as the other two paths, and manual smoke-testing covers it.

- [ ] **Step 2: Run tests to confirm they fail**

  ```bash
  cargo test -p octos-tui compose_command_records 2>&1 | tail -10
  cargo test -p octos-tui compose_command_does_not 2>&1 | tail -10
  cargo test -p octos-tui compose_command_resets 2>&1 | tail -5
  ```

  Expected: FAIL — `query_history` does not get populated yet.

- [ ] **Step 3: Implement the changes in `src/store.rs:compose_command`**

  At the **very top** of `compose_command`, right after the `let prompt = ...` line (line 242) and before the slash check (line 243), add:

  ```rust
  // Reset history index unconditionally on any submission so a stale
  // index is never left behind regardless of the dispatch path taken.
  self.state.history_index = None;
  ```

  In the **no-active-session path** (lines 264–273), add the push inside the `if command.is_some()` block, before `clear_current_composer_draft()`:

  ```rust
  if command.is_some() {
      self.state.query_history.push(prompt.clone());
      self.state.clear_current_composer_draft();
      self.state.pending_messages.push(prompt);
      return command;
  }
  ```

  In the **staged-message path** (lines 282–287), `clear_current_composer_draft()` is called *before* the `if active_turn` guard — do not move it. Add the push *inside* the `if` block:

  ```rust
  self.state.clear_current_composer_draft();  // ← stays here, outside the if
  if self.state.active_turn().is_some() {
      self.state.query_history.push(prompt.clone());
      self.state.pending_messages.push(prompt);
      self.state.status = t!("status.message_staged").into_owned();
      self.state.scroll_transcript_to_latest();
      return None;
  }
  ```

  In the **immediate-send path** (line 290), add the push immediately before `start_prompt_turn`:

  ```rust
  self.state.query_history.push(prompt.clone());
  self.start_prompt_turn(prompt, t!("status.queued_turn_start").into_owned())
  ```

- [ ] **Step 4: Run tests to confirm they pass**

  ```bash
  cargo test -p octos-tui compose_command 2>&1 | tail -15
  ```

  Expected: all `ok`.

- [ ] **Step 5: Run full suite**

  ```bash
  cargo test -p octos-tui 2>&1 | tail -10
  ```

  Expected: all passing.

- [ ] **Step 6: Commit**

  ```bash
  git add src/store.rs
  git commit -m "feat(store): record sent prompts in query_history"
  ```

---

## Chunk 3: Up/Down navigation + edit resets

### Task 4: Wire Up/Down history navigation in the composer

**Files:**
- Modify: `src/event_loop.rs:794–803` (Up/Down handler for `FocusPane::Composer`)

---

- [ ] **Step 1: Write the failing tests**

  Add inside `#[cfg(test)] mod tests` in `src/event_loop.rs`:

  ```rust
  #[test]
  fn up_on_empty_composer_enters_history() {
      let mut store = store_with_sessions(1);
      store.state.query_history.push("first query".into());
      store.state.focus = FocusPane::Composer;
      handle_key(&mut store, key(KeyCode::Up));
      assert_eq!(store.state.composer, "first query");
      assert_eq!(store.state.history_index, Some(0));
  }

  #[test]
  fn up_on_nonempty_composer_does_not_enter_history() {
      let mut store = store_with_sessions(1);
      store.state.query_history.push("first query".into());
      store.state.focus = FocusPane::Composer;
      store.state.composer = "typing".into();
      handle_key(&mut store, key(KeyCode::Up));
      assert_eq!(store.state.composer, "typing");
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn up_does_nothing_when_history_empty_and_composer_empty() {
      let mut store = store_with_sessions(1);
      store.state.focus = FocusPane::Composer;
      handle_key(&mut store, key(KeyCode::Up));
      assert!(store.state.composer.is_empty());
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn down_exits_history_and_clears_composer() {
      let mut store = store_with_sessions(1);
      store.state.query_history.push("q1".into());
      store.state.query_history.push("q2".into());
      store.state.focus = FocusPane::Composer;
      handle_key(&mut store, key(KeyCode::Up)); // → "q2", index=1
      handle_key(&mut store, key(KeyCode::Up)); // → "q1", index=0
      handle_key(&mut store, key(KeyCode::Down)); // → "q2", index=1
      assert_eq!(store.state.composer, "q2");
      assert_eq!(store.state.history_index, Some(1));
      handle_key(&mut store, key(KeyCode::Down)); // → new-input slot
      assert!(store.state.composer.is_empty());
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn down_with_no_history_index_does_not_panic() {
      // When history_index is None and composer is empty, Down falls through
      // to the existing transcript-scroll behavior (must not panic).
      let mut store = store_with_sessions(1);
      store.state.focus = FocusPane::Composer;
      handle_key(&mut store, key(KeyCode::Down));
  }
  ```

- [ ] **Step 2: Run tests to confirm they fail**

  ```bash
  cargo test -p octos-tui "up_on_empty_composer\|down_exits_history\|up_does_nothing_when_history\|up_on_nonempty\|down_with_no_history" 2>&1 | tail -15
  ```

  Expected: FAIL — navigation not yet wired.

- [ ] **Step 3: Replace the Up/Down composer branches in `src/event_loop.rs`**

  At lines 794–803, replace:

  ```rust
  KeyCode::Down if store.state.focus == FocusPane::Composer => {
      if !store.state.move_composer_cursor_down() {
          store.state.scroll_transcript_down(1);
      }
  }
  KeyCode::Up if store.state.focus == FocusPane::Composer => {
      if !store.state.move_composer_cursor_up() {
          store.state.scroll_transcript_up(1);
      }
  }
  ```

  With:

  ```rust
  KeyCode::Down if store.state.focus == FocusPane::Composer => {
      if store.state.composer.trim().is_empty() {
          store.state.history_navigate_down();
      } else if !store.state.move_composer_cursor_down() {
          store.state.scroll_transcript_down(1);
      }
  }
  KeyCode::Up if store.state.focus == FocusPane::Composer => {
      if store.state.composer.trim().is_empty() {
          store.state.history_navigate_up();
      } else if !store.state.move_composer_cursor_up() {
          store.state.scroll_transcript_up(1);
      }
  }
  ```

- [ ] **Step 4: Run tests to confirm they pass**

  ```bash
  cargo test -p octos-tui "up_on_empty_composer\|down_exits_history\|up_does_nothing_when_history\|up_on_nonempty\|down_with_no_history" 2>&1 | tail -15
  ```

  Expected: all `ok`.

- [ ] **Step 5: Run full suite**

  ```bash
  cargo test -p octos-tui 2>&1 | tail -10
  ```

- [ ] **Step 6: Commit**

  ```bash
  git add src/event_loop.rs
  git commit -m "feat(tui): wire Up/Down history navigation in the composer"
  ```

---

### Task 5: Reset `history_index` on any composer edit

**Files:**
- Modify: `src/event_loop.rs` — `handle_plain_key`, `handle_paste`, `handle_composer_modified_key`, `handle_composer_vim_key`, and the Ctrl+U global handler

---

- [ ] **Step 1: Write the failing tests**

  Add inside `#[cfg(test)] mod tests` in `src/event_loop.rs`:

  ```rust
  fn store_in_history_mode() -> Store {
      let mut store = store_with_sessions(1);
      store.state.query_history.push("recalled".into());
      store.state.focus = FocusPane::Composer;
      handle_key(&mut store, key(KeyCode::Up)); // enters history, composer = "recalled"
      assert_eq!(store.state.history_index, Some(0));
      store
  }

  #[test]
  fn typing_char_resets_history_index() {
      let mut store = store_in_history_mode();
      handle_key(&mut store, key(KeyCode::Char('x')));
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn backspace_resets_history_index() {
      let mut store = store_in_history_mode();
      handle_key(&mut store, key(KeyCode::Backspace));
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn delete_key_resets_history_index() {
      let mut store = store_in_history_mode();
      handle_key(&mut store, key(KeyCode::Delete));
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn paste_resets_history_index() {
      let mut store = store_in_history_mode();
      // handle_paste is private but accessible via use super::* in this module
      handle_paste(&mut store, "pasted text");
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn ctrl_u_resets_history_index() {
      let mut store = store_in_history_mode();
      handle_key(
          &mut store,
          modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL),
      );
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn shift_enter_resets_history_index() {
      let mut store = store_in_history_mode();
      handle_key(
          &mut store,
          modified_key(KeyCode::Enter, KeyModifiers::SHIFT),
      );
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn ctrl_w_resets_history_index() {
      let mut store = store_in_history_mode();
      handle_key(
          &mut store,
          modified_key(KeyCode::Char('w'), KeyModifiers::CONTROL),
      );
      assert!(store.state.history_index.is_none());
  }

  #[test]
  fn vim_x_resets_history_index() {
      let mut store = store_in_history_mode();
      store.state.vim_mode = true;
      store.state.composer_mode = crate::model::ComposerMode::Normal;
      handle_key(&mut store, key(KeyCode::Char('x')));
      assert!(store.state.history_index.is_none());
  }
  ```

- [ ] **Step 2: Run tests to confirm they fail**

  ```bash
  cargo test -p octos-tui "resets_history_index" 2>&1 | tail -20
  ```

  Expected: FAIL — resets not yet in place.

- [ ] **Step 3: Add `clear_history_index()` call to `handle_plain_key`**

  In `src/event_loop.rs`, in `handle_plain_key`, update the three mutating branches:

  Line 845 (`KeyCode::Delete`):
  ```rust
  KeyCode::Delete if store.state.focus == FocusPane::Composer => {
      store.state.clear_history_index();
      store.state.delete_composer_next_char();
  }
  ```

  Line 848 (`KeyCode::Backspace`):
  ```rust
  KeyCode::Backspace if store.state.focus == FocusPane::Composer => {
      store.state.clear_history_index();
      store.state.delete_composer_prev_char();
  }
  ```

  Line 880 (`KeyCode::Char(ch)`):
  ```rust
  KeyCode::Char(ch) => {
      let opens_slash_popup = ch == '/' && store.state.composer.is_empty();
      store.state.clear_history_index();
      store.state.insert_composer_char(ch);
      store.state.focus = FocusPane::Composer;
      if opens_slash_popup {
          store.open_menu(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));
      }
  }
  ```

- [ ] **Step 4: Add `clear_history_index()` call to `handle_paste`**

  In `src/event_loop.rs` line 693, before `insert_composer_text`:

  ```rust
  store.state.clear_history_index();
  store.state.insert_composer_text(text);
  ```

- [ ] **Step 5: Add `clear_history_index()` to `handle_composer_modified_key`**

  Add `store.state.clear_history_index();` before each content-mutating call. Do NOT add it to cursor-movement-only branches (Ctrl+a, Ctrl+e, Ctrl+b, Ctrl+f, Alt+b, Alt+f).

  Mutating sites to patch:
  - Shift+Enter (line ~956): before `store.state.insert_composer_text("\n")`
  - Alt+Enter (line ~967): before `store.state.insert_composer_text("\n")`
  - Alt+d (line ~979): before `store.state.delete_composer_next_word()`
  - Alt+Backspace (line ~983): before `store.state.delete_composer_prev_word()`
  - Ctrl+j (line ~997): before `store.state.insert_composer_text("\n")`
  - Ctrl+w (line ~1017): before `store.state.delete_composer_prev_word()`
  - Ctrl+d / Delete (line ~1021): before `store.state.delete_composer_next_char()`
  - Ctrl+h / Backspace (line ~1025): before `store.state.delete_composer_prev_char()`
  - Ctrl+k (line ~1029): before `store.state.kill_composer_to_line_end()`

- [ ] **Step 6: Add `clear_history_index()` to `handle_composer_vim_key`**

  In `src/event_loop.rs`, in `handle_composer_vim_key` (starting at line 1043), add `store.state.clear_history_index();` before each content-mutating call. Only the mutation branches need it — motions (`h`, `j`, `k`, `l`, `w`, `b`, `e`, `G`, `0`, `$`) and mode-switches without mutation (`i`, `a`, `A`, `I`) do NOT.

  Mutating sites to patch:
  - Two-key sequences resolved (lines ~1080–1090):
    - `('d', 'd')`: before `store.state.delete_composer_line()`
    - `('d', 'w')`: before `store.state.delete_composer_word_forward()`
    - `('c', 'c')`: before `store.state.clear_composer_line()`
  - Single-key edits in the `match c` block (lines ~1093–1137):
    - `'x'`: before `store.state.delete_composer_next_char()`
    - `'o'`: before `store.state.open_composer_line_below()`
    - `'O'`: before `store.state.open_composer_line_above()`

- [ ] **Step 7: Add `clear_history_index()` to the Ctrl+U global handler**

  In `src/event_loop.rs` at line 629, update:

  ```rust
  if is_control_char(&key, 'u') {
      store.state.clear_history_index();
      store.clear_composer_or_staged_messages();
      return KeyAction::Continue;
  }
  ```

- [ ] **Step 8: Run all reset tests to confirm they pass**

  ```bash
  cargo test -p octos-tui "resets_history_index" 2>&1 | tail -20
  ```

  Expected: all `ok`.

- [ ] **Step 9: Run full suite**

  ```bash
  cargo test -p octos-tui 2>&1 | tail -10
  ```

  Expected: all passing.

- [ ] **Step 10: Commit**

  ```bash
  git add src/event_loop.rs
  git commit -m "feat(tui): reset history_index on any composer edit keystroke"
  ```

---

## Chunk 4: Final verification

### Task 6: Manual smoke test + help string

- [ ] **Step 1: Build the debug binary**

  ```bash
  cargo build -p octos-tui 2>&1 | tail -5
  ```

  Expected: `Finished` with no errors.

- [ ] **Step 2: Run the app and verify history navigation**

  ```bash
  ./target/debug/octos-tui --mode protocol \
    --stdio-command "octos serve --stdio --solo --data-dir ./octos-data"
  ```

  Manually verify:
  - Send two different messages (e.g., "hello" then "world").
  - Press Up on empty composer → "world" appears.
  - Press Up again → "hello" appears.
  - Press Up again → stays on "hello" (no wraparound).
  - Press Down → "world" appears.
  - Press Down again → composer is empty (back to new-input slot).
  - Type something in the composer, press Up → cursor moves within text (no history navigation).
  - Recall a history entry, edit it, press Up → edit is discarded, previous entry appears.

- [ ] **Step 3: Update the help string in `src/keymap.rs`**

  The current help string (`src/keymap.rs:1`) does not mention history. Update it:

  ```rust
  pub const HELP: &str = "Tab inspector | Esc chat | PgUp/PgDn scroll | y/s/n approval | Alt+A show approval | [/] diff hunk | c stage diff | Enter send | ↑/↓ history (empty composer) | Ctrl+U clear | Ctrl+C interrupt/quit | q quit";
  ```

- [ ] **Step 4: Commit**

  ```bash
  git add src/keymap.rs
  git commit -m "docs(keymap): add history navigation hint to help string"
  ```
