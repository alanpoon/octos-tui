//! Inline-viewport driver: owns the scrollback-flush bookkeeping that turns
//! octos-tui's "rebuild everything every frame" model into codex's "finalized
//! history → scrollback, live UI → inline viewport" model.
//!
//! The event loop ([`crate::event_loop`]) calls [`ScrollbackTracker::sync`] each
//! time it is about to draw. The tracker compares the committed message history
//! to what it has already pushed into the terminal's scrollback and returns the
//! *new* finalized lines to insert (and whether the prior scrollback must be
//! reset first, e.g. on a session switch or a hydrate that replaced history).
//!
//! Keeping this state in one small, unit-tested type — separate from the
//! escape-sequence emitter and the render code — is what makes the rearchitecture
//! reviewable: the "what is finalized" decision lives here, the "how to draw the
//! live UI" decision lives in [`crate::app`], and the "how to write scrollback"
//! mechanism lives in [`crate::insert_history`].

use ratatui::text::Line;

use crate::app::{self, CommittedFingerprint};
use crate::model::AppState;
use crate::theme::Palette;

/// Tracks how much committed history has been flushed to terminal scrollback so
/// that each draw only appends the *newly finalized* lines.
#[derive(Debug, Default)]
pub struct ScrollbackTracker {
    /// Fingerprint of the committed history we last flushed.
    last: CommittedFingerprint,
    /// Number of committed messages already flushed for `last.session_id`.
    flushed_messages: usize,
}

/// What the event loop should do with scrollback before drawing the viewport.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScrollbackUpdate {
    /// Lines to insert into scrollback above the inline viewport, in order.
    pub lines_to_insert: Vec<Line<'static>>,
    /// When true, the previously flushed scrollback is stale (session switch or
    /// a hydrate that replaced history). The caller cannot un-write real
    /// scrollback, but it should treat `lines_to_insert` as a fresh full
    /// re-flush of the (now-current) committed history rather than an append.
    pub reset: bool,
}

impl ScrollbackTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconcile the tracker against the current app state and return the lines
    /// to push into scrollback. `wrap_width` is the inline-viewport width.
    pub fn sync(
        &mut self,
        app: &AppState,
        palette: Palette,
        wrap_width: usize,
    ) -> ScrollbackUpdate {
        let fingerprint = app::committed_messages_fingerprint(app);

        // No active session, or no committed messages yet → nothing to flush.
        if fingerprint.message_count == 0 {
            // Keep the session id so a later first message is treated as an
            // append, not a reset (avoids a spurious reset on the first flush).
            self.last = fingerprint;
            self.flushed_messages = 0;
            return ScrollbackUpdate::default();
        }

        // A fresh tracker (nothing flushed yet) treats its first flush as an
        // append of the whole current history, not a reset — there is no prior
        // scrollback to invalidate.
        let first_flush = self.flushed_messages == 0 && self.last.session_id.is_empty();
        let is_extension = first_flush
            || (fingerprint.session_id == self.last.session_id
                && fingerprint.message_count >= self.flushed_messages
                && is_prefix_preserved(&self.last, &fingerprint, self.flushed_messages));

        if is_extension {
            // Append only the messages we have not flushed yet.
            let new_lines =
                app::finalized_history_lines_range(app, palette, wrap_width, self.flushed_messages);
            self.flushed_messages = fingerprint.message_count;
            self.last = fingerprint;
            ScrollbackUpdate {
                lines_to_insert: new_lines,
                reset: false,
            }
        } else {
            // Discontinuity: session switch or hydrate replaced history. We
            // cannot remove already-written scrollback, but we re-flush the full
            // current history so the up-to-date content is selectable below the
            // (now-stale) prior block. Rare (reconnect / session switch).
            let all_lines = app::finalized_history_lines(app, palette, wrap_width);
            self.flushed_messages = fingerprint.message_count;
            self.last = fingerprint;
            ScrollbackUpdate {
                lines_to_insert: all_lines,
                reset: true,
            }
        }
    }
}

