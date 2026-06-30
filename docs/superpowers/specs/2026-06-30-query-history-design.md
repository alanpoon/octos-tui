# Query History Navigation Design

**Date:** 2026-06-30
**Status:** Approved

## Overview

Add Up/Down key navigation through previously sent messages in the composer,
matching the bash readline history UX. Navigation activates only when the
composer is empty, so multi-line editing is unaffected.

## State

Two new fields added to `AppState` in `src/model.rs`:

```rust
pub query_history: Vec<String>,   // oldest → newest; in-memory only
pub history_index: Option<usize>, // None = new-input slot; Some(i) = viewing history[i]
```

`history_index` is `None` during normal typing. It becomes `Some(len - 1)` when
the user first presses Up on an empty composer. Down from `Some(0)` resets it to
`None` and clears the composer back to the empty new-input slot.

## Recording History

In `Store::compose_command()` in `src/store.rs`, after the prompt passes all
guards (non-empty, not a slash command, not a bang command, session available),
push before clearing:

```rust
self.state.query_history.push(prompt.clone());
self.state.history_index = None;
```

**What is recorded:** plain conversational prompts only.
**What is not recorded:** slash commands (`/…`), bang commands (`!…`).
**Staged messages** (queued during an active turn) are recorded at the point
they are staged, same as immediately-sent prompts.

## Key Handling

Modification to the `KeyCode::Up` / `KeyCode::Down` branches for
`FocusPane::Composer` in `src/event_loop.rs`.

**Trigger condition:** `store.state.composer.trim().is_empty()`

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
| Composer empty, `history_index` is `Some(0)` | Set `history_index = None`, call `set_composer_text("")` |
| Composer empty, `history_index` is `Some(i)` where `i > 0` | Set `history_index = Some(i - 1)`, call `set_composer_text(history[i-1])` |
| Composer empty, `history_index` is `None` | Existing behavior: `scroll_transcript_down(1)` |
| Composer non-empty | Existing behavior: `move_composer_cursor_down()` → transcript scroll fallback |

### Edit Discard

Any edits made while viewing a history entry are silently discarded on the next
Up/Down — `set_composer_text` overwrites without saving the edit back to the
history slot.

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
| `src/model.rs` | Add `query_history` and `history_index` fields to `AppState`; initialize in `AppState::new` |
| `src/store.rs` | Push to `query_history` in `compose_command()` |
| `src/event_loop.rs` | Extend `KeyCode::Up` / `KeyCode::Down` for `FocusPane::Composer` |
