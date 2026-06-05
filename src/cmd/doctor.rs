//! `octos-tui doctor` — flutter-doctor-style diagnostics (design §B).
//!
//! One line per check (`[✓]` pass / `[!]` warn / `[✗]` fail), grouped by
//! category, each non-pass line followed by an indented `→ fix:` action,
//! closing with a one-line summary. `--json` emits the same data structured
//! (support bundle; tokens redacted); `--verbose` adds resolved paths/versions.
//!
//! Exit `0` when all checks pass (warnings are OK but mentioned), `1` on any
//! `[✗]`. `--strict` promotes warnings to failures.
//!
//! Checks implemented here:
//! - **Binary & version**: octos-tui on PATH, install method, newer release,
//!   shadowing installs.
//! - **Terminal**: TERM/terminfo, UTF-8 locale, CJK width, color support.
//! - **Config & data**: config dir + data dir writability.
//! - **Backend**: stdio-command resolves (+ `octos --version`), and a
//!   structural **protocol-skew** comparison of the TUI's compiled-in
//!   `octos-core` schema/feature set against the protocol's known feature
//!   registry. The live WS `config/capabilities/list` probe is a documented
//!   TODO (see [`backend_checks`]).
//! - **Network**: GitHub reachability.

use std::path::{Path, PathBuf};

use eyre::Result;
use octos_core::ui_protocol::{
    UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1, UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1,
    UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1, UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1, UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1,
    UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1, UI_PROTOCOL_FEATURE_SESSION_HYDRATE_V1,
    UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1, UI_PROTOCOL_FEATURE_USER_QUESTION_V1,
    UI_PROTOCOL_KNOWN_FEATURES, UI_PROTOCOL_SCHEMA_VERSION, UI_PROTOCOL_V1, UiProtocolCapabilities,
};

use super::github::{self, Reachability};
use super::install_method::{self, InstallMethod};

/// Features the TUI *requires* of any server it connects to (the set it sends
/// in `X-Octos-Ui-Features`). The skew check fails when the server's schema is
/// incompatible and warns when a required feature is missing.
pub const TUI_REQUIRED_FEATURES: &[&str] = &[
    UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1,
    UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1,
    UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1,
    UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1,
    UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1,
    UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1,
    UI_PROTOCOL_FEATURE_SESSION_HYDRATE_V1,
    UI_PROTOCOL_FEATURE_USER_QUESTION_V1,
];

/// Parsed `octos-tui doctor` flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorArgs {
    /// Emit machine-readable JSON (support bundle).
    pub json: bool,
    /// Add resolved paths / versions to each line.
    pub verbose: bool,
    /// Promote warnings to failures (affects exit code).
    pub strict: bool,
    /// stdio child command, if the TUI is configured for stdio transport.
    pub stdio_command: Option<String>,
    /// WS endpoint, if configured.
    pub endpoint: Option<String>,
    /// Data dir override (defaults to `~/.octos`).
    pub data_dir: Option<PathBuf>,
}

/// Pass / warn / fail per check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn glyph(self) -> &'static str {
        match self {
            CheckStatus::Pass => "[✓]",
            CheckStatus::Warn => "[!]",
            CheckStatus::Fail => "[✗]",
        }
    }

    fn json_str(self) -> &'static str {
        match self {
            CheckStatus::Pass => "pass",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }
}

/// A single diagnostic line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub category: &'static str,
    pub name: String,
    pub status: CheckStatus,
    /// One-line detail shown after the name.
    pub detail: String,
    /// Actionable fix, rendered as a `→ fix:` line. `None` for passing checks.
    pub fix: Option<String>,
    /// Optional resolved value (path/version) shown in `--verbose` and JSON.
    pub value: Option<String>,
}

impl Check {
    fn pass(category: &'static str, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            category,
            name: name.into(),
            status: CheckStatus::Pass,
            detail: detail.into(),
            fix: None,
            value: None,
        }
    }

    fn warn(
        category: &'static str,
        name: impl Into<String>,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status: CheckStatus::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
            value: None,
        }
    }

    fn fail(
        category: &'static str,
        name: impl Into<String>,
        detail: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.into(),
            status: CheckStatus::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
            value: None,
        }
    }

    fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }
}

