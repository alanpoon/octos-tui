//! Clipboard copy support for the TUI.
//!
//! The TUI runs in the alternate screen with mouse capture enabled, so the
//! terminal's native click-drag selection is intercepted by the app and the
//! user cannot select-to-copy. This module provides an in-app copy path that
//! is **terminal-agnostic and SSH-safe**: it writes the system clipboard via
//! the [OSC 52] terminal escape sequence. OSC 52 travels in-band over the same
//! PTY/SSH channel the TUI already uses, so a copy on a remote host (the fleet
//! minis) lands in the *operator's local* clipboard — something a clipboard
//! crate that talks to the remote host's clipboard could never do.
//!
//! [OSC 52]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands
//!
//! Two pure, unit-tested pieces live here so the behaviour can be verified
//! without a real terminal:
//!  - [`osc52_copy_sequence`]: builds the escape sequence (base64 + framing).
//!  - [`copyable_assistant_text`]: decides *what* gets copied (the last
//!    assistant reply — the answer / research report / code block the user
//!    most often wants out of the TUI).
//!
//! # Copy IN: clipboard bitmaps (Route-2 image attach)
//!
//! The paste direction is the mirror problem. Bracketed paste is **text-only**
//! — a terminal never hands the app the BYTES of a screenshot on the clipboard
//! — so "screenshot → Ctrl+V → ask about it" cannot work through the PTY. The
//! bytes have to be fetched out of band, from the OS clipboard, by an external
//! helper. [`ClipboardImageSource`] is that seam: [`SystemClipboardImageSource`]
//! shells out to `osascript` (macOS), `wl-paste` (Wayland) or `xclip` (X11),
//! and every decision built on top of it (staging path, size validation,
//! fallback) is a pure function driven by the trait, so the whole feature is
//! unit-testable with a fake and no real clipboard.

use crate::model::AppState;
use std::path::{Path, PathBuf};

/// Maximum size, in bytes, of the base64 payload inside the OSC 52 sequence.
///
/// Common terminals silently drop OSC 52 sequences past an internal limit —
/// xterm historically caps the whole sequence near 100 KB, and tmux's
/// passthrough adds framing on top — so a multi-hundred-KB copy would no-op
/// with no feedback. 72 KB of encoded payload (~54 KB of text) stays well
/// under every known cap while still fitting any realistic answer.
pub const OSC52_MAX_ENCODED_BYTES: usize = 72 * 1024;

/// Maximum input bytes so `base64(input)` never exceeds
/// [`OSC52_MAX_ENCODED_BYTES`]: base64 emits 4 output bytes per 3 input bytes.
const OSC52_MAX_INPUT_BYTES: usize = OSC52_MAX_ENCODED_BYTES / 4 * 3;

/// Build the OSC 52 escape sequence that sets the system clipboard to `text`.
///
/// Shape: `ESC ] 52 ; c ; <base64(text)> BEL`. The `c` selection targets the
/// clipboard (as opposed to the primary/selection buffer). Terminals that
/// honour OSC 52 (iTerm2, kitty, WezTerm, foot, recent xterm, tmux with
/// `set-clipboard on`, and SSH sessions through any of them) decode the
/// base64 payload and place it on the local clipboard.
///
/// The payload is standard base64 (RFC 4648, `+`/`/`, `=` padding) with **no**
/// line wrapping — line breaks in an OSC string would terminate the sequence.
/// Oversized input is truncated (head kept) to [`OSC52_MAX_ENCODED_BYTES`];
/// use [`osc52_copy_sequence_capped`] to learn whether truncation happened.
pub fn osc52_copy_sequence(text: &str) -> String {
    osc52_copy_sequence_for(text, std::env::var_os("TMUX").is_some())
}

/// Build the OSC 52 sequence, optionally wrapped for tmux passthrough.
///
/// Inside tmux, a bare OSC 52 sequence is consumed by tmux itself and never
/// reaches the outer terminal (so the operator's *local* clipboard is never
/// set). tmux's DCS passthrough — `ESC P tmux; <escaped-payload> ESC \` with the
/// inner `ESC` bytes doubled — forwards the sequence to the outer terminal.
/// codex uses the same wrapper (`clipboard_copy.rs`). The `set-clipboard on`
/// tmux option must also be enabled for this to work end to end.
///
/// Detection is by the `TMUX` env var (set by tmux for its child processes);
/// `tmux` is the parameterized seam so the behaviour is unit-testable.
pub fn osc52_copy_sequence_for(text: &str, tmux: bool) -> String {
    osc52_copy_sequence_capped(text, tmux).0
}

