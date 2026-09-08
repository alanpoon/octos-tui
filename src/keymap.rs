//! Key-binding reference for the TUI.

pub const HELP: &str = "Tab agents | Esc chat | PgUp/PgDn scroll | y/s/n approval | Ctrl+R/Alt+A show approval | [/] diff hunk | c stage diff | Enter send | Ctrl+U clear | Ctrl+V paste image | Ctrl+C interrupt | q quit";

/// Ctrl+V, the clipboard-image paste chord (Route-2 image attach).
///
/// Ctrl+V is **shared**, not owned. It already toggles the diff-preview view
/// mode while that overlay is up, and it must still reach the ordinary
/// text-paste path whenever the clipboard holds no bitmap — otherwise this
/// feature would break plain pasting for every user who never pastes an image.
/// The precedence is therefore:
///
/// 1. diff preview active → the existing view-mode toggle wins;
/// 2. otherwise → [`crate::store::Store::paste_clipboard_image`]:
///    - [`ClipboardImagePaste::Staged`]/[`Rejected`] consume the key;
///    - [`ClipboardImagePaste::FallThroughToText`] does not — the keypress
///      continues to the text-paste handler untouched.
///
/// [`ClipboardImagePaste::Staged`]: crate::store::ClipboardImagePaste::Staged
/// [`Rejected`]: crate::store::ClipboardImagePaste::Rejected
/// [`ClipboardImagePaste::FallThroughToText`]: crate::store::ClipboardImagePaste::FallThroughToText
pub const CLIPBOARD_IMAGE_PASTE_CHORD: &str = "Ctrl+V";