/// Aggregated report.
#[derive(Debug, Clone)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(checks: Vec<Check>) -> Self {
        Self { checks }
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut pass = 0;
        let mut warn = 0;
        let mut fail = 0;
        for c in &self.checks {
            match c.status {
                CheckStatus::Pass => pass += 1,
                CheckStatus::Warn => warn += 1,
                CheckStatus::Fail => fail += 1,
            }
        }
        (pass, warn, fail)
    }

    /// Exit code: `1` on any failure, or (with `strict`) any warning.
    pub fn exit_code(&self, strict: bool) -> i32 {
        let (_, warn, fail) = self.counts();
        if fail > 0 || (strict && warn > 0) {
            1
        } else {
            0
        }
    }

    /// Render the flutter-doctor-style human report to a string.
    pub fn render(&self, verbose: bool, strict: bool) -> String {
        let mut out = String::new();
        let mut last_category: Option<&str> = None;
        for check in &self.checks {
            if last_category != Some(check.category) {
                if last_category.is_some() {
                    out.push('\n');
                }
                out.push_str(check.category);
                out.push('\n');
                last_category = Some(check.category);
            }
            out.push_str(check.status.glyph());
            out.push(' ');
            out.push_str(&check.name);
            if !check.detail.is_empty() {
                out.push_str(" — ");
                out.push_str(&check.detail);
            }
            if verbose {
                if let Some(value) = &check.value {
                    out.push_str(" (");
                    out.push_str(value);
                    out.push(')');
                }
            }
            out.push('\n');
            if let Some(fix) = &check.fix {
                out.push_str("    → fix: ");
                out.push_str(fix);
                out.push('\n');
            }
        }

        let (pass, warn, fail) = self.counts();
        out.push('\n');
        if fail == 0 && (warn == 0 || !strict) {
            out.push_str(&format!(
                "• Doctor summary: {pass} passed, {warn} warning(s). No fatal issues found."
            ));
        } else {
            out.push_str(&format!(
                "• Doctor summary: {pass} passed, {warn} warning(s), {fail} failure(s)."
            ));
        }
        out.push('\n');
        out
    }

    /// Render the support-bundle JSON.
    pub fn to_json(&self, strict: bool) -> serde_json::Value {
        let (pass, warn, fail) = self.counts();
        let checks: Vec<_> = self
            .checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "category": c.category,
                    "name": c.name,
                    "status": c.status.json_str(),
                    "detail": c.detail,
                    "fix": c.fix,
                    "value": c.value,
                })
            })
            .collect();
        serde_json::json!({
            "checks": checks,
            "summary": {
                "passed": pass,
                "warnings": warn,
                "failures": fail,
            },
            "exit_code": self.exit_code(strict),
            "octos_tui_version": env!("CARGO_PKG_VERSION"),
            "octos_core_schema_version": UI_PROTOCOL_SCHEMA_VERSION,
            "octos_protocol": UI_PROTOCOL_V1,
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        })
    }
}

/// Entry point: gather all checks, render, return the exit code.
pub fn run(args: DoctorArgs) -> Result<i32> {
    let mut checks = Vec::new();
    checks.extend(binary_checks(&args));
    checks.extend(terminal_checks());
    checks.extend(config_checks(&args));
    checks.extend(backend_checks(&args));
    checks.extend(network_checks());

    let report = Report::new(checks);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.to_json(args.strict))?
        );
    } else {
        print!("{}", report.render(args.verbose, args.strict));
    }
    Ok(report.exit_code(args.strict))
}

// ---------------------------------------------------------------------------
// Binary & version
// ---------------------------------------------------------------------------

const CAT_BINARY: &str = "Binary & version";