/// [`osc52_copy_sequence_for`] plus a truncation signal.
///
/// Returns `(sequence, truncated)`: when `text` encodes past
/// [`OSC52_MAX_ENCODED_BYTES`] the input is cut at the largest char boundary
/// that fits (keeping the head — the start of an answer is the part the user
/// asked for) and `truncated` is `true`, so callers can surface a "copied
/// first N KB" hint instead of a silent whole-copy no-op in the terminal.
pub fn osc52_copy_sequence_capped(text: &str, tmux: bool) -> (String, bool) {
    let (text, truncated) = truncate_at_char_boundary(text, OSC52_MAX_INPUT_BYTES);
    let encoded = base64_encode(text.as_bytes());
    let bare = format!("\x1b]52;c;{encoded}\x07");
    let sequence = if tmux {
        // Double every ESC inside the payload, then wrap in the tmux DCS frame.
        let escaped = bare.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{escaped}\x1b\\")
    } else {
        bare
    };
    (sequence, truncated)
}

/// How many CHARACTERS of `text` will actually reach the clipboard once the
/// OSC 52 input cap is applied, or `None` when the whole text fits. Lets the
/// `/copy` status line report a truncated copy honestly instead of claiming
/// the full length (the emit-time cap in [`osc52_copy_sequence_capped`] is
/// otherwise invisible to the caller that stages the status).
pub fn osc52_truncated_chars(text: &str) -> Option<usize> {
    let (kept, truncated) = truncate_at_char_boundary(text, OSC52_MAX_INPUT_BYTES);
    truncated.then(|| kept.chars().count())
}

/// Truncate `text` to at most `max_bytes`, backing off to a UTF-8 char
/// boundary so the kept head is always valid UTF-8. Returns the (possibly
/// shortened) slice and whether anything was dropped.
fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (&text[..cut], true)
}

/// The text to copy when the user invokes the copy command: the most recent
/// assistant output for the active session.
///
/// Prefers the in-flight `live_reply` (the answer currently streaming in) and
/// otherwise falls back to the last committed assistant message. Returns
/// `None` when there is no assistant text yet (e.g. a fresh session), so the
/// caller can surface a "nothing to copy" hint instead of clobbering the
/// clipboard with an empty string.
pub fn copyable_assistant_text(state: &AppState) -> Option<String> {
    let session = state.active_session()?;

    if let Some(live) = session.live_reply.as_ref() {
        let trimmed = live.text.trim();
        if !trimmed.is_empty() {
            return Some(live.text.clone());
        }
    }

    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role.as_str() == "assistant" && !message.content.trim().is_empty())
        .map(|message| message.content.clone())
}

/// Minimal RFC 4648 standard base64 encoder (no line wrapping).
///
/// OSC 52 needs a self-contained, single-line base64 payload; rolling the few
/// lines here keeps the escape-sequence builder free of any line-wrapping or
/// alphabet surprises a general-purpose call site might introduce.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Route-2: reading a bitmap OUT of the system clipboard
// ---------------------------------------------------------------------------

/// Wall-clock ceiling for one clipboard-image read.
///
/// The read happens on the UI thread inside the key handler, so an
/// `osascript`/`xclip` that hangs (a wedged pasteboard server, an X server
/// that never answers the selection request) would freeze the whole TUI. The
/// helper is killed past this deadline and the paste degrades to text.
pub const CLIPBOARD_IMAGE_READ_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(2000);

