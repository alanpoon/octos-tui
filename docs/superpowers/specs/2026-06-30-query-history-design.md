# Query History Navigation Design

**Date:** 2026-06-30
**Status:** Approved

## Overview

Add Up/Down key navigation through previously sent messages in the composer,
matching the bash readline history UX. Navigation activates only when the
composer text is empty (after trimming whitespace/newlines), so multi-line
editing is unaffected.

## State

Two new fields added to `AppState` in `src/model.rs`:

```rust
pub query_history: Vec<String>,   // oldest → newest; in-memory only
pub history_index: Option<usize>, // None = new-input slot; Some(i) = viewing history[i]
```

`history_index` is `None` during normal typing. It becomes `Some(len - 1)` (the
newest entry) when the user first presses Up on an empty composer. Down from
`Some(len - 1)` resets it to `None` and clears the composer back to the empty
new-input slot.

Both fields are initialized to their zero values (`Vec::new()` / `None`) in
`AppState::new_with_panes`, which is the true construction site called by both
`AppState::new` and `AppState::from_snapshot`.

## Recording History

In `Store::compose_command()` in `src/store.rs`, push and reset `history_index`
**in all three successful plain-prompt paths**, immediately before each path's
cleanup call:

1. **Immediate-send path** (session active, no active turn): push before
   `start_prompt_turn`.
2. **Staged-message path** (session active, active turn in progress): push before
   `pending_messages.push`.
3. **No-active-session path** (onboarding auto-open): push before
   `pending_messages.push` at the early-exit branch that stages the message
   while returning `onboarding_finish_command()`.

In all three cases:

```rust
self.state.query_history.push(prompt.clone());
self.state.history_index = None;
```

**What is recorded:** plain conversational prompts only, provided they are
non-empty after trimming (the `prompt.is_empty()` guard in `compose_command()`
already prevents empty-after-trim strings from reaching any of these paths, so
no additional check is needed).

**What is not recorded:** slash commands (`/…`), bang commands (`!…`).

When a slash or bang command is submitted while `history_index` is `Some(_)`,
reset `history_index = None` at the **top** of `compose_command()` (before the
slash/bang early-return branches), so a stale index is never left behind
regardless of which dispatch path is taken.

## Key Handling

Modification to the `KeyCode::Up` / `KeyCode::Down` branches for
`FocusPane::Composer` in `src/event_loop.rs`.

**Trigger condition:** `store.state.composer.trim().is_empty()`
("Empty" means the composer contains only whitespace or newlines, or nothing at
all — `.trim().is_empty()` is used rather than `.is_empty()`, so
whitespace-only text also activates history navigation.)

### Up

| State | Action |
|-------|--------|
| Composer empty, `history_index` is `None`, history non-empty | Set `history_index = Some(len - 1)`, call `set_composer_text(history[len-1])` |
| Composer empty, `history_index` is `Some(i)` where `i > 0` | Set `history_index = Some(i - 1)`, call `set_composer_text(history[i-1])` |
| Composer empty, already at oldest entry (`i == 0`) | No-op (stay at oldest) |
| Composer empty, history is empty | No-op (nothing to recall, no transcript scroll) |
| Composer non-empty | Existing behavior: `move_composer_cursor_up()` → transcript scroll fallback |

### Down

| State | Action |
|-------|--------|
| Composer empty, `history_index` is `Some(i)` where `i < len - 1` | Set `history_index = Some(i + 1)`, call `set_composer_text(history[i+1])` |
| Composer empty, `history_index` is `Some(len - 1)` (newest entry) | Set `history_index = None`, call `set_composer_text("")` |
| Composer empty, `history_index` is `None` | Existing behavior: `scroll_transcript_down(1)` |
| Composer non-empty | Existing behavior: `move_composer_cursor_down()` → transcript scroll fallback |

### Edit Discard and `history_index` Reset on Edit

Any edits made while viewing a history entry are silently discarded on the next
Up/Down — `set_composer_text` overwrites without saving the edit back to the
history slot.

If the user modifies the composer text while `history_index` is `Some(_)`, reset
`history_index` to `None`. The text in the composer is kept as-is, but it is no
longer tracking a history slot. This reset must be inserted in the following
locations in `src/event_loop.rs`:

- `handle_plain_key`: `KeyCode::Char(_)`, `KeyCode::Backspace`,
  `KeyCode::Delete` branches for `FocusPane::Composer`.
- `handle_paste`: wherever text is inserted into the composer.
- `handle_composer_modified_key`: any branch that calls `delete_composer_*`,
  `insert_composer_*`, or similar mutation methods (Ctrl+W word-delete,
  Ctrl+K kill-line, etc.).
- The **Ctrl+U global handler** in `event_loop.rs` (the `is_control_char(&key,
  'u')` branch that calls `clear_composer_or_staged_messages()`) bypasses
  `handle_composer_modified_key` entirely. Add `history_index = None` reset
  directly in that branch, alongside the `clear_composer_or_staged_messages()`
  call.

## Cursor Placement

`set_composer_text` places the cursor at the end of the recalled text (existing
behavior, no change needed).

## Out of Scope

- Persistence across app restarts (in-memory only)
- Per-session history
- Deduplication of consecutive identical entries
- Maximum history size cap

## Files Changed

| File | Change |
|------|--------|
| `src/model.rs` | Add `query_history` and `history_index` fields to `AppState`; initialize both in `AppState::new_with_panes` |
| `src/store.rs` | Push + reset `history_index` in all three plain-prompt send paths; reset `history_index` unconditionally at top of `compose_command()` before slash/bang dispatch |
| `src/event_loop.rs` | Extend `KeyCode::Up` / `KeyCode::Down` for `FocusPane::Composer`; reset `history_index` in `handle_plain_key`, `handle_paste`, and mutating branches of `handle_composer_modified_key` |