fn binary_checks(_args: &DoctorArgs) -> Vec<Check> {
    let mut checks = Vec::new();

    // current_exe resolves.
    match std::env::current_exe() {
        Ok(exe) => checks.push(
            Check::pass(
                CAT_BINARY,
                "octos-tui binary",
                format!("v{}", env!("CARGO_PKG_VERSION")),
            )
            .with_value(exe.display().to_string()),
        ),
        Err(err) => checks.push(Check::warn(
            CAT_BINARY,
            "octos-tui binary",
            format!("could not resolve current executable: {err}"),
            "ensure octos-tui is on a real filesystem path",
        )),
    }

    // Install method.
    let method = install_method::detect();
    checks.push(Check::pass(CAT_BINARY, "install method", method.label()).with_value(method.id()));

    // Shadowing installs.
    let shadows = find_octos_tui_on_path();
    checks.push(shadow_check(&shadows));

    // Newer release (best-effort; network failure → warn, not fail).
    checks.push(release_check(&method));

    checks
}

/// Build the shadowing-install check from a list of resolved binary paths.
/// `[✓]` when ≤1, `[!]` when >1 (the Claude Code #22415 failure mode).
fn shadow_check(paths: &[PathBuf]) -> Check {
    match paths.len() {
        0 => Check::warn(
            CAT_BINARY,
            "no shadowing installs",
            "octos-tui not found on $PATH",
            "add the install dir to your PATH",
        ),
        1 => Check::pass(CAT_BINARY, "no shadowing installs", "exactly one on PATH")
            .with_value(paths[0].display().to_string()),
        n => {
            let list: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            Check::warn(
                CAT_BINARY,
                "no shadowing installs",
                format!("{n} octos-tui binaries on PATH; first wins: {}", list[0]),
                format!("remove the extras: {}", list[1..].join(", ")),
            )
            .with_value(list.join(" | "))
        }
    }
}

fn release_check(method: &InstallMethod) -> Check {
    match github::latest_release(false) {
        Ok(latest) => {
            let current = env!("CARGO_PKG_VERSION");
            let current_v = super::update::parse_version(current);
            let latest_v = super::update::parse_version(&latest.tag);
            match (current_v, latest_v) {
                (Some(c), Some(l)) if super::update::is_newer(&c, &l) => {
                    let fix = method
                        .upgrade_command()
                        .map(|cmd| cmd.to_string())
                        .unwrap_or_else(|| "run `octos-tui update`".to_string());
                    Check::warn(
                        CAT_BINARY,
                        "up to date",
                        format!("newer release available: {c} -> {l}"),
                        fix,
                    )
                }
                (Some(c), Some(l)) => {
                    Check::pass(CAT_BINARY, "up to date", format!("v{c} is current"))
                        .with_value(l.to_string())
                }
                _ => Check::warn(
                    CAT_BINARY,
                    "up to date",
                    format!("could not parse versions (latest tag {})", latest.tag),
                    "run `octos-tui update --check`",
                ),
            }
        }
        Err(err) => Check::warn(
            CAT_BINARY,
            "up to date",
            format!("could not check GitHub for a newer release: {err}"),
            "run `octos-tui update --check` when online",
        ),
    }
}

/// Enumerate every `octos-tui` on `$PATH` plus known install prefixes,
/// de-duplicated by canonical path, preserving PATH precedence (first wins).
pub fn find_octos_tui_on_path() -> Vec<PathBuf> {
    let exe_name = if cfg!(windows) {
        "octos-tui.exe"
    } else {
        "octos-tui"
    };
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    // Known prefixes that may not be on PATH.
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(&home).join(".cargo").join("bin"));
        dirs.push(PathBuf::from(&home).join(".local").join("bin"));
    }

    for dir in dirs {
        let candidate = dir.join(exe_name);
        if !candidate.is_file() {
            continue;
        }
        let canonical = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical);
        found.push(candidate);
    }
    found
}

// ---------------------------------------------------------------------------
// Terminal environment
// ---------------------------------------------------------------------------

const CAT_TERM: &str = "Terminal environment";

fn terminal_checks() -> Vec<Check> {
    let term = std::env::var("TERM").ok();
    let lang = std::env::var("LANG").ok();
    let lc_all = std::env::var("LC_ALL").ok();
    let lc_ctype = std::env::var("LC_CTYPE").ok();
    let colorterm = std::env::var("COLORTERM").ok();
    vec![
        term_check(term.as_deref()),
        locale_check(lang.as_deref(), lc_all.as_deref(), lc_ctype.as_deref()),
        cjk_check(),
        color_check(term.as_deref(), colorterm.as_deref()),
    ]
}