/// What the OS clipboard turned out to be holding.
///
/// The three arms exist because the caller reacts differently to each:
/// `Png` attaches, `Empty` falls silently back to the text paste (the common
/// case — the clipboard usually holds text), and `Unavailable` falls back too
/// but says so in the status line, because the user pressed Ctrl+V expecting
/// an image and the machine could not even look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardImage {
    /// The clipboard holds a bitmap; these are its PNG bytes.
    Png(Vec<u8>),
    /// The clipboard holds no bitmap (text, nothing, an unsupported flavour).
    Empty,
    /// The read could not be performed: helper binary missing, non-zero exit,
    /// timeout, or a zero-byte payload from a "successful" helper.
    ///
    /// The payload is a short diagnostic reason. It NEVER contains clipboard
    /// bytes — the contract forbids clipboard content reaching logs.
    Unavailable(String),
}

/// The seam between "ask the OS for a clipboard bitmap" and everything built
/// on top of it.
///
/// Implemented for real by [`SystemClipboardImageSource`] (external helper
/// processes) and by fakes in tests, so staging, size validation, the turn-image
/// cap and the text-paste fallback are all verifiable without a clipboard, a
/// terminal, or a platform.
pub trait ClipboardImageSource {
    /// Read the clipboard's bitmap as PNG bytes. Must never panic and must
    /// return within roughly [`CLIPBOARD_IMAGE_READ_TIMEOUT`].
    fn read_png(&self) -> ClipboardImage;
}

/// Real clipboard reader: shells out to the platform's helper.
///
/// No crate dependency is added for this (contract: 不新增 crate 依赖); the
/// helpers are the same ones a user would type by hand.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClipboardImageSource;

impl ClipboardImageSource for SystemClipboardImageSource {
    fn read_png(&self) -> ClipboardImage {
        read_system_clipboard_png()
    }
}

/// Probe order is fixed per platform, first available helper wins:
/// macOS → `osascript`; Linux/BSD → Wayland `wl-paste`, then X11 `xclip`.
/// Windows clipboard bitmaps are explicitly out of scope.
fn read_system_clipboard_png() -> ClipboardImage {
    #[cfg(target_os = "macos")]
    {
        read_via_osascript()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match read_via_wl_paste() {
            ClipboardImage::Unavailable(wayland) => match read_via_xclip() {
                ClipboardImage::Unavailable(x11) => {
                    ClipboardImage::Unavailable(format!("{wayland}; {x11}"))
                }
                other => other,
            },
            other => other,
        }
    }
    #[cfg(not(unix))]
    {
        ClipboardImage::Unavailable("clipboard image read unsupported on this platform".into())
    }
}

/// A scratch file the helper writes its PNG into.
///
/// Helpers are given a FILE for stdout rather than a pipe on purpose: a 20 MiB
/// screenshot overruns the ~64 KiB pipe buffer, and a parent that polls
/// `try_wait` (which it must, to honour the timeout) instead of draining the
/// pipe would deadlock the child forever. A file has no such backpressure.
fn helper_scratch_path() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "octos-clipboard-{}-{unique}.png",
        std::process::id()
    ))
}

/// Run `command` with `args`, its stdout redirected to a fresh scratch file,
/// under [`CLIPBOARD_IMAGE_READ_TIMEOUT`]. Returns the scratch file's bytes on
/// a zero exit. The scratch file is always removed.
#[cfg(all(unix, not(target_os = "macos")))]
fn run_helper_to_bytes(command: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let scratch = helper_scratch_path();
    let sink = std::fs::File::create(&scratch)
        .map_err(|err| format!("{command}: cannot create scratch file: {err}"))?;
    let result = run_helper_with_stdout(command, args, sink);
    let bytes = match result {
        Ok(()) => std::fs::read(&scratch).map_err(|err| format!("{command}: {err}")),
        Err(err) => Err(err),
    };
    let _ = std::fs::remove_file(&scratch);
    bytes
}

/// Spawn, wait with a deadline, kill on timeout. Stderr is discarded — a
/// helper's chatter about the clipboard is not something to log.
fn run_helper_with_stdout(
    command: &str,
    args: &[&str],
    stdout: std::fs::File,
) -> Result<(), String> {
    let mut child = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(stdout))
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => format!("{command} not found"),
            _ => format!("{command}: {err}"),
        })?;

    let deadline = std::time::Instant::now() + CLIPBOARD_IMAGE_READ_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("{command} exited with {status}"))
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{command} timed out"));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(err) => return Err(format!("{command}: {err}")),
        }
    }
}