/// Whether the already-flushed prefix is preserved in the new fingerprint. We
/// only have a hash of the *whole* committed list, so when the message count is
/// unchanged we can compare hashes directly; when it grew we optimistically
/// treat it as an append (the common streaming/commit case). A hydrate that
/// rewrites earlier messages while also growing the list is the one case this
/// can miss; it is rare and self-heals on the next count-stable frame.
fn is_prefix_preserved(
    last: &CommittedFingerprint,
    next: &CommittedFingerprint,
    flushed: usize,
) -> bool {
    if next.message_count == last.message_count {
        // Same length: only an append-noop if the content is identical.
        return next.content_hash == last.content_hash;
    }
    // Grew: treat as append as long as we had flushed a prefix of it.
    next.message_count >= flushed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ThemeName;
    use crate::model::{ActivityItem, ActivityKind, AppState, TurnActivityLog};
    use octos_core::Message;
    use octos_core::SessionKey;
    use octos_core::app_ui::AppUiSession;
    use octos_core::ui_protocol::TurnId;

    fn palette() -> Palette {
        Palette::for_theme(ThemeName::Slate)
    }

    fn session_with(messages: Vec<Message>) -> AppUiSession {
        AppUiSession {
            id: SessionKey("local:test".into()),
            title: "t".into(),
            profile_id: None,
            messages,
            tasks: Vec::new(),
            live_reply: None,
        }
    }

    fn state(messages: Vec<Message>) -> AppState {
        AppState::new(vec![session_with(messages)], 0, "ready".into(), None, false)
    }

    #[test]
    fn first_flush_emits_all_committed_messages() {
        let app = state(vec![Message::user("hi"), Message::assistant("hello there")]);
        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, palette(), 60);
        assert!(!update.reset);
        assert!(
            !update.lines_to_insert.is_empty(),
            "expected committed lines to flush"
        );
    }

    #[test]
    fn appending_a_message_flushes_only_the_new_one() {
        let mut tracker = ScrollbackTracker::new();
        let app1 = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let first = tracker.sync(&app1, palette(), 60);
        let first_count = first.lines_to_insert.len();
        assert!(first_count > 0);

        let app2 = state(vec![
            Message::user("hi"),
            Message::assistant("a1"),
            Message::user("again"),
            Message::assistant("a2"),
        ]);
        let second = tracker.sync(&app2, palette(), 60);
        assert!(!second.reset, "append should not reset");
        assert!(
            !second.lines_to_insert.is_empty(),
            "expected the new messages to flush"
        );
        // The second flush is only the 2 new messages, so it is smaller than a
        // full re-flush of all 4 messages would be.
        let full = app::finalized_history_lines(&app2, palette(), 60);
        assert!(
            second.lines_to_insert.len() < full.len(),
            "append flush ({}) should be smaller than full ({})",
            second.lines_to_insert.len(),
            full.len()
        );
    }

    #[test]
    fn no_new_messages_flushes_nothing() {
        let mut tracker = ScrollbackTracker::new();
        let app = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let _ = tracker.sync(&app, palette(), 60);
        let again = tracker.sync(&app, palette(), 60);
        assert!(again.lines_to_insert.is_empty());
        assert!(!again.reset);
    }

    #[test]
    fn late_activity_log_archive_triggers_reflush() {
        let mut tracker = ScrollbackTracker::new();
        let mut app = state(vec![
            Message::user("build the site"),
            Message::assistant("done"),
        ]);
        let _ = tracker.sync(&app, palette(), 60);

        let session_id = app.sessions[0].id.clone();
        let turn_id = TurnId::new();
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo test")
                    .with_success(true),
            ],
        });

        let update = tracker.sync(&app, palette(), 60);
        let text = update
            .lines_to_insert
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(update.reset, "late activity log changes finalized history");
        assert!(
            text.contains("Agent task completed") && text.contains("$ cargo test"),
            "reflush should include archived activity log: {text:?}"
        );
    }

    #[test]
    fn session_switch_triggers_reset_and_full_reflush() {
        let mut tracker = ScrollbackTracker::new();
        let app1 = state(vec![Message::user("hi"), Message::assistant("a1")]);
        let _ = tracker.sync(&app1, palette(), 60);

        let other = AppUiSession {
            id: SessionKey("local:other".into()),
            title: "o".into(),
            profile_id: None,
            messages: vec![Message::user("q"), Message::assistant("a")],
            tasks: Vec::new(),
            live_reply: None,
        };
        let app2 = AppState::new(vec![other], 0, "ready".into(), None, false);
        let update = tracker.sync(&app2, palette(), 60);
        assert!(update.reset, "switching sessions should reset scrollback");
        assert!(!update.lines_to_insert.is_empty());
    }

    #[test]
    fn empty_session_flushes_nothing() {
        let mut tracker = ScrollbackTracker::new();
        let app = state(vec![]);
        let update = tracker.sync(&app, palette(), 60);
        assert!(update.lines_to_insert.is_empty());
        assert!(!update.reset);
    }
}