fn term_check(term: Option<&str>) -> Check {
    match term {
        Some(t) if !t.is_empty() && t != "dumb" => {
            Check::pass(CAT_TERM, "TERM set", t.to_string()).with_value(t.to_string())
        }
        Some("dumb") => Check::warn(
            CAT_TERM,
            "TERM set",
            "TERM=dumb has no terminfo capabilities",
            "export TERM=xterm-256color",
        ),
        _ => Check::warn(
            CAT_TERM,
            "TERM set",
            "TERM is unset; the TUI may not render or may report 'can't find terminfo database'",
            "export TERM=xterm-256color",
        ),
    }
}

fn locale_check(lang: Option<&str>, lc_all: Option<&str>, lc_ctype: Option<&str>) -> Check {
    let effective = lc_all.or(lc_ctype).or(lang);
    match effective {
        Some(v)
            if v.to_ascii_uppercase().contains("UTF-8")
                || v.to_ascii_uppercase().contains("UTF8") =>
        {
            Check::pass(CAT_TERM, "UTF-8 locale", v.to_string()).with_value(v.to_string())
        }
        Some(v) => Check::warn(
            CAT_TERM,
            "UTF-8 locale",
            format!("locale `{v}` is not UTF-8; box-drawing and CJK may break"),
            "export LANG=en_US.UTF-8 (or your locale with .UTF-8)",
        ),
        None => Check::warn(
            CAT_TERM,
            "UTF-8 locale",
            "no LANG/LC_ALL/LC_CTYPE set",
            "export LANG=en_US.UTF-8",
        ),
    }
}

fn cjk_check() -> Check {
    // Informational: octos-tui uses `unicode-width` for CJK double-width; the
    // visible result also depends on the terminal font, so this never fails.
    Check::pass(
        CAT_TERM,
        "CJK width",
        "uses unicode-width for double-width glyphs (also depends on terminal font)",
    )
}

fn color_check(term: Option<&str>, colorterm: Option<&str>) -> Check {
    let truecolor = colorterm
        .map(|c| c.contains("truecolor") || c.contains("24bit"))
        .unwrap_or(false);
    let has_256 = term.map(|t| t.contains("256color")).unwrap_or(false);
    if truecolor {
        Check::pass(CAT_TERM, "color support", "truecolor (24-bit)")
    } else if has_256 {
        Check::pass(CAT_TERM, "color support", "256-color")
    } else {
        Check::warn(
            CAT_TERM,
            "color support",
            "no truecolor/256-color advertised; themes may look flat",
            "use a 256-color terminal and set TERM=xterm-256color (COLORTERM=truecolor)",
        )
    }
}

// ---------------------------------------------------------------------------
// Config & data
// ---------------------------------------------------------------------------

const CAT_CONFIG: &str = "Config & data";

fn config_checks(args: &DoctorArgs) -> Vec<Check> {
    let data_dir = args
        .data_dir
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".octos")))
        .unwrap_or_else(|| PathBuf::from(".octos"));
    vec![writability_check("octos data dir", &data_dir)]
}