/// True when `list` (a helper's newline-separated flavour list) advertises PNG.
///
/// Split out as a pure helper so the "clipboard simply has no image" branch —
/// the one that must stay SILENT and fall through to text — is testable.
#[cfg(all(unix, not(target_os = "macos")))]
fn advertises_png(list: &str) -> bool {
    list.lines()
        .any(|line| line.trim().eq_ignore_ascii_case("image/png"))
}

/// Read the flavour list a helper prints on stdout (small; a pipe is fine).
#[cfg(all(unix, not(target_os = "macos")))]
fn helper_stdout_text(command: &str, args: &[&str]) -> Result<String, String> {
    let bytes = run_helper_to_bytes(command, args)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(target_os = "macos")]
fn read_via_osascript() -> ClipboardImage {
    // AppleScript is the only dependency-free way to reach the NSPasteboard's
    // PNG flavour. `«class PNGf»` is the PNG type; the `try` block turns "no
    // image on the pasteboard" into a distinguishable sentinel rather than an
    // error, so an image-less clipboard falls through to text SILENTLY while a
    // genuinely broken helper gets a status line.
    let scratch = helper_scratch_path();
    let script = format!(
        r#"set outPath to "{}"
try
    set png to (the clipboard as «class PNGf»)
on error
    return "NOIMAGE"
end try
set fh to open for access (POSIX file outPath) with write permission
set eof fh to 0
write png to fh
close access fh
return "OK""#,
        scratch
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    );

    let outcome = (|| -> Result<String, String> {
        let sentinel = helper_scratch_path().with_extension("txt");
        let sink = std::fs::File::create(&sentinel)
            .map_err(|err| format!("osascript: cannot create scratch file: {err}"))?;
        let ran = run_helper_with_stdout("osascript", &["-e", script.as_str()], sink);
        let text = ran.and_then(|()| {
            std::fs::read_to_string(&sentinel).map_err(|err| format!("osascript: {err}"))
        });
        let _ = std::fs::remove_file(&sentinel);
        text
    })();

    let image = match outcome {
        Err(reason) => ClipboardImage::Unavailable(reason),
        Ok(text) if text.trim() == "NOIMAGE" => ClipboardImage::Empty,
        Ok(_) => match std::fs::read(&scratch) {
            Ok(bytes) if !bytes.is_empty() => ClipboardImage::Png(bytes),
            Ok(_) => ClipboardImage::Unavailable("osascript returned no bytes".into()),
            Err(err) => ClipboardImage::Unavailable(format!("osascript: {err}")),
        },
    };
    let _ = std::fs::remove_file(&scratch);
    image
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_via_wl_paste() -> ClipboardImage {
    // Ask what the clipboard holds first: `wl-paste` exits non-zero both when
    // it is missing a PNG flavour and when it is genuinely broken, and those
    // two must not produce the same user-visible outcome.
    match helper_stdout_text("wl-paste", &["--list-types"]) {
        Err(reason) => ClipboardImage::Unavailable(reason),
        Ok(list) if !advertises_png(&list) => ClipboardImage::Empty,
        Ok(_) => match run_helper_to_bytes("wl-paste", &["--type", "image/png"]) {
            Err(reason) => ClipboardImage::Unavailable(reason),
            Ok(bytes) if bytes.is_empty() => {
                ClipboardImage::Unavailable("wl-paste returned no bytes".into())
            }
            Ok(bytes) => ClipboardImage::Png(bytes),
        },
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_via_xclip() -> ClipboardImage {
    match helper_stdout_text("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"]) {
        Err(reason) => ClipboardImage::Unavailable(reason),
        Ok(list) if !advertises_png(&list) => ClipboardImage::Empty,
        Ok(_) => match run_helper_to_bytes(
            "xclip",
            &["-selection", "clipboard", "-t", "image/png", "-o"],
        ) {
            Err(reason) => ClipboardImage::Unavailable(reason),
            Ok(bytes) if bytes.is_empty() => {
                ClipboardImage::Unavailable("xclip returned no bytes".into())
            }
            Ok(bytes) => ClipboardImage::Png(bytes),
        },
    }
}

/// Staging file name for a clipboard bitmap: `paste-<sha256[..16]>.png`.
///
/// CONTENT-ADDRESSED on purpose. Pasting the same screenshot twice (a common
/// "did that go through?" reflex) must not fill `~/.octos/tmp/paste/` with
/// byte-identical copies, and the same paste across two turns should resolve to
/// one file the server can cache on. 16 hex chars (64 bits) is far past any
/// realistic collision risk for a scratch directory.
pub fn staged_paste_file_name(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let hex: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("paste-{hex}.png")
}

/// Default staging directory for clipboard pastes: `~/.octos/tmp/paste/`.
///
/// Returns `None` when the home directory cannot be resolved, so the caller
/// degrades to a text paste instead of writing somewhere unexpected. Tests
/// always pass an explicit temp directory and never touch the real one.
pub fn default_paste_staging_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::new().join(home).join(".octos/tmp/paste"))
}

/// Write `bytes` to `<dir>/paste-<sha256[..16]>.png`, reusing the file when it
/// already holds the same content.
///
/// Content addressing makes the reuse check cheap and safe: same name implies
/// same bytes, so a matching length is enough to skip the write. A stale/truncated
/// file (interrupted earlier write) is rewritten rather than attached as-is.
pub fn stage_clipboard_png(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(staged_paste_file_name(bytes));
    let already_staged = std::fs::metadata(&path)
        .map(|meta| meta.is_file() && meta.len() == bytes.len() as u64)
        .unwrap_or(false);
    if !already_staged {
        std::fs::write(&path, bytes)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use octos_core::Message;
    use octos_core::SessionKey;
    use octos_core::app_ui::{AppUiLiveReply, AppUiSession};
    use octos_core::ui_protocol::TurnId;

    fn base_session() -> AppUiSession {
        AppUiSession {
            id: SessionKey("local:test".into()),
            title: "t".into(),
            profile_id: None,
            messages: Vec::new(),
            tasks: Vec::new(),
            live_reply: None,
        }
    }

    fn state_with(session: AppUiSession) -> AppState {
        AppState::new(vec![session], 0, "ready".into(), None, false)
    }

    fn empty_state() -> AppState {
        AppState::new(Vec::new(), 0, "ready".into(), None, false)
    }

    // --- base64 (RFC 4648 test vectors) ---

    #[test]
    fn should_match_rfc4648_base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn should_encode_non_ascii_utf8_bytes() {
        // "✓ café" — exercises multi-byte UTF-8 through the byte encoder.
        assert_eq!(base64_encode("✓ café".as_bytes()), "4pyTIGNhZsOp");
    }

    // --- OSC 52 framing ---

    #[test]
    fn should_wrap_payload_in_osc52_clipboard_frame() {
        let seq = osc52_copy_sequence("foobar");
        assert_eq!(seq, "\x1b]52;c;Zm9vYmFy\x07");
    }

    #[test]
    fn should_wrap_in_tmux_passthrough_when_inside_tmux() {
        // Matches codex's tmux DCS frame: ESC P tmux ; <esc-doubled OSC52> ESC \
        let seq = osc52_copy_sequence_for("foobar", /*tmux*/ true);
        assert_eq!(seq, "\x1bPtmux;\x1b\x1b]52;c;Zm9vYmFy\x07\x1b\\");
    }

    #[test]
    fn should_emit_bare_osc52_when_not_in_tmux() {
        let seq = osc52_copy_sequence_for("foobar", /*tmux*/ false);
        assert_eq!(seq, "\x1b]52;c;Zm9vYmFy\x07");
    }

    // --- OSC 52 payload cap ---

    fn payload_of(seq: &str) -> &str {
        seq.strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .expect("bare OSC 52 frame present")
    }

    #[test]
    fn should_not_truncate_input_at_or_below_the_cap() {
        let text = "a".repeat(OSC52_MAX_INPUT_BYTES);
        let (seq, truncated) = osc52_copy_sequence_capped(&text, false);
        assert!(!truncated);
        assert_eq!(payload_of(&seq), base64_encode(text.as_bytes()));
        assert!(payload_of(&seq).len() <= OSC52_MAX_ENCODED_BYTES);
    }

    #[test]
    fn should_cap_oversized_payload_keeping_the_head() {
        // Multi-hundred-KB copy: terminals cap OSC 52 (~100 KB) and would
        // silently no-op. The sequence must stay under the documented cap.
        let text = "x".repeat(300 * 1024);
        let (seq, truncated) = osc52_copy_sequence_capped(&text, false);
        assert!(truncated);
        let payload = payload_of(&seq);
        assert!(payload.len() <= OSC52_MAX_ENCODED_BYTES);
        // Head is kept: payload is exactly base64 of the input's prefix.
        assert_eq!(
            payload,
            base64_encode(&text.as_bytes()[..OSC52_MAX_INPUT_BYTES])
        );
        // Valid base64: length is a multiple of 4 (whole input, no padding split).
        assert_eq!(payload.len() % 4, 0);
    }

    #[test]
    fn should_truncate_on_a_char_boundary() {
        // 2-byte codepoints: with an odd byte cap the cut would land
        // mid-character; the cap must back off to a boundary, never panic.
        let ch = "é"; // 2 bytes
        let text = ch.repeat(OSC52_MAX_INPUT_BYTES); // ~110 KB, boundary at every even byte
        let (seq, truncated) = osc52_copy_sequence_capped(&text, false);
        assert!(truncated);
        let mut cut = OSC52_MAX_INPUT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        assert_eq!(payload_of(&seq), base64_encode(&text.as_bytes()[..cut]));
    }

    #[test]
    fn should_cap_the_tmux_wrapped_variant_too() {
        let text = "y".repeat(300 * 1024);
        let (seq, truncated) = osc52_copy_sequence_capped(&text, true);
        assert!(truncated);
        assert!(seq.starts_with("\x1bPtmux;"));
        // The whole wrapped sequence stays within cap + framing overhead.
        assert!(seq.len() <= OSC52_MAX_ENCODED_BYTES + 32);
    }

    #[test]
    fn public_uncapped_helpers_apply_the_same_cap() {
        let text = "z".repeat(300 * 1024);
        let seq = osc52_copy_sequence_for(&text, false);
        assert!(payload_of(&seq).len() <= OSC52_MAX_ENCODED_BYTES);
    }

    #[test]
    fn should_not_emit_newlines_in_the_escape_sequence() {
        // A literal newline mid-sequence would terminate the OSC string and
        // corrupt the terminal; the base64 payload must be single-line even
        // when the source text spans many lines.
        let seq = osc52_copy_sequence("line one\nline two\nline three\n");
        let payload = seq
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .expect("frame present");
        assert!(!payload.contains('\n'));
        assert!(!payload.contains('\r'));
    }

    // --- selection: what text gets copied ---

    #[test]
    fn should_copy_last_assistant_message_when_no_live_reply() {
        let mut session = base_session();
        session.messages = vec![
            Message::user("please summarize"),
            Message::assistant("first answer"),
            Message::user("and again"),
            Message::assistant("the final report"),
        ];
        let state = state_with(session);
        assert_eq!(
            copyable_assistant_text(&state).as_deref(),
            Some("the final report")
        );
    }

    #[test]
    fn should_prefer_streaming_live_reply_over_committed_message() {
        let mut session = base_session();
        session.messages = vec![Message::assistant("old answer")];
        session.live_reply = Some(AppUiLiveReply {
            turn_id: TurnId::new(),
            text: "streaming answer".into(),
        });
        let state = state_with(session);
        assert_eq!(
            copyable_assistant_text(&state).as_deref(),
            Some("streaming answer")
        );
    }

    #[test]
    fn should_fall_back_to_message_when_live_reply_is_blank() {
        let mut session = base_session();
        session.messages = vec![Message::assistant("committed answer")];
        session.live_reply = Some(AppUiLiveReply {
            turn_id: TurnId::new(),
            text: "   ".into(),
        });
        let state = state_with(session);
        assert_eq!(
            copyable_assistant_text(&state).as_deref(),
            Some("committed answer")
        );
    }

    #[test]
    fn should_return_none_when_session_has_no_assistant_text() {
        let mut session = base_session();
        session.messages = vec![Message::user("just a prompt")];
        let state = state_with(session);
        assert_eq!(copyable_assistant_text(&state), None);
    }

    #[test]
    fn should_return_none_when_no_active_session() {
        let state = empty_state();
        assert_eq!(copyable_assistant_text(&state), None);
    }
}