/// Check that a directory exists and is writable (or creatable). A missing dir
/// that can be created is a `[!]` with a `--fix`-able action, not a failure.
fn writability_check(name: &'static str, dir: &Path) -> Check {
    if dir.is_dir() {
        if is_writable(dir) {
            Check::pass(CAT_CONFIG, name, "present and writable")
                .with_value(dir.display().to_string())
        } else {
            Check::fail(
                CAT_CONFIG,
                name,
                format!("{} is not writable", dir.display()),
                format!("chmod u+w {}", dir.display()),
            )
        }
    } else {
        Check::warn(
            CAT_CONFIG,
            name,
            format!("{} does not exist yet", dir.display()),
            format!("mkdir -p {}", dir.display()),
        )
        .with_value(dir.display().to_string())
    }
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(".octos-tui-doctor-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Backend connectivity + protocol skew
// ---------------------------------------------------------------------------

const CAT_BACKEND: &str = "Backend";

fn backend_checks(args: &DoctorArgs) -> Vec<Check> {
    let mut checks = Vec::new();

    // Transport resolution.
    if let Some(cmd) = &args.stdio_command {
        checks.push(stdio_command_check(cmd));
    } else if let Some(endpoint) = &args.endpoint {
        // Live WS probe is a documented TODO; we record the configured endpoint
        // and run the structural skew check below regardless.
        checks.push(
            Check::warn(
                CAT_BACKEND,
                "WS endpoint probe",
                format!("endpoint configured ({endpoint}); live config/capabilities/list probe not yet wired"),
                "run `octos-tui --endpoint … ` to exercise the live connection (TODO: doctor live WS probe)",
            )
            .with_value(endpoint.clone()),
        );
    } else {
        checks.push(Check::pass(
            CAT_BACKEND,
            "transport",
            "no backend configured (mock mode); skipping connectivity",
        ));
    }

    // Structural protocol-skew check (always runs; does not need a live
    // server). Compares the TUI's required feature set + compiled-in schema
    // version against the octos-core feature registry the TUI is built with.
    checks.push(protocol_skew_check());

    checks
}

/// Resolve the first token of `--stdio-command` on PATH and, if it is the
/// `octos` server, run `<bin> --version` to surface the build it would launch.
fn stdio_command_check(command: &str) -> Check {
    let Some(program) = shlex::split(command).and_then(|parts| parts.into_iter().next()) else {
        return Check::fail(
            CAT_BACKEND,
            "stdio command",
            format!("could not parse stdio command `{command}`"),
            "set a valid --stdio-command (e.g. `octos serve --stdio`)",
        );
    };

    let resolved = which(&program);
    match resolved {
        Some(path) => {
            // Surface the server build (best effort).
            let version = std::process::Command::new(&path)
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            let detail = match &version {
                Some(v) if !v.is_empty() => format!("resolves to {} ({v})", path.display()),
                _ => format!("resolves to {}", path.display()),
            };
            Check::pass(CAT_BACKEND, "stdio command", detail).with_value(path.display().to_string())
        }
        None => Check::fail(
            CAT_BACKEND,
            "stdio command",
            format!("`{program}` not found on PATH"),
            format!("install `{program}` or correct --stdio-command"),
        ),
    }
}

/// Structural protocol-skew check (design §B, P3 fallback).
///
/// Compares what the TUI requires against the `octos-core` it was compiled
/// with: confirms every [`TUI_REQUIRED_FEATURES`] entry is a known feature in
/// this protocol build (so the TUI isn't asking for a feature the protocol
/// crate no longer defines), and reports the compiled-in protocol/schema
/// version. A live server `config/capabilities/list` comparison reuses
/// [`compare_against_server`]; wiring the live WS handshake is a TODO.
fn protocol_skew_check() -> Check {
    let unknown: Vec<&str> = TUI_REQUIRED_FEATURES
        .iter()
        .copied()
        .filter(|f| !UI_PROTOCOL_KNOWN_FEATURES.contains(f))
        .collect();
    if unknown.is_empty() {
        Check::pass(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "TUI requires {} features; all known in {UI_PROTOCOL_V1} (schema v{UI_PROTOCOL_SCHEMA_VERSION})",
                TUI_REQUIRED_FEATURES.len()
            ),
        )
        .with_value(format!("{UI_PROTOCOL_V1} schema v{UI_PROTOCOL_SCHEMA_VERSION}"))
    } else {
        Check::fail(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "TUI requires features absent from its octos-core build: {}",
                unknown.join(", ")
            ),
            "re-pin octos-tui's octos-core revision to one that defines these features",
        )
    }
}

/// Compare the TUI's compiled-in protocol against a live server's advertised
/// capabilities. Reusable by a future live WS/stdio probe.
///
/// - `[✗]` when the protocol string differs or the server's schema version is
///   *older* than the TUI's compiled-in schema (incompatible).
/// - `[!]` when the server is missing a feature the TUI requires.
/// - `[✓]` otherwise.
pub fn compare_against_server(server: &UiProtocolCapabilities) -> Check {
    if server.version.protocol != UI_PROTOCOL_V1 {
        return Check::fail(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "server speaks `{}` but the TUI speaks `{UI_PROTOCOL_V1}`",
                server.version.protocol
            ),
            "upgrade whichever side is on the wrong protocol family",
        );
    }
    if server.version.schema_version < UI_PROTOCOL_SCHEMA_VERSION {
        return Check::fail(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "server schema v{} is older than the TUI's v{UI_PROTOCOL_SCHEMA_VERSION}",
                server.version.schema_version
            ),
            "upgrade the octos server (`octos update`) so its schema ≥ the client's",
        );
    }
    let missing: Vec<&str> = TUI_REQUIRED_FEATURES
        .iter()
        .copied()
        .filter(|f| !server.supported_features.iter().any(|s| s == f))
        .collect();
    if missing.is_empty() {
        Check::pass(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "compatible (server schema v{}, all required features present)",
                server.version.schema_version
            ),
        )
    } else {
        Check::warn(
            CAT_BACKEND,
            "protocol skew",
            format!(
                "server is missing TUI-required features: {}",
                missing.join(", ")
            ),
            "upgrade the octos server to advertise these features, or expect degraded behavior",
        )
    }
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

const CAT_NETWORK: &str = "Network";

fn network_checks() -> Vec<Check> {
    let check = match github::reachability() {
        Reachability::Ok => Check::pass(CAT_NETWORK, "GitHub reachable", "api.github.com OK"),
        Reachability::RateLimited => Check::warn(
            CAT_NETWORK,
            "GitHub reachable",
            "api.github.com rate-limited (HTTP 403)",
            "set OCTOS_TUI_GITHUB_TOKEN to raise the rate limit",
        ),
        Reachability::Unreachable(err) => Check::warn(
            CAT_NETWORK,
            "GitHub reachable",
            format!("api.github.com unreachable: {err}"),
            "check your network/proxy; update checks will be unavailable",
        ),
    };
    vec![check]
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Cross-platform `which`: resolve `program` against `$PATH`.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe = if cfg!(windows) && !program.ends_with(".exe") {
        format!("{program}.exe")
    } else {
        program.to_string()
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(&exe))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use octos_core::ui_protocol::UiProtocolCapabilities;

    fn server_caps() -> UiProtocolCapabilities {
        UiProtocolCapabilities::full_protocol()
    }

    #[test]
    fn renderer_groups_by_category_and_shows_fix_lines() {
        let checks = vec![
            Check::pass("Cat A", "ok thing", "all good"),
            Check::warn("Cat A", "warny thing", "soft problem", "do the fix"),
            Check::fail("Cat B", "broken thing", "hard problem", "fix me"),
        ];
        let report = Report::new(checks);
        let text = report.render(false, false);
        assert!(text.contains("Cat A\n"));
        assert!(text.contains("Cat B\n"));
        assert!(text.contains("[✓] ok thing"));
        assert!(text.contains("[!] warny thing"));
        assert!(text.contains("[✗] broken thing"));
        assert!(text.contains("    → fix: do the fix"));
        assert!(text.contains("    → fix: fix me"));
        // No fix line for the passing check.
        assert!(!text.contains("→ fix: \n"));
        assert!(text.contains("1 passed, 1 warning(s), 1 failure(s)"));
    }

    #[test]
    fn exit_code_is_one_on_failure_zero_on_warnings() {
        let warn_only = Report::new(vec![Check::warn("c", "n", "d", "f")]);
        assert_eq!(warn_only.exit_code(false), 0);
        assert_eq!(warn_only.exit_code(true), 1); // strict promotes warnings

        let with_fail = Report::new(vec![Check::fail("c", "n", "d", "f")]);
        assert_eq!(with_fail.exit_code(false), 1);
    }

    #[test]
    fn json_redacts_nothing_sensitive_and_carries_summary() {
        let report = Report::new(vec![Check::pass("c", "n", "d")]);
        let json = report.to_json(false);
        assert_eq!(json["summary"]["passed"], 1);
        assert_eq!(json["octos_tui_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            json["octos_core_schema_version"],
            UI_PROTOCOL_SCHEMA_VERSION
        );
        assert!(json["checks"].is_array());
    }

    #[test]
    fn shadow_check_passes_for_single_and_warns_for_multiple() {
        let one = shadow_check(&[PathBuf::from("/usr/local/bin/octos-tui")]);
        assert_eq!(one.status, CheckStatus::Pass);

        let two = shadow_check(&[
            PathBuf::from("/opt/homebrew/bin/octos-tui"),
            PathBuf::from("/home/u/.cargo/bin/octos-tui"),
        ]);
        assert_eq!(two.status, CheckStatus::Warn);
        assert!(two.detail.contains("2 octos-tui binaries"));
        assert!(two.fix.unwrap().contains(".cargo/bin/octos-tui"));
    }

    #[test]
    fn term_check_warns_when_unset_or_dumb() {
        assert_eq!(term_check(None).status, CheckStatus::Warn);
        assert_eq!(term_check(Some("dumb")).status, CheckStatus::Warn);
        assert_eq!(term_check(Some("xterm-256color")).status, CheckStatus::Pass);
    }

    #[test]
    fn locale_check_requires_utf8() {
        assert_eq!(
            locale_check(Some("en_US.UTF-8"), None, None).status,
            CheckStatus::Pass
        );
        assert_eq!(
            locale_check(Some("C"), None, None).status,
            CheckStatus::Warn
        );
        assert_eq!(locale_check(None, None, None).status, CheckStatus::Warn);
        // LC_ALL overrides LANG.
        assert_eq!(
            locale_check(Some("C"), Some("en_US.UTF-8"), None).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn color_check_recognizes_truecolor_and_256() {
        assert_eq!(
            color_check(Some("xterm"), Some("truecolor")).status,
            CheckStatus::Pass
        );
        assert_eq!(
            color_check(Some("xterm-256color"), None).status,
            CheckStatus::Pass
        );
        assert_eq!(color_check(Some("xterm"), None).status, CheckStatus::Warn);
    }

    #[test]
    fn structural_skew_check_passes_against_own_core_build() {
        // Every TUI-required feature must be a known feature in the octos-core
        // this crate compiles against — otherwise the TUI ships broken.
        assert_eq!(protocol_skew_check().status, CheckStatus::Pass);
    }

    #[test]
    fn compare_against_server_passes_for_full_protocol() {
        let check = compare_against_server(&server_caps());
        assert_eq!(check.status, CheckStatus::Pass, "{:?}", check);
    }

    #[test]
    fn compare_against_server_warns_when_feature_missing() {
        let mut caps = server_caps();
        caps.supported_features
            .retain(|f| f != UI_PROTOCOL_FEATURE_USER_QUESTION_V1);
        let check = compare_against_server(&caps);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains(UI_PROTOCOL_FEATURE_USER_QUESTION_V1));
    }

    #[test]
    fn compare_against_server_fails_on_older_schema() {
        let mut caps = server_caps();
        // Force an incompatible (older) server schema.
        if UI_PROTOCOL_SCHEMA_VERSION > 0 {
            caps.version.schema_version = UI_PROTOCOL_SCHEMA_VERSION - 1;
            let check = compare_against_server(&caps);
            assert_eq!(check.status, CheckStatus::Fail);
            assert!(check.detail.contains("older"));
        }
    }

    #[test]
    fn compare_against_server_fails_on_wrong_protocol_family() {
        let mut caps = server_caps();
        caps.version.protocol = "octos-ui/v2alpha".into();
        let check = compare_against_server(&caps);
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn writability_check_passes_for_writable_tempdir() {
        let dir = std::env::temp_dir();
        let check = writability_check("tmp", &dir);
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn writability_check_warns_for_missing_dir() {
        let missing = std::env::temp_dir().join("octos-tui-doctor-nope-xyz-12345");
        let _ = std::fs::remove_dir_all(&missing);
        let check = writability_check("missing", &missing);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.fix.unwrap().contains("mkdir -p"));
    }
}
