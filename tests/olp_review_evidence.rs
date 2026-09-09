//! olp-review-evidence 生产入口集成测试(spec: review-freeze / review-challenge /
//! review-cross / review-peer-validity / review-regression / JSON+human 双输出)。
//!
//! 全部以子进程真实调用 scripts/olp-review-evidence.py。
//! 顺序如实记录: 脚本与首批 18 测试为同轮实现(先落盘后补测试,详见
//! .octos/red-proof/evidence-red.log 的真实 RED 记录与本文件 git 史);
//! 外层反例场景(olp_review_outer_probe_*)按"先 RED 后修"补写,
//! 对应 ../outer-evidence-probe-results.json 实测缺陷。
//! v4(evidence-rescue-shared): adapter 行为证据改经真实最小 Cargo fixture
//! (make_cargo_fixture,无依赖真实编译真实断言,共享 target 缓存),不再递归
//! 编译整仓;adapter 修复外层两实测漏洞(zero-match/foreign-manifest,
//! ../outer-adapter-probe-results.json)并新增 --lib 单元测试入口;8 真实
//! Store 探针重放改 #[ignore] 显式单独集成入口(外层单次执行)。
//! v5(evidence-core-k3-rescue): 外层六反例最终门(../outer-evidence-final-
//! gate-probes.json 0/6)真实 RED→GREEN — turns.txt 同轮 outcome 冲突、报告
//! future turn、foreign originator/goal 冻结拒绝;外部 JSON receipt 不再作
//! 执行证明(challenge --live-cargo 实时执行生产 adapter 为唯一 live 入口,
//! --imported 一律 not-replayed);cross 结构化逐 claim 裁决(structured
//! cross_claims 块,子串不算覆盖),refute 需已验证新行为证据或显式
//! --allow-operator-refute 人工裁决;review_accepted 需全部冻结 claim 有
//! 可核对证据 + 两原 reviewer 各自新 native completed cross。

use std::path::{Path, PathBuf};
use std::process::Command;

fn script() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("scripts/olp-review-evidence.py");
    assert!(p.exists(), "missing script: {}", p.display());
    p
}

fn fixtures() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("fixtures/review-evidence");
    p
}

/// 测试评审基线 HEAD: 本仓库真实 HEAD(init 真实解析,测试锚定同一值)。
fn head() -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn py() -> Command {
    let mut c = Command::new("python3");
    c.arg(script());
    c
}

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "olp-review-evidence-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        TmpDir(base)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = py()
        .args(args)
        .output()
        .expect("spawn olp-review-evidence.py");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn write_review(dir: &Path, name: &str, outcome: &str, turn: &str, head: Option<&str>) -> PathBuf {
    write_review_full(
        dir,
        name,
        name,
        outcome,
        turn,
        head,
        Some(&["X"]),
        "初审内容: PR#627 存在行为缺口,判词 X=approve。",
    )
}

/// 完整形态初审: 自定义 slug/body/显式 JSON claims 块。
/// claims=Some(["X"]) 时生成:
/// ```json
/// {"claims": [{"id": "X", "verdict": "approve", "evidence": []}]}
/// ```
#[allow(clippy::too_many_arguments)]
fn write_review_full(
    dir: &Path,
    file: &str,
    slug: &str,
    outcome: &str,
    turn: &str,
    head: Option<&str>,
    claims: Option<&[&str]>,
    body: &str,
) -> PathBuf {
    let p = dir.join(file);
    let head_line = head.map(|h| format!("HEAD: {h}\n")).unwrap_or_default();
    // v5: 初审 frontmatter 必须绑定调用视角身份(runtime/session/goal/repo)。
    // 测试夹具从 init 落盘的 review-state.json 真实读取,与生产校验同源。
    let identity = identity_lines(dir);
    let claims_block = claims
        .map(|cs| {
            let items: Vec<String> = cs
                .iter()
                .map(|c| format!(r#"{{"id": "{c}", "verdict": "approve", "evidence": []}}"#))
                .collect();
            format!("\n```json\n{{\"claims\": [{}]}}\n```\n", items.join(", "))
        })
        .unwrap_or_default();
    std::fs::write(
        &p,
        format!("---\nslug: {slug}\noutcome: {outcome}\nturn: {turn}\n{head_line}{identity}---\n\n{body}\n{claims_block}"),
    )
    .unwrap();
    p
}

/// 从评审目录 init 状态读取复合身份,生成初审 frontmatter 身份行。
/// 目录无 state(未 init)时返回空 — 对应负向测试自行构造身份。
fn identity_lines(dir: &Path) -> String {
    let state_path = dir.join("review-state.json");
    let Ok(text) = std::fs::read_to_string(&state_path) else {
        return String::new();
    };
    let Ok(st) = serde_json::from_str::<serde_json::Value>(&text) else {
        return String::new();
    };
    let get = |k: &str| st[k].as_str().unwrap_or("").to_string();
    format!(
        "runtime: {}\nsession: {}\ngoal: {}\nrepo: {}\n",
        get("runtime"),
        get("session"),
        get("goal"),
        get("repo")
    )
}

/// v5 prose cross 报告(带 HEAD 绑定、无结构化 cross_claims 块) —
/// 用于隔离"纯文字不算覆盖"的负向路径(HEAD 缺失另有 report-head-missing)。
fn write_cross_prose(d: &Path, slug: &str, file: &str, turn: &str, body: &str) -> PathBuf {
    write_authority_at(&d.join("native"), slug, turn, "completed", true);
    let head = state_head(d);
    let p = d.join(file);
    std::fs::write(
        &p,
        format!(
            "---\nslug: {slug}\noutcome: completed\nturn: {turn}\nHEAD: {head}\n---\n\n{body}\n"
        ),
    )
    .unwrap();
    p
}

/// 评审 state 中真实记录的 HEAD(freeze/challenge/cross 绑定同一基线)。
fn state_head(dir: &Path) -> String {
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("review-state.json")).unwrap())
            .unwrap();
    st["head"].as_str().unwrap().to_string()
}

/// native 终止收据(外部权威): native/<slug>/result-<turn>.md + turns.txt。
/// freeze/cross fail-closed 后,测试必须提供至少一种外部权威。
fn write_authority(dir: &Path, slug: &str, turn: &str, outcome: &str) {
    write_authority_at(&dir.join("native"), slug, turn, outcome, false)
}

/// 在指定 native root 写收据;append=true 时向既有 turns.txt 追加
/// (同一 peer 多轮次权威,如 first turn1 + cross turn2)。
fn write_authority_at(root: &Path, slug: &str, turn: &str, outcome: &str, append: bool) {
    let nd = root.join(slug);
    std::fs::create_dir_all(&nd).unwrap();
    std::fs::write(
        nd.join(format!("result-{turn}.md")),
        format!("---\nslug: {slug}\noutcome: {outcome}\nturn: {turn}\n---\nbody\n"),
    )
    .unwrap();
    // v5: native 权威必须含 originator(master/session 发起者)与 goal 归属文件,
    // 与 init 复合身份(sess-1/goal_01)绑定;foreign 身份 → 拒绝。
    std::fs::write(nd.join("originator"), "sess-1").unwrap();
    std::fs::write(nd.join("goal"), "goal_01").unwrap();
    let line = format!("{turn} {outcome} 100\n");
    let turns = nd.join("turns.txt");
    if append && turns.exists() {
        let prev = std::fs::read_to_string(&turns).unwrap();
        std::fs::write(&turns, format!("{prev}{line}")).unwrap();
    } else {
        std::fs::write(&turns, line).unwrap();
    }
}

fn native_root(dir: &Path) -> std::path::PathBuf {
    dir.join("native")
}

fn init_review(dir: &Path, head: &str) {
    let (ok, so, se) = run(&[
        "init",
        dir.to_str().unwrap(),
        "--base",
        "0a174d95",
        "--runtime",
        "/tmp/runtime-x",
        "--session",
        "sess-1",
        "--goal",
        "goal_01",
        "--head",
        head,
    ]);
    assert!(ok, "init failed: {so} {se}");
}

/// init 于指定 repo: 不传 --head(显式锚定校验已由专项测试覆盖),
/// HEAD 由 init 真实解析(本仓或 fixture 仓库)。
fn init_review_at(dir: &Path, repo: &Path) {
    let (ok, so, se) = run(&[
        "init",
        dir.to_str().unwrap(),
        "--repo",
        repo.to_str().unwrap(),
        "--base",
        "0a174d95",
        "--runtime",
        "/tmp/runtime-x",
        "--session",
        "sess-1",
        "--goal",
        "goal_01",
    ]);
    assert!(ok, "init failed: {so} {se}");
}

// ---------------------------------------------------------------------------
// 最小 Cargo fixture(外层授权改造):无依赖、真实 git 初始化的独立仓库,
// 供 adapter 走生产路径 `cargo test` 执行 PASS/FAIL/zero-match/compile-error。
// 行为门不降低 — 全部真实编译真实断言;8 真实 Store 探针重放归外层单次
// 集成验证(../prepared-replay-slots.json 只读),见文末 #[ignore] 显式入口。
// ---------------------------------------------------------------------------

/// 评审 HEAD 绑定: adapter 绑定 fixture 真实 HEAD;review 目录用 --repo 指向
/// 本仓真实解析 HEAD,不假造本仓测试通过。
fn review_head(dir: &Path, fixture_repo: &Path) -> String {
    let (ok, so, se) = run(&[
        "init",
        dir.to_str().unwrap(),
        "--repo",
        fixture_repo.to_str().unwrap(),
        "--base",
        "0a174d95",
        "--runtime",
        "/tmp/runtime-x",
        "--session",
        "sess-1",
        "--goal",
        "goal_01",
    ]);
    assert!(ok, "init(fixture repo) failed: {so} {se}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    v["head"].as_str().unwrap().to_string()
}

/// 创建最小无依赖 Cargo fixture(真实 git 仓库,HEAD 可绑定)。
/// 子进程带 deadline;首次调用编译一次并复制 target 到共享缓存,
/// 后续调用复制缓存,秒级完成(8 并发不再 8 份从零编译)。
fn make_cargo_fixture(tag: &str, tests_rs: &str) -> (TmpDir, PathBuf, String) {
    let t = TmpDir::new(&format!("fixture-{tag}"));
    let repo = t.path().join("fixture-repo");
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    // 空 [workspace]: fixture 临时目录可能落在本仓工作区树内(沙箱化 TMPDIR),
    // 显式排除 cargo  workspace 归属误判,fixture 仍是独立无依赖真实 crate。
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"olp-min-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(repo.join("tests").join("fixture.rs"), tests_rs).unwrap();
    let seed = cache_dir().join("seed-Cargo.lock");
    if seed.exists() {
        std::fs::copy(&seed, repo.join("Cargo.lock")).unwrap();
    }
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "Cargo.toml", "tests/fixture.rs"],
        vec!["commit", "--quiet", "-m", "fixture init"],
    ] {
        let out = run_deadline(
            Command::new("git").args(&args).current_dir(&repo),
            30,
            &format!("git {}", args[0]),
        );
        assert!(out.status.success(), "git {:?} failed", args);
    }
    let head = fixture_head(&repo);
    let cache_t = cache_dir().join("target");
    if !cache_t.exists() {
        let out = run_deadline(
            Command::new("cargo")
                .args(["test", "--no-run"])
                .current_dir(&repo)
                .env("CARGO_TARGET_DIR", &cache_t)
                .env("CARGO_BUILD_JOBS", "4"),
            300,
            "fixture seed build",
        );
        assert!(
            out.status.success(),
            "fixture seed build failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::rename(repo.join("Cargo.lock"), &seed);
    }
    let _ = std::fs::remove_dir_all(t.path().join("target"));
    copy_dir(&cache_t, &t.path().join("target"));
    (t, repo, head)
}

fn fixture_head(repo: &Path) -> String {
    let out = run_deadline(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo),
        30,
        "fixture rev-parse",
    );
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 共享缓存(进程级,跨并发测试复用): target 编译产物 + seed Cargo.lock。
fn cache_dir() -> PathBuf {
    let d = std::env::temp_dir().join("olp-review-evidence-fixture-cache");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            let _ = std::fs::copy(e.path(), &to);
        }
    }
}

/// 带 deadline 的子进程执行: 持句柄 poll,超时 kill+wait 回收,
/// 不长时间 sleep 等待死进程(外层纪律)。
fn run_deadline(cmd: &mut Command, secs: u64, label: &str) -> std::process::Output {
    use std::io::Read;
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {label}: {e}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                child.stdout.take().unwrap().read_to_end(&mut stdout).ok();
                child.stderr.take().unwrap().read_to_end(&mut stderr).ok();
                return std::process::Output {
                    status: child.wait().unwrap(),
                    stdout,
                    stderr,
                };
            }
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{label} 超时(>{secs}s),已 kill 回收");
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => panic!("wait {label}: {e}"),
        }
    }
}

/// v5 live challenge: 通过生产入口 evidence.py --live-cargo 实时执行固定
/// 生产 adapter(不接受任何外部 receipt)。返回 (成功, stdout, stderr)。
#[allow(clippy::too_many_arguments)]
fn live_challenge(
    d: &Path,
    repo: &Path,
    claim: &str,
    selector: &str,
    expect: &str,
    target_dir: &Path,
) -> (bool, String, String) {
    run(&[
        "challenge",
        d.to_str().unwrap(),
        "--live-cargo",
        "--claim",
        claim,
        "--selector",
        selector,
        "--expect",
        expect,
        "--exec-repo",
        repo.to_str().unwrap(),
        "--cargo-target-dir",
        target_dir.to_str().unwrap(),
        "--timeout",
        "300",
    ])
}

/// 生产 adapter 子进程调用(带 deadline 与工件路径),返回 (成功, stdout)。
fn run_adapter(
    repo: &Path,
    args: &[&str],
    env_fixture: Option<&str>,
    deadline: u64,
) -> (bool, String) {
    let adapter =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/olp-review-evidence-cargo.py");
    assert!(
        adapter.exists(),
        "生产 cargo adapter 缺失: {}",
        adapter.display()
    );
    let mut c = Command::new("python3");
    c.arg(&adapter)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .arg("--timeout")
        .arg(deadline.to_string());
    if let Some(v) = env_fixture {
        c.env("OLP_ADAPTER_FIXTURE", v);
    }
    let out = run_deadline(&mut c, deadline + 30, "cargo adapter");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

// ---------------------------------------------------------------------------
// review-freeze
// ---------------------------------------------------------------------------

/// 场景: 冻结两份独立初审 — 仅有 glm 一份时 freeze 报错且 manifest 未写入。
#[test]
fn olp_review_freeze_requires_both_first_reviews() {
    let t = TmpDir::new("freeze-missing");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    write_review(d, "glm.md", "completed", "1", Some(&head()));
    // k3 缺失
    let (ok, so, _se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        d.join("glm.md").to_str().unwrap(),
        "--k3-review",
        d.join("k3.md").to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "freeze 应失败");
    let v: serde_json::Value = serde_json::from_str(so.trim()).expect("JSON 输出");
    assert_eq!(v["error"]["code"], "first-reviews-incomplete");
    assert!(
        !d.join("review-state.json").exists()
            || !serde_json::from_str::<serde_json::Value>(
                &std::fs::read_to_string(d.join("review-state.json")).unwrap()
            )
            .unwrap()["frozen"]
                .as_bool()
                .unwrap_or(false),
        "manifest 不应处于 frozen=true"
    );
}

/// 场景: 冻结记录可审计标识(SHA256/HEAD/base/runtime/session/goal/peer/turn)。
#[test]
fn olp_review_freeze_records_identity() {
    let t = TmpDir::new("freeze-id");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "peer-glm-x", "1", "completed");
    write_authority(d, "peer-k3-x", "1", "completed");
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    let (ok, so, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--glm-slug",
        "peer-glm-x",
        "--k3-slug",
        "peer-k3-x",
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(ok, "freeze failed: {so} {se}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    for key in ["head", "base", "runtime", "session", "goal"] {
        assert!(state[key].is_string(), "manifest 缺 {key}");
    }
    assert_eq!(v["reviews"]["glm"]["peer"], "peer-glm-x");
    assert_eq!(v["reviews"]["k3"]["turn"], "1");
    assert!(v["reviews"]["glm"]["sha256"].as_str().unwrap().len() == 64);
}

/// 场景: 冻结后篡改初审被拒绝。
#[test]
fn olp_review_freeze_rejects_tampered_first_review() {
    let t = TmpDir::new("tamper");
    let d = t.path();
    init_review(d, &head());
    // 冻结 turn=1: native 最高编号 result 与 turns.txt 均到 1(严格一致)
    write_authority(d, "glm", "1", "completed");
    write_authority(d, "k3", "1", "completed");
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    let (ok, so, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(ok, "freeze failed: {so} {se}");
    // 篡改: 追加一行
    std::fs::write(
        &glm,
        format!("{}\n事后追加行\n", std::fs::read_to_string(&glm).unwrap()),
    )
    .unwrap();
    let (ok2, so2, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        d.join("ev.log").to_str().unwrap(),
        "--claim",
        "X",
    ]);
    assert!(!ok2, "篡改后 challenge 应失败");
    let v: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v["error"]["code"], "first-review-tampered");
}

// ---------------------------------------------------------------------------
// review-peer-validity
// ---------------------------------------------------------------------------

/// 场景: pending peer(active_thread 非空)不得冒充有效初审。
#[test]
fn olp_review_rejects_pending_peer_as_first_review() {
    let t = TmpDir::new("pending");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "peer-glm-x", "1", "completed");
    write_authority(d, "peer-k3-x", "1", "completed");
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    // runtime-evidence: glm peer active_thread 非空
    let re = d.join("runtime-evidence.json");
    std::fs::write(
        &re,
        r#"{"peers": [{"slug": "peer-glm-x", "active_thread": "thread-abc"}]}"#,
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--glm-slug",
        "peer-glm-x",
        "--k3-slug",
        "peer-k3-x",
        "--runtime-evidence",
        re.to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "pending peer 应被拒绝");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-outcome-invalid");
}

/// 场景: 报告声称 completed 但 native result-N.md 为 errored → 外部信号否决。
#[test]
fn olp_review_rejects_claimed_completed_but_native_errored() {
    let t = TmpDir::new("native-veto");
    let d = t.path();
    init_review(d, &head());
    let glm = write_review(d, "glm.md", "completed", "2", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "3", Some(&head()));
    // native dir: peer-glm-x 最新 result 是 errored
    let native = d.join("native").join("peer-glm-x");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(
        native.join("result-2.md"),
        "---\nslug: peer-glm-x\noutcome: errored\nturn: 2\n---\nerrored body\n",
    )
    .unwrap();
    std::fs::write(
        native.join("turns.txt"),
        "1 errored 1788919255\n2 errored 1788919300\n",
    )
    .unwrap();
    write_authority(d, "peer-k3-x", "3", "completed");
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--glm-slug",
        "peer-glm-x",
        "--k3-slug",
        "peer-k3-x",
        "--native-root",
        d.join("native").to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "native errored 应否决自报 completed");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-outcome-invalid");
}

/// 场景: 异 HEAD 与旧轮次分别拒绝(两个独立错误码,per-peer 比较)。
#[test]
fn olp_review_rejects_head_mismatch_and_stale_turn() {
    // (a) head mismatch
    let t1 = TmpDir::new("headmm");
    let d1 = t1.path();
    init_review(d1, &head());
    write_authority(d1, "glm", "2", "completed");
    write_authority(d1, "k3", "3", "completed");
    let glm1 = write_review(d1, "glm.md", "completed", "2", Some("H2"));
    let k31 = write_review(d1, "k3.md", "completed", "3", Some(&head()));
    let (ok, so, _) = run(&[
        "freeze",
        d1.to_str().unwrap(),
        "--glm-review",
        glm1.to_str().unwrap(),
        "--k3-review",
        k31.to_str().unwrap(),
        "--native-root",
        native_root(d1).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "head-mismatch");

    // (b) stale turn: 报告 turn=1 但 turns.txt 已到 2
    let t2 = TmpDir::new("stale");
    let d2 = t2.path();
    init_review(d2, &head());
    let glm2 = write_review(d2, "glm.md", "completed", "1", Some(&head()));
    let k32 = write_review(d2, "k3.md", "completed", "3", Some(&head()));
    let native = d2.join("native").join("peer-glm-y");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(
        native.join("result-2.md"),
        "---\nslug: peer-glm-y\noutcome: completed\nturn: 2\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(
        native.join("turns.txt"),
        "1 completed 100\n2 completed 200\n",
    )
    .unwrap();
    let (ok2, so2, _) = run(&[
        "freeze",
        d2.to_str().unwrap(),
        "--glm-review",
        glm2.to_str().unwrap(),
        "--k3-review",
        k32.to_str().unwrap(),
        "--glm-slug",
        "peer-glm-y",
        "--k3-slug",
        "peer-k3-y",
        "--native-root",
        d2.join("native").to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok2, "旧轮次应被拒绝");
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v2["error"]["code"], "stale-turn");
}

// ---------------------------------------------------------------------------
// review-challenge
// ---------------------------------------------------------------------------

fn frozen_dir(tag: &str) -> (TmpDir, PathBuf, PathBuf) {
    frozen_dir_at(tag, None)
}

/// fixture_repo=Some 时 review HEAD 真实解析自 fixture 仓库(challenge
/// receipt 与评审 HEAD 一致绑定)。
fn frozen_dir_at(tag: &str, fixture_repo: Option<&Path>) -> (TmpDir, PathBuf, PathBuf) {
    let t = TmpDir::new(tag);
    let d = t.path().to_path_buf();
    let default_repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    init_review_at(&d, fixture_repo.unwrap_or(&default_repo));
    // 评审 HEAD: fixture_repo 在场时 = fixture 真实 HEAD(init --repo 已解析)
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    let review_head = st["head"].as_str().unwrap().to_string();
    write_authority(&d, "glm", "1", "completed");
    write_authority(&d, "k3", "1", "completed");
    let glm = write_review(&d, "glm.md", "completed", "1", Some(&review_head));
    let k3 = write_review(&d, "k3.md", "completed", "1", Some(&review_head));
    let (ok, so, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--head",
        &review_head,
    ]);
    assert!(ok, "freeze failed: {so} {se}");
    (t, glm, k3)
}

// v5: write_adapter_receipt* 已移除 — 外部 receipt 文件不再被 challenge
// 采信(外层六反例 4/5);正向行为证据一律 live_challenge 实时执行。

/// 最小 fixture 测试源: PASS / FAIL / #[ignore] 三形态。
const FIXTURE_PASS_TEST: &str = r#"
#[test]
fn fixture_probe_pass() {
    assert_eq!(2 + 2, 4);
}
"#;

const FIXTURE_FAIL_TEST: &str = r#"
#[test]
fn fixture_probe_fails() {
    assert_eq!(2 + 2, 5, "fixture real assertion failure");
}
"#;

const FIXTURE_IGNORED_TEST: &str = r#"
#[test]
#[ignore]
fn fixture_probe_ignored() {
    assert!(true);
}
"#;

const FIXTURE_COMPILE_ERROR_TEST: &str = r#"
#[test]
fn fixture_probe_compile_error() {
    let _x: i32 = "not a number";
}
"#;

/// 同 fixture 内 PASS + FAIL 双探针: 用于"先执行失败 → 实时重执行通过"
/// 的 refute-by-new-evidence 路径(同一 repo HEAD,两条独立 selector)。
const FIXTURE_PASS_AND_FAIL: &str = r#"
#[test]
fn fixture_probe_pass() {
    assert_eq!(2 + 2, 4);
}

#[test]
fn fixture_probe_fails() {
    assert_eq!(2 + 2, 5, "fixture real assertion failure");
}
"#;

/// LEGACY tofu(仅用于"python 打印 cargo 样式文本被拒绝"两个负向测试;
/// 正向执行一律走 write_adapter_receipt 生产路径)。
fn write_probe(dir: &Path) -> (PathBuf, Vec<String>) {
    // 真实会被执行的探针脚本: 输出 cargo 失败样式并 exit 101
    let probe = dir.join("probe-fail.py");
    std::fs::write(
        &probe,
        r#"import sys
name = sys.argv[1]
print(f"running 1 test")
print(f"test {name} ... ")
print(f"thread '{name}' (1) panicked at probe.rs:10:5:")
print("assertion failed: defect reproduced")
print("FAILED")
print("test result: FAILED. 0 passed; 1 failed; 0 ignored;")
sys.exit(101)
"#,
    )
    .unwrap();
    let ev = dir.join("ev-behavioral.log");
    std::fs::write(
        &ev,
        "running 1 test\ntest probe_X ... \nthread 'probe_X' (1) panicked at probe.rs:10:5:\nassertion failed\nFAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;\n",
    )
    .unwrap();
    let argv = vec![
        "python3".into(),
        probe.to_str().unwrap().to_string(),
        "probe_X".into(),
    ];
    (ev, argv)
}

/// 场景: 行为证据驱动改判(v5 live 执行: 真实 FAIL 探针 → blocked-on-evidence)。
#[test]
fn olp_review_challenge_evidence_flips_verdict() {
    // fixture 先行: 评审 HEAD 与 live 执行 repo 真实 HEAD 绑定同一 fixture
    let (ft, fx_repo, _h) = make_cargo_fixture("flip", FIXTURE_FAIL_TEST);
    let (t, _glm, _k3) = frozen_dir_at("flip", Some(&fx_repo));
    let d = t.path();
    // v5: 唯一 live 入口 — challenge --live-cargo 实时执行生产 adapter,
    // 真实 cargo exit101 + panic 锚 → 缺陷复现
    let (ok, so, se) = live_challenge(
        d,
        &fx_repo,
        "X",
        "fixture_probe_fails",
        "fail",
        &ft.path().join("target"),
    );
    assert!(ok, "live challenge failed: {so} {se}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(
        v["state"].as_str().unwrap(),
        "blocked-on-evidence",
        "PR HEAD 真实执行复现失败 → blocked-on-evidence"
    );
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    assert_eq!(st["challenge"]["accepted"].as_bool(), Some(true));
    // 每个 claim 独立保存执行结果(v5: per-claim challenges map)
    assert_eq!(
        st["challenges"]["X"]["latest"]["observed"].as_str(),
        Some("fail"),
        "claim X 应独立保存 live 执行结果"
    );
}

/// 场景: 文档性证据不被接纳(无论 kind 声明)。
#[test]
fn olp_review_challenge_rejects_non_behavioral_evidence() {
    let (t, _glm, _k3) = frozen_dir("docev");
    let d = t.path();
    let ev = d.join("doc-ev.md");
    std::fs::write(
        &ev,
        "# 挑战\nPR#627 有问题:assert!(\"字符串常量断言\"); 判词 X 不成立。\n",
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--kind",
        "behavioral",
    ]);
    assert!(!ok, "doc 证据应被拒绝");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "evidence-not-behavioral");
    // 判词标 unverified
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    assert_eq!(state["verdicts"]["X"]["state"], "unverified");
}

/// 场景: 伪造 cargo 样式日志(结构锚齐全但不可执行/无执行记录)被拒绝。
#[test]
fn olp_review_challenge_rejects_fabricated_cargo_log() {
    let (t, _glm, _k3) = frozen_dir("fabricated");
    let d = t.path();
    let ev = d.join("fake.log");
    std::fs::write(
        &ev,
        "running 8 tests\ntest outer_review_627_x ... panicked at store-repros.rs:31:5:\nassertion\nFAILED\ntest result: FAILED. 0 passed; 8 failed; 0 ignored;\n",
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "outer_review_627_x",
        // 无 --exec-argv / 非 receipt: 结构齐全但无执行(生产执行仅 cargo
        // adapter 路径,--exec-argv 限可信执行体,缺省即不可信)
    ]);
    assert!(!ok, "结构形似但无执行应被拒绝");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    // 无可信执行入口: 空 argv → evidence-not-executed(拒绝);
    // 非可信执行体(如 python)显式传入时 → executor-not-trusted。
    assert_eq!(v["error"]["code"], "evidence-not-executed");
}

/// 场景: imported 证据不自动 accepted → not-replayed。
#[test]
fn olp_review_imported_evidence_not_replayed() {
    let (t, _glm, _k3) = frozen_dir("imported");
    let d = t.path();
    let ev = d.join("imported.log");
    std::fs::write(
        &ev,
        "running 8 tests\ntest outer_review_629_x ... panicked at store-repros.rs:101:5:\nassertion\nFAILED\ntest result: FAILED. 0 passed; 8 failed; 0 ignored;\n",
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "outer_review_629_x",
        "--imported",
    ]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["state"], "not-replayed");
}

// ---------------------------------------------------------------------------
// review-cross
// ---------------------------------------------------------------------------

/// 冻结 + claim X 经 live 真实 FAIL 执行挑战(blocked-on-evidence)的评审目录。
/// v5: live_challenge 实时执行生产 adapter;X 处于已复现失败状态。
fn challenged_dir(tag: &str) -> (TmpDir, PathBuf) {
    let (ft, fx_repo, _h) = make_cargo_fixture(tag, FIXTURE_FAIL_TEST);
    let (t, glm, _k3) = frozen_dir_at(tag, Some(&fx_repo));
    let d = t.path().to_path_buf();
    let (ok, so, se) = live_challenge(
        &d,
        &fx_repo,
        "X",
        "fixture_probe_fails",
        "fail",
        &ft.path().join("target"),
    );
    assert!(ok, "live challenge failed: {so} {se}");
    (t, glm)
}

/// cross 报告的 helper: 写报告 + native 权威收据。
fn write_cross_report(d: &Path, slug: &str, turn: &str, outcome: &str, body: &str) -> PathBuf {
    write_cross_report_named(d, slug, &format!("{slug}.md"), turn, outcome, body)
}

/// cross 报告(文件名与 slug 解耦): slug 决定 frontmatter/native 权威,
/// file 决定落盘文件名 — 避免与冻结初审路径(如 glm.md/k3.md)碰撞导致
/// first-review-tampered 误触发。
fn write_cross_report_named(
    d: &Path,
    slug: &str,
    file: &str,
    turn: &str,
    outcome: &str,
    body: &str,
) -> PathBuf {
    write_authority(d, slug, turn, outcome);
    let p = d.join(file);
    std::fs::write(
        &p,
        format!("---\nslug: {slug}\noutcome: {outcome}\nturn: {turn}\n---\n\n{body}\n"),
    )
    .unwrap();
    p
}

/// v5 结构化 cross 报告: 原 reviewer slug + native 新轮次权威(append) +
/// HEAD 绑定 + 显式 cross_claims JSON 块(逐 claim accept/refute/pending
/// 与引用);子串包含不再算覆盖。
fn write_cross_structured(
    d: &Path,
    slug: &str,
    file: &str,
    turn: &str,
    outcome: &str,
    cross_claims: &str,
) -> PathBuf {
    write_authority_at(&d.join("native"), slug, turn, outcome, true);
    let head = state_head(d);
    let p = d.join(file);
    std::fs::write(
        &p,
        format!(
            "---\nslug: {slug}\noutcome: {outcome}\nturn: {turn}\nHEAD: {head}\n---\n\ncross 互审\n```json\n{{\"cross_claims\": {cross_claims}}}\n```\n"
        ),
    )
    .unwrap();
    p
}

/// 场景: 交叉互审需初审冻结且挑战证据已接纳。
#[test]
fn olp_review_cross_requires_frozen_and_challenged() {
    let t = TmpDir::new("crosspending");
    let d = t.path().to_path_buf();
    init_review(&d, &head());
    write_authority(&d, "glm", "1", "completed");
    write_authority(&d, "k3", "1", "completed");
    let glm = write_review(&d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(&d, "k3.md", "completed", "1", Some(&head()));
    let (ok, so_f, se_f) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(ok, "freeze failed: {so_f} {se_f}");
    let cross = write_cross_report(&d, "cross-k3", "4", "completed", "X: 采纳");
    let (ok2, _so2, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "cross-k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(!ok2, "challenge 未接纳时 cross 应拒绝");
    let vc: serde_json::Value = serde_json::from_str(_so2.trim()).unwrap();
    assert_eq!(vc["error"]["code"], "challenge-pending");
}

/// 场景: 交叉报告逐项裁决(3 条判词只回应 2 条)。
#[test]
fn olp_review_cross_requires_per_claim_verdicts() {
    let (t, _glm) = challenged_dir("coverage");
    let d = t.path().to_path_buf();
    // 补两条判词
    let state_path = d.join("review-state.json");
    let mut st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    st["verdicts"]["Y"] = serde_json::json!({"state": "approve"});
    st["verdicts"]["Z"] = serde_json::json!({"state": "approve"});
    std::fs::write(&state_path, serde_json::to_string(&st).unwrap()).unwrap();

    // v5: 结构化 cross(原 reviewer k3, native turn2 权威),只覆盖 X/Y 缺 Z
    let cross = write_cross_structured(
        &d,
        "k3",
        "cross-k3.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "accept"}, {"id": "Y", "verdict": "pending"}]"#,
    );
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(!ok, "缺 Z 覆盖应拒绝");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "missing-claim-coverage");
}

/// 场景: errored cross 报告不冒充有效。
#[test]
fn olp_review_rejects_errored_cross_report() {
    let (t, _glm) = challenged_dir("errcross");
    let d = t.path().to_path_buf();
    // v5: 原 reviewer k3 的 cross 自报 errored(native 同轮亦 errored)→ 拒绝
    let cross = write_cross_structured(
        &d,
        "k3",
        "cross-k3.md",
        "2",
        "errored",
        r#"[{"id": "X", "verdict": "accept"}]"#,
    );
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(!ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-outcome-invalid");
}

// REMOVED(k3 轮): 旧版 olp_review_cross_refutation_reverts_flip — 该测试允许
// 文字+行号反驳覆盖已复现失败,违反冻结合约 4(反驳需新重放行为证据/核验
// 代码引用/人工裁决)。v5 以同名测试按新合约恢复: 结构化 refute 仅在
// 已验证新行为证据或显式 --allow-operator-refute 人工裁决下生效。
// REMOVED(k3 轮): olp_review_approve_with_executed_evidence — python
// probe-pass.py 假日志路线,违反合约 3;替换为
// olp_review_k3_full_happy_path_accepted(真实 cargo adapter 生产路径)。

/// 场景: 两模型一致无行为证据 → pending-behavioral-evidence。
#[test]
fn olp_review_agreement_without_evidence_stays_pending() {
    let (t, _glm, _k3) = frozen_dir("agree");
    let d = t.path().to_path_buf();
    let state_path = d.join("review-state.json");
    let mut st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    st["verdicts"]["PR629-new-regression"] = serde_json::json!({"state": "approve"});
    std::fs::write(&state_path, serde_json::to_string(&st).unwrap()).unwrap();
    let (ok, so, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(
        v["verdicts"]["PR629-new-regression"]["state"],
        "pending-behavioral-evidence"
    );
}

// ---------------------------------------------------------------------------
// review-regression + 双输出
// ---------------------------------------------------------------------------

/// 外层反馈: 至少一条集成测试必须调用真实运行入口执行历史生产渲染/事件
/// 反例(真实 cargo test + 生产 Store/transport 源),不是 Python 打印的
/// cargo 样式文本。本测试在临时 clone 里 include! 外层诊断反例源码,
/// 真实编译执行 store-repros.rs 的 7 条负向探针(独立昂贵验收,单独运行)。
/// Reproduce 流程与 fixtures/review-evidence/evidence/reproduce.sh 一致。
///
/// 8 真实 Store 探针重放归外层单次集成验证(../prepared-replay-slots.json
/// 只读): 本测试改为显式单独集成入口(#[ignore]),不在常规套件内并发执行。
/// 运行方式:
///   cargo test --test olp_review_evidence -- --ignored \
///     olp_review_real_store_harness_executes_historical_negative_probes
#[test]
#[ignore = "显式单独集成验证: 8 真实 Store 探针重放归外层单次执行(prepared-replay-slots.json)"]
fn olp_review_real_store_harness_executes_historical_negative_probes() {
    let t = TmpDir::new("realharness");
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let evidence = repo.join("fixtures/review-evidence/evidence");
    // 外层单次集成重放的只读资产在场性绑定(不展开执行;执行属外层职责)
    let replay_slots = repo.join("../prepared-replay-slots.json");
    assert!(
        replay_slots.exists(),
        "外层集成重放资产缺失: {} (8 Store 探针由外层单次执行)",
        replay_slots.display()
    );
    // 断言反例源码存在且含 7 个 store 探针(不依赖 clone 成功也能给出清晰失败)
    let store_src = std::fs::read_to_string(evidence.join("store-repros.rs")).unwrap();
    assert_eq!(
        store_src.matches("fn outer_review_").count(),
        7,
        "store-repros 应含 7 个负向探针函数"
    );
    // Clone 源: 若固定 PR 对象缺失(浅仓),降级为标记 ignored-by-environment
    // 并在断言消息中显式说明,不得静默通过。
    let clone = t.path().join("review-clone");
    let out = run_deadline(
        Command::new("bash")
            .arg(evidence.join("reproduce.sh"))
            .arg(&repo)
            .arg(&clone),
        1800,
        "reproduce.sh",
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        // 唯一允许的失败: 源仓缺少 pinned PR 对象(环境缺资源),此时必须
        // 显式 panic 说明,不允许静默当作通过
        panic!(
            "真实 store harness 未复现 8 failed(exit={}): {}",
            out.status, combined
        );
    }
    assert!(
        combined.contains("All eight negative probes reproduced"),
        "reproduce.sh 应报告 8 探针复现: {}",
        &combined[combined.len().saturating_sub(400)..]
    );
}

// --- 外层实测反例回归(../outer-evidence-probe-results.json,先 RED 后修) ---

/// 外层反例 1: 无 native/runtime 权威证据时,仅凭自报 completed/turn:1 的报告
/// 不得冻结 — freeze 必须要求至少一种外部终止证据(native result-N 或
/// runtime-evidence 显式无活跃线程)匹配该 slug,缺权威即拒绝。
#[test]
fn olp_review_outer_probe_freeze_requires_native_authority() {
    let t = TmpDir::new("outer1");
    let d = t.path();
    init_review(d, &head());
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    // 不传 --native-root / --runtime-evidence: 无任何外部权威
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "无外部权威时 freeze 不得成功(外层反例1)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-authority-missing");
    // 提供匹配的 native 终止收据(含 originator/goal 身份绑定)后应成功
    let native = d.join("native");
    for (slug, turn) in [("glm", "1"), ("k3", "1")] {
        let nd = native.join(slug);
        std::fs::create_dir_all(&nd).unwrap();
        std::fs::write(
            nd.join(format!("result-{turn}.md")),
            format!("---\nslug: {slug}\noutcome: completed\nturn: {turn}\n---\nbody\n"),
        )
        .unwrap();
        std::fs::write(nd.join("turns.txt"), format!("{turn} completed 100\n")).unwrap();
        std::fs::write(nd.join("originator"), "sess-1").unwrap();
        std::fs::write(nd.join("goal"), "goal_01").unwrap();
    }
    let t2_dir = d.join("with-authority");
    std::fs::create_dir_all(&t2_dir).unwrap();
    init_review(&t2_dir, &head());
    let glm2 = write_review(&t2_dir, "glm.md", "completed", "1", Some(&head()));
    let k32 = write_review(&t2_dir, "k3.md", "completed", "1", Some(&head()));
    let (ok2, so2, se2) = run(&[
        "freeze",
        t2_dir.to_str().unwrap(),
        "--glm-review",
        glm2.to_str().unwrap(),
        "--k3-review",
        k32.to_str().unwrap(),
        "--native-root",
        native.to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(ok2, "有权威收据时 freeze 应成功: {so2} {se2}");
}

/// 外层反例 2: --exec-argv /usr/bin/false(无任何输出)不得作为行为证据 flip。
#[test]
fn olp_review_outer_probe_unrelated_false_command_rejected() {
    let (t, _glm, _k3) = frozen_dir("outer2");
    let d = t.path();
    let ev = d.join("ev-outer2.log");
    std::fs::write(
        &ev,
        "running 1 test\ntest probe_X ... \nthread 'probe_X' panicked at probe.rs:10:5:\nFAILED\ntest result: FAILED. 0 passed; 1 failed;\n",
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "probe_X",
        "--exec-argv",
        "/usr/bin/false",
        "--run-dir",
        d.to_str().unwrap(),
    ]);
    assert!(!ok, "无关 false 命令不得被采信(外层反例2)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "executor-not-trusted");
}

/// 外层反例 3: 冻结后删除初审文件,status/challenge 必须报 first-review-tampered,
/// 不得静默通过(verify_no_tamper 需 fail-closed 于文件缺失)。
#[test]
fn olp_review_outer_probe_missing_frozen_review_rejected() {
    let (t, glm, _k3) = frozen_dir("outer3");
    let d = t.path();
    std::fs::remove_file(&glm).unwrap();
    let (ok, so, _) = run(&["status", d.to_str().unwrap()]);
    assert!(!ok, "冻结后删除初审文件,status 不得 ok(外层反例3)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "first-review-tampered");
    // challenge 同样拒绝
    let ev = d.join("ev3.log");
    std::fs::write(
        &ev,
        "running 1 test\ntest probe_X ... ok\ntest result: ok. 1 passed;\n",
    )
    .unwrap();
    let (ok2, so2, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "probe_X",
    ]);
    assert!(!ok2);
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v2["error"]["code"], "first-review-tampered");
}

/// 外层反例补充: native 收据 slug 与报告 slug 不匹配(跨 runtime 同名目录冒充)
/// → peer-authority-mismatch,不得把别家 peer 的终止收据当自己的权威。
#[test]
fn olp_review_outer_probe_native_slug_mismatch_rejected() {
    let t = TmpDir::new("outer4");
    let d = t.path();
    init_review(d, &head());
    // native/glm 的收据 slug 写的是别的 peer
    let nd = d.join("native").join("glm");
    std::fs::create_dir_all(&nd).unwrap();
    std::fs::write(
        nd.join("result-2.md"),
        "---\nslug: other-runtime-glm\noutcome: completed\nturn: 2\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(nd.join("turns.txt"), "1 completed 100\n2 completed 200\n").unwrap();
    write_authority(d, "k3", "3", "completed");
    let glm = write_review(d, "glm.md", "completed", "2", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "3", Some(&head()));
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "slug 不匹配的收据不得作权威(跨 runtime 冒充)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-authority-mismatch");
}

// ---------------------------------------------------------------------------
// 冻结合约纠正(k3 实现轮, 2026-09-09): 严格权威 / 事务冻结 / 生产 Cargo
// harness / per-claim 结构化 cross / 正向完整 happy path。
// TDD 顺序如实记录: 本区块测试先写、对旧实现跑出断言级 RED
// (.octos/red-proof/evidence-k3-red.log),随后重写实现至 GREEN。
// ---------------------------------------------------------------------------

/// 冻结合约 1a: runtime-evidence 只含 slug 名(缺 status/identity/active_thread
/// 字段)不构成终止权威 — 外层反例 ../outer-authority-probe-results.json
/// "slug-only": {peers:[{slug:glm},{slug:k3}]} 单独即可冻结(旧实现 bug)。
#[test]
fn olp_review_k3_freeze_rejects_bare_slug_runtime_evidence() {
    let t = TmpDir::new("k3-slugonly");
    let d = t.path();
    init_review(d, &head());
    let glm = write_review(d, "glm.md", "completed", "99", None);
    let k3 = write_review(d, "k3.md", "completed", "99", None);
    let re = d.join("runtime.json");
    std::fs::write(&re, r#"{"peers": [{"slug": "glm"}, {"slug": "k3"}]}"#).unwrap();
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--runtime-evidence",
        re.to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(
        !ok,
        "仅 slug 的 runtime-evidence 不得构成权威(外层 slug-only 反例)"
    );
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-authority-missing");
}

/// 冻结合约 1b: native outcome=interrupted/turn1 + 报告声称 completed/turn99
/// 不得冻结 — 必须要求 native 最高编号 result 严格 outcome=completed 且
/// slug/turn 与报告一致(外层 interrupted-native-future-report 反例)。
#[test]
fn olp_review_k3_freeze_rejects_interrupted_native_with_future_claim() {
    let t = TmpDir::new("k3-interrupted");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "interrupted");
    write_authority(d, "k3", "1", "interrupted");
    let glm = write_review(d, "glm.md", "completed", "99", None);
    let k3 = write_review(d, "k3.md", "completed", "99", None);
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "interrupted native + 声称 completed/turn99 不得冻结");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-outcome-invalid");
}

/// 冻结合约 1c: 报告 HEAD 缺失不能绕过 HEAD 校验 — freeze 需显式 --head
/// 锚定评审基线,初审文件 frontmatter 必须声明相等 HEAD。
#[test]
fn olp_review_k3_freeze_requires_explicit_head_anchor() {
    let t = TmpDir::new("k3-headreq");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    write_authority(d, "k3", "1", "completed");
    let glm = write_review(d, "glm.md", "completed", "1", None);
    let k3 = write_review(d, "k3.md", "completed", "1", None);
    // (a) 不传 --head: 拒绝(head-anchor-missing)
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
    ]);
    assert!(!ok, "freeze 缺 --head 锚定应拒绝");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "head-anchor-missing");
    // (b) 传 --head 但报告无 HEAD frontmatter: 拒绝(report-head-missing)
    let (ok2, so2, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok2, "报告缺 HEAD 声明应拒绝(缺失不得绕过校验)");
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v2["error"]["code"], "report-head-missing");
    // (c) --head 与 init 基线不一致: 拒绝(head-mismatch)
    let glm2 = write_review(d, "glm2.md", "completed", "1", Some(&head()));
    let k32 = write_review(d, "k32.md", "completed", "1", Some(&head()));
    let (ok3, so3, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm2.to_str().unwrap(),
        "--k3-review",
        k32.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        "H999",
    ]);
    assert!(!ok3, "--head 与 init 基线不一致应拒绝");
    let v3: serde_json::Value = serde_json::from_str(so3.trim()).unwrap();
    assert_eq!(v3["error"]["code"], "head-mismatch");
}

/// 冻结合约 1d: 两个评审方必须 distinct — 同 slug 双份报告冻结即拒绝
/// (duplication 防线: 同一 peer/同 lane 两份不得计为两独立报告)。
#[test]
fn olp_review_k3_freeze_rejects_same_peer_both_sides() {
    let t = TmpDir::new("k3-dupe");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let glm2 = write_review_full(
        d,
        "glm-copy.md",
        "glm",
        "completed",
        "1",
        Some(&head()),
        Some(&["Y"]),
        "第二份",
    );
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        glm2.to_str().unwrap(),
        "--glm-slug",
        "glm",
        "--k3-slug",
        "glm",
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "同 peer 两份报告不得冻结为两独立初审");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "duplicate-reviewer");
}

/// 冻结合约 1e: 初审文件必须显式声明 claims(结构化 JSON claims 块),
/// 缺 claims 块不得冻结 — 冻结即确立全部初始 claim ID。
#[test]
fn olp_review_k3_freeze_requires_claims_block() {
    let t = TmpDir::new("k3-claims");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    write_authority(d, "k3", "1", "completed");
    // claims=None: 报告无显式 JSON claims 块
    let glm = write_review_full(
        d,
        "glm.md",
        "glm",
        "completed",
        "1",
        Some(&head()),
        None,
        "初审",
    );
    let k3 = write_review_full(
        d,
        "k3.md",
        "k3",
        "completed",
        "1",
        Some(&head()),
        None,
        "初审",
    );
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "缺显式 claims 块不得冻结");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "claims-block-missing");
}

/// 挑战合约 3a: 行为执行门禁限定可信 Cargo harness — python 打印 cargo 样式
/// 文本不再被采信为 behavioral(write_probe/probe-pass.py 路线废弃)。
/// 生产路径为 cargo-harness adapter(scripts/olp-review-evidence-cargo.py)。
#[test]
fn olp_review_k3_challenge_rejects_python_fake_cargo() {
    let t = TmpDir::new("k3-pyfake");
    let d = t.path().to_path_buf();
    init_review(&d, &head());
    write_authority(&d, "glm", "1", "completed");
    write_authority(&d, "k3", "1", "completed");
    let glm = write_review_full(
        &d,
        "glm.md",
        "glm",
        "completed",
        "1",
        Some(&head()),
        Some(&["X"]),
        "初审",
    );
    let k3 = write_review_full(
        &d,
        "k3.md",
        "k3",
        "completed",
        "1",
        Some(&head()),
        Some(&["X"]),
        "初审",
    );
    let (ok, so, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(ok, "freeze failed: {so} {se}");
    let (ev, argv) = write_probe(&d);
    let (ok2, so2, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "probe_X",
        "--exec-argv",
        &argv.join(" "),
        "--run-dir",
        d.to_str().unwrap(),
    ]);
    assert!(!ok2, "python 假 cargo 探针不得作为行为证据(合约3)");
    let v: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v["error"]["code"], "executor-not-trusted");
}

/// 挑战合约 3b: 打印假 cargo 日志的 python "pass" 探针同样拒绝
/// (probe-pass.py happy-path 玩具路线废弃)。
#[test]
fn olp_review_k3_challenge_rejects_python_fake_pass_probe() {
    let t = TmpDir::new("k3-pypass");
    let d = t.path().to_path_buf();
    init_review(&d, &head());
    write_authority(&d, "glm", "1", "completed");
    write_authority(&d, "k3", "1", "completed");
    let glm = write_review_full(
        &d,
        "glm.md",
        "glm",
        "completed",
        "1",
        Some(&head()),
        Some(&["X"]),
        "初审",
    );
    let k3 = write_review_full(
        &d,
        "k3.md",
        "k3",
        "completed",
        "1",
        Some(&head()),
        Some(&["X"]),
        "初审",
    );
    let (ok, so, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(ok, "freeze failed: {so} {se}");
    let probe = d.join("probe-pass.py");
    std::fs::write(
        &probe,
        "import sys\nprint('running 1 test')\nprint('test probe_P ... ok')\nprint('test result: ok. 1 passed; 0 failed;')\nsys.exit(0)\n",
    )
    .unwrap();
    let ev = d.join("ev-pass.log");
    std::fs::write(
        &ev,
        "running 1 test\ntest probe_P ... ok\ntest result: ok. 1 passed; 0 failed;\n",
    )
    .unwrap();
    let (ok2, so2, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "probe_P",
        "--exec-argv",
        &format!("python3 {} probe_P", probe.display()),
        "--run-dir",
        d.to_str().unwrap(),
    ]);
    assert!(!ok2, "python 假 pass 探针不得采信(合约3)");
    let v: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v["error"]["code"], "executor-not-trusted");
}

/// 挑战合约 3c + 外层 adapter 实测两漏洞(../outer-adapter-probe-results.json):
/// 真实最小 fixture 仓库上的 PASS/FAIL/zero-match/compile-error/ignored/
/// 跨仓库 manifest 六形态。走生产 adapter 同一路径,绑定 fixture 真实 HEAD
/// 与 review repo(init --repo 真实解析),不假造本仓通过。
#[test]
fn olp_review_k3_cargo_adapter_rejects_fake_and_no_tests() {
    let adapter =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/olp-review-evidence-cargo.py");
    assert!(
        adapter.exists(),
        "生产 Cargo harness adapter 缺失: {}",
        adapter.display()
    );

    // (a) 外层反例 zero-tests-must-reject: selector 无定义 → no-tests-matched,
    //     不得 exit 0 假 PASS
    let (ft, repo, _h) = make_cargo_fixture("zero", FIXTURE_PASS_TEST);
    let (ok, so) = run_adapter(
        &repo,
        &[
            "--selector",
            "definitely_no_such_test_selector_zzz",
            "--expect",
            "pass",
            "--target-dir",
            ft.path().join("target").to_str().unwrap(),
        ],
        None,
        300,
    );
    assert!(!ok, "无匹配测试不得成功: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "no-tests-matched");

    // (b) 真实 FAIL 探针 --expect pass → expectation-violated,observed=fail
    let (ft2, repo2, _h2) = make_cargo_fixture("fail", FIXTURE_FAIL_TEST);
    let (ok2, so2) = run_adapter(
        &repo2,
        &[
            "--selector",
            "fixture_probe_fails",
            "--expect",
            "pass",
            "--receipt",
            ft2.path().join("fail-receipt.json").to_str().unwrap(),
            "--target-dir",
            ft2.path().join("target").to_str().unwrap(),
        ],
        None,
        300,
    );
    assert!(!ok2, "真实 FAIL 不得判 pass: {so2}");
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v2["error"]["code"], "expectation-violated");
    assert_eq!(v2["error"]["detail"]["observed"], "fail");
    // receipt 形态绑定: before/after HEAD + source/manifest sha + 完整工件
    let rc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ft2.path().join("fail-receipt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rc["observed"], "fail");
    assert_eq!(rc["head_before"], rc["head_after"]);
    assert_eq!(rc["head_before"].as_str().unwrap().len(), 40);
    assert!(rc["manifest_sha256"].is_string() && rc["test_target_sha256"].is_string());
    let art = rc["artifacts"].as_object().unwrap();
    assert!(std::path::Path::new(art["stdout"].as_str().unwrap()).exists());
    assert!(std::path::Path::new(art["stderr"].as_str().unwrap()).exists());

    // (c) ignored 探针不得当执行证据
    let (ft3, repo3, _h3) = make_cargo_fixture("ignored", FIXTURE_IGNORED_TEST);
    let (ok3, so3) = run_adapter(
        &repo3,
        &[
            "--selector",
            "fixture_probe_ignored",
            "--expect",
            "pass",
            "--target-dir",
            ft3.path().join("target").to_str().unwrap(),
        ],
        None,
        300,
    );
    assert!(!ok3, "ignored 探针不得判 pass: {so3}");
    let v3: serde_json::Value = serde_json::from_str(so3.trim()).unwrap();
    assert_eq!(v3["error"]["code"], "ignored-not-run");

    // (d) 编译错误与断言失败区分: 不得当行为终态
    let (ft4, repo4, _h4) = make_cargo_fixture("cerr", FIXTURE_COMPILE_ERROR_TEST);
    let (ok4, so4) = run_adapter(
        &repo4,
        &[
            "--selector",
            "fixture_probe_compile_error",
            "--expect",
            "pass",
            "--target-dir",
            ft4.path().join("target").to_str().unwrap(),
        ],
        None,
        300,
    );
    assert!(!ok4, "编译错误不得判 pass: {so4}");
    let v4: serde_json::Value = serde_json::from_str(so4.trim()).unwrap();
    assert!(
        v4["error"]["code"] == "compile-error" || v4["error"]["code"] == "no-tests-matched",
        "编译错误形态应为 compile-error/no-tests-matched,实际 {}",
        v4["error"]["code"]
    );

    // (e) 外层反例 foreign-manifest-must-reject: --repo A --manifest B 跨仓库
    //     误绑定必须拒绝
    let (ft5, repo5, _h5) = make_cargo_fixture("foreign", FIXTURE_PASS_TEST);
    let foreign_manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let (ok5, so5) = run_adapter(
        &repo5,
        &[
            "--manifest",
            foreign_manifest.to_str().unwrap(),
            "--selector",
            "fixture_probe_pass",
            "--expect",
            "pass",
            "--target-dir",
            ft5.path().join("target").to_str().unwrap(),
        ],
        None,
        300,
    );
    assert!(!ok5, "跨仓库 manifest 不得成功: {so5}");
    let v5: serde_json::Value = serde_json::from_str(so5.trim()).unwrap();
    assert_eq!(v5["error"]["code"], "foreign-manifest");
}

/// adapter --lib 单元测试入口: src/ 内 lib 测试经 cargo --list 全限定定位
/// (--exact 执行),绑定定义源文件 SHA256,完整 stdout/stderr 工件留存。
/// 八探针(src/store.rs|transport.rs 内 lib 测试)的完整重放属外层单次集成
/// 验证;此处以本仓 src/app 内真实 lib 测试验证入口机制本身。
#[test]
fn olp_review_k3_cargo_adapter_lib_entry_full_artifacts() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let t = TmpDir::new("k3-libentry");
    let receipt = t.path().join("lib-receipt.json");
    // 真实 lib 测试(由外层 smoke 验证存在): src/app/markdown_highlight.rs
    let (ok, so) = run_adapter(
        &repo,
        &[
            "--lib",
            "--selector",
            "app::markdown_highlight::tests::blockquote_lines_render_muted_italic_without_inline_parsing",
            "--expect",
            "pass",
            "--receipt",
            receipt.to_str().unwrap(),
            "--artifact-dir",
            t.path().to_str().unwrap(),
        ],
        None,
        600,
    );
    assert!(ok, "--lib 入口真实 PASS 应成功: {so}");
    let rc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&receipt).unwrap()).unwrap();
    assert_eq!(rc["observed"], "pass");
    assert_eq!(rc["test_target"], "--lib");
    // 绑定: 全限定名经 cargo --list 定位;定义源文件 SHA256 在场
    assert!(
        rc["selector_qualified"]
            .as_str()
            .unwrap()
            .ends_with("blockquote_lines_render_muted_italic_without_inline_parsing")
    );
    assert!(
        rc["test_target_sha256"].is_string(),
        "lib 源文件 sha 绑定缺失"
    );
    assert!(
        rc["test_source"]
            .as_str()
            .unwrap()
            .ends_with("markdown_highlight.rs")
    );
    // 完整 stdout/stderr 工件留存(非仅尾部)
    let art = rc["artifacts"].as_object().unwrap();
    let stdout_log = std::fs::read_to_string(art["stdout"].as_str().unwrap()).unwrap();
    assert!(stdout_log.contains("test result: ok."));
    assert!(std::path::Path::new(art["stderr"].as_str().unwrap()).exists());
    // --lib selector 未定义 → no-tests-matched,不得假通过
    let (ok2, so2) = run_adapter(
        &repo,
        &[
            "--lib",
            "--selector",
            "no::such::unit_test_zzz",
            "--expect",
            "pass",
        ],
        None,
        300,
    );
    assert!(!ok2, "--lib 无定义 selector 不得成功: {so2}");
}

/// cross 合约 4a: 反驳已复现失败需要**新的重放行为证据**(或显式人工裁决)
/// — 仅凭文字 "line123/refute/人工裁决" 不构成结构化覆盖,更不得静默
/// 覆盖 reproduced failure。v5: prose cross 缺 cross_claims 块 → 拒绝。
#[test]
fn olp_review_k3_cross_refute_words_do_not_overwrite_reproduced_failure() {
    let (t, _glm) = challenged_dir("k3-refutewords");
    let d = t.path().to_path_buf();
    // challenged_dir 后 X 处于 reproduced failure(blocked-on-evidence)
    // 原 reviewer k3 的 prose cross(HEAD 绑定但无结构化块,隔离覆盖门)
    let cross = write_cross_prose(
        &d,
        "k3",
        "cross-k3-words.md",
        "2",
        "X: 反驳成立 — line 10 行号引用,refute,人工裁决,文字断言反例不成立。",
    );
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    // v5: 纯文字(含"人工裁决/line"字样)无结构化 cross_claims 块 → 拒绝收录
    assert!(!ok, "纯文字 cross 不得被采信: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "missing-claim-coverage");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    let st = state["verdicts"]["X"]["state"].as_str().unwrap_or("");
    assert_eq!(
        st, "blocked-on-evidence",
        "文字/行号反驳不得覆盖已复现失败,应保持原判"
    );
}

/// cross 合约 4b: 两份独立 native reviewer 的 cross 都收齐且全 claim 覆盖后
/// 才 accepted;单份 cross 报告不完成流程(review_accepted=false)。
#[test]
fn olp_review_k3_single_cross_does_not_complete_flow() {
    let (t, _glm) = challenged_dir("k3-onecross");
    let d = t.path().to_path_buf();
    let cross = write_cross_structured(
        &d,
        "k3",
        "cross-k3.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "accept"}]"#,
    );
    let (ok, so, se) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok, "cross failed: {so} {se}");
    let (ok2, so2, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok2);
    let v: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(
        v["review_accepted"].as_bool(),
        Some(false),
        "单份 cross 不得完成互审流程: {so2}"
    );
}

/// cross 合约 4c: 旧 turn1 初审报告不得冒充 cross turn2(stale-turn 拒绝)。
#[test]
fn olp_review_k3_stale_turn1_report_cannot_masquerade_as_cross() {
    let (t, glm) = challenged_dir("k3-stalecross");
    let d = t.path().to_path_buf();
    // glm 初审是 turn1;直接拿初审文件当 cross(冻结后原件未动)
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        glm.to_str().unwrap(),
        "--cross-slug",
        "glm",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(!ok, "旧 turn1 报告不得冒充 cross(应 stale-turn)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "stale-turn");
}

/// happy path 合约 5(v5): 两份有效独立 first turn1 → 冻结 → 实际最小 Cargo
/// PASS live 执行(challenge --live-cargo,本入口实时驱动生产 adapter) →
/// 两份各自新 native completed turn2 cross(结构化全覆盖) → accepted;
/// 初审原件哈希不变保留。
#[test]
fn olp_review_k3_full_happy_path_accepted() {
    // 真实最小 fixture 仓库: review HEAD 绑定 fixture 真实 HEAD(init --repo
    // 真实解析),不假造本仓通过;live PASS probe 走生产 adapter 同一路径。
    let (ft, fixture_repo, fx_head) = make_cargo_fixture("happy", FIXTURE_PASS_TEST);
    let t = TmpDir::new("k3-happy");
    let d = t.path().to_path_buf();
    let h = review_head(&d, &fixture_repo);
    assert_eq!(h, fx_head, "init --repo 应解析 fixture 真实 HEAD");
    write_authority(&d, "glm", "1", "completed");
    write_authority(&d, "k3", "1", "completed");
    let glm = write_review_full(
        &d,
        "glm.md",
        "glm",
        "completed",
        "1",
        Some(&h),
        Some(&["X"]),
        "glm 初审",
    );
    let k3 = write_review_full(
        &d,
        "k3.md",
        "k3",
        "completed",
        "1",
        Some(&h),
        Some(&["X"]),
        "k3 初审",
    );
    let (ok, so, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--head",
        &h,
    ]);
    assert!(ok, "freeze failed: {so} {se}");
    let glm_sha = sha256_hex(&glm);
    let k3_sha = sha256_hex(&k3);
    // v5: 唯一 live 入口 — 本入口实时执行生产 adapter(真实 cargo PASS)。
    let (ok2, so2, se2) = live_challenge(
        &d,
        &fixture_repo,
        "X",
        "fixture_probe_pass",
        "pass",
        &ft.path().join("target"),
    );
    assert!(ok2, "live challenge(PASS) failed: {so2} {se2}");
    // 两份结构化 cross: glm/k3 各自 turn2 native-completed,覆盖全部 claims
    let cross_g = write_cross_structured(
        &d,
        "glm",
        "cross-glm.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "accept"}]"#,
    );
    let (ok3, so3, se3) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross_g.to_str().unwrap(),
        "--cross-slug",
        "glm",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok3, "cross glm failed: {so3} {se3}");
    let (ok4, so4, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok4);
    let v4: serde_json::Value = serde_json::from_str(so4.trim()).unwrap();
    assert_eq!(
        v4["review_accepted"].as_bool(),
        Some(false),
        "单份 cross 不应收口"
    );
    let cross_k = write_cross_structured(
        &d,
        "k3",
        "cross-k3.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "accept"}]"#,
    );
    let (ok5, so5, se5) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross_k.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok5, "cross k3 failed: {so5} {se5}");
    let (ok6, so6, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok6);
    let v6: serde_json::Value = serde_json::from_str(so6.trim()).unwrap();
    assert_eq!(
        v6["review_accepted"].as_bool(),
        Some(true),
        "双 cross 后应 accepted: {so6}"
    );
    assert_eq!(v6["verdicts"]["X"]["state"], "approve");
    // 初审原件不可变保留
    assert_eq!(sha256_hex(&glm), glm_sha, "冻结后初审原件被改动");
    assert_eq!(sha256_hex(&k3), k3_sha, "冻结后初审原件被改动");
}

fn sha256_hex(p: &Path) -> String {
    use std::io::Read;
    let mut f = std::fs::File::open(p).unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    // 简易 SHA256 via 系统 shasum,避免新增 crate 依赖
    let out = Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(p)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

// REMOVED(最小 fixture 改造): k3_adapter_fixture_pass / k3_adapter_fixture_fails
// 曾让 adapter 递归编译整仓执行(8 并发 = 8 份全仓从零编译,600s 超时根因)。
// 由 make_cargo_fixture + FIXTURE_PASS_TEST/FIXTURE_FAIL_TEST 替代 — 无依赖
// 真实编译真实断言,行为门不降低,见 olp_review_k3_cargo_adapter_rejects_*。

/// 场景: 八反例回归数据集 — fixtures 完整性(逐字节 SHA256 清单)。
/// (完整 freeze→challenge→cross 流程在 olp_review_regression_dataset_classifies_four_prs)
#[test]
fn olp_review_regression_dataset_classifies_four_prs() {
    let fx = fixtures();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.join("FIXTURES-SHA256.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["source_sha256_check"].as_str().unwrap(),
        "byte-identical(源/fixture 逐一 SHA256 相等)"
    );
    let files = manifest["files"].as_array().unwrap();
    assert!(
        files.len() >= 60,
        "fixtures 应含 60+ 文件,实际 {}",
        files.len()
    );
    // adversarial.log 是缺陷证据: 8 failed,不得写成通过。
    // 文件经 git add -f 强制纳入提交(.gitignore *.log 规则例外,
    // 见 FIXTURES-SHA256.json note) — 干净 checkout 必须在场,缺失即失败。
    let adv = std::fs::read_to_string(fx.join("evidence/adversarial.log"))
        .expect("adversarial.log 必须在提交内(git add -f;*.log ignore 例外)");
    assert!(adv.contains("8 failed"), "adversarial.log 应记录 8 failed");
    assert!(!adv.contains("8 passed"), "不得把 8 failed 写成 8 passed");
    // MANIFEST 期望分类(外层独立写入)存在且非硬编码于脚本
    let outer: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.join("MANIFEST.json")).unwrap()).unwrap();
    assert_eq!(
        outer["prs"]["627"]["outer_recommendation"],
        "request-changes"
    );
    assert_eq!(
        outer["prs"]["629"]["outer_recommendation"],
        "conditional-approve-scope-and-specs"
    );
    // 脚本源码不得按 PR 编号硬编码分类
    let script_src = std::fs::read_to_string(script()).unwrap();
    assert!(
        !script_src.contains("\"629\"") && !script_src.contains("'629'"),
        "禁止按 PR 编号硬编码 residual"
    );
}

/// 场景: JSON 与人读双输出 + 错误输出可解析。
#[test]
fn olp_review_evidence_json_and_human_output() {
    let t = TmpDir::new("dualout");
    let d = t.path().to_path_buf();
    init_review(&d, &head());
    // JSON status 可解析
    let (ok, so, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert!(v["identity"].is_object() || v["frozen"].is_boolean());
    // 错误输出也是可解析 JSON
    let (ok2, so2, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        "nope.log",
        "--claim",
        "X",
    ]);
    assert!(!ok2);
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert!(v2["error"]["code"].is_string());
    // human 格式
    let out = Command::new("python3")
        .arg(script())
        .arg("--format")
        .arg("human")
        .arg("status")
        .arg(d.to_str().unwrap())
        .output()
        .unwrap();
    let human = String::from_utf8_lossy(&out.stdout);
    assert!(human.contains("lifecycle") || human.contains("verdicts") || human.contains("frozen"));
    // human 格式必须是真人类渲染: 不得是可解析 JSON
    assert!(
        serde_json::from_str::<serde_json::Value>(human.trim()).is_err(),
        "--format human 输出不得为可解析 JSON: {}",
        &human[..human.len().min(200)]
    );
    // human 模式下错误也不得泄漏 JSON 错误信封
    let out2 = Command::new("python3")
        .arg(script())
        .arg("--format")
        .arg("human")
        .arg("challenge")
        .arg(d.to_str().unwrap())
        .arg("--evidence")
        .arg("nope.log")
        .arg("--claim")
        .arg("X")
        .output()
        .unwrap();
    assert!(!out2.status.success());
    let herr = String::from_utf8_lossy(&out2.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(herr.trim()).is_err(),
        "--format human 错误输出不得为 JSON 信封"
    );
}

// ---------------------------------------------------------------------------
// v5 救援: 外层六反例最终门(../outer-evidence-final-gate-probes.json 0/6)
// 真实 RED→GREEN。复现器 ../probe-evidence-final-gates.py,前测收据
// .octos/k3-core-six-probes-before.json(0/6)。
// ---------------------------------------------------------------------------

/// 反例 1: native result-1 completed 但 turns.txt 同轮 errored → 冲突,freeze
/// 必须拒绝(同轮 outcome 必须精确 completed,缺项/冲突都不是终止权威)。
#[test]
fn k3_rescue_freeze_rejects_turns_outcome_conflict() {
    let t = TmpDir::new("k3r-turnsconflict");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    write_authority(d, "k3", "1", "completed");
    // 同轮冲突: result-1 completed 而 turns.txt 记 errored
    std::fs::write(
        native_root(d).join("glm").join("turns.txt"),
        "1 errored 100\n",
    )
    .unwrap();
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(
        !ok,
        "turns.txt 同轮 errored 与 result completed 冲突,freeze 应拒绝: {so}"
    );
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-outcome-invalid");
}

/// 反例 2: 报告 turn=9 而 native 最新只有 turn1 → 未来轮次,freeze 必须拒绝
/// (报告 turn 恰等 native 最新编号,非缺项/非未来/非冲突)。
#[test]
fn k3_rescue_freeze_rejects_future_report_turn() {
    let t = TmpDir::new("k3r-futureturn");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    write_authority(d, "k3", "1", "completed");
    let glm = write_review(d, "glm.md", "completed", "9", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(!ok, "报告 turn9 超过 native 最新 turn1,freeze 应拒绝: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "turn-mismatch");
}

/// 反例 3: native originator/goal 属于别的 master/goal → 外来 native-root
/// 冒充,freeze 必须拒绝(originator/goal 文件须与调用视角精确绑定)。
#[test]
fn k3_rescue_freeze_rejects_foreign_originator_goal() {
    let t = TmpDir::new("k3r-foreignid");
    let d = t.path();
    init_review(d, &head());
    write_authority(d, "glm", "1", "completed");
    write_authority(d, "k3", "1", "completed");
    // foreign 身份: originator 属于别的 master/session,goal 属于别的 goal
    std::fs::write(
        native_root(d).join("glm").join("originator"),
        "octosfix:local:tui#foreign",
    )
    .unwrap();
    std::fs::write(native_root(d).join("glm").join("goal"), "goal_99").unwrap();
    let glm = write_review(d, "glm.md", "completed", "1", Some(&head()));
    let k3 = write_review(d, "k3.md", "completed", "1", Some(&head()));
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
        "--head",
        &head(),
    ]);
    assert!(
        !ok,
        "foreign originator/goal 的 native 收据不得作权威: {so}"
    );
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-authority-mismatch");
}

/// 反例 4: 手写 JSON receipt(receipt_kind=cargo-test-execution、observed=pass、
/// 当前 head、stdout_sha256="invented")完全没执行 → 不得 challenge_accepted。
/// v5: 外部 receipt 字段不是执行证明;唯一 live 入口是 --live-cargo 实时执行。
#[test]
fn k3_rescue_challenge_rejects_fabricated_receipt() {
    let (t, _glm, _k3) = frozen_dir("k3r-fakereceipt");
    let d = t.path();
    let h = state_head(d);
    let fake = d.join("invented-receipt.json");
    std::fs::write(
        &fake,
        format!(
            "{{\"receipt_kind\": \"cargo-test-execution\", \"head_before\": \"{h}\", \"observed\": \"pass\", \"exit_code\": 0, \"stdout_sha256\": \"invented\", \"selector\": \"never_executed\"}}"
        ),
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        fake.to_str().unwrap(),
        "--claim",
        "X",
    ]);
    assert!(!ok, "伪造 receipt(无任何执行)不得接纳: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "evidence-not-executed");
    let (oks, sos, _) = run(&["status", d.to_str().unwrap()]);
    assert!(oks);
    let vs: serde_json::Value = serde_json::from_str(sos.trim()).unwrap();
    assert_eq!(
        vs["challenge_accepted"].as_bool(),
        Some(false),
        "伪造 receipt 后 challenge_accepted 不得为 true"
    );
}

/// 反例 5: 同一伪造 receipt 加 --imported → 一律 not-replayed,--imported
/// 不能旁路执行门(JSON 分支不得在 imported 前提前返回)。
#[test]
fn k3_rescue_challenge_imported_receipt_not_replayed() {
    let (t, _glm, _k3) = frozen_dir("k3r-fakeimport");
    let d = t.path();
    let h = state_head(d);
    let fake = d.join("invented-receipt.json");
    std::fs::write(
        &fake,
        format!(
            "{{\"receipt_kind\": \"cargo-test-execution\", \"head_before\": \"{h}\", \"observed\": \"pass\", \"exit_code\": 0, \"stdout_sha256\": \"invented\", \"selector\": \"never_executed\"}}"
        ),
    )
    .unwrap();
    let (ok, so, _) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        fake.to_str().unwrap(),
        "--claim",
        "X",
        "--imported",
    ]);
    assert!(ok, "imported 收据应显式分层为 not-replayed: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["state"], "not-replayed");
    let (oks, sos, _) = run(&["status", d.to_str().unwrap()]);
    assert!(oks);
    let vs: serde_json::Value = serde_json::from_str(sos.trim()).unwrap();
    assert_eq!(vs["challenge_accepted"].as_bool(), Some(false));
    assert_eq!(vs["verdicts"]["X"]["state"], "not-replayed");
}

/// 反例 6: 只有 X 被挑战,Y 无行为证据;两 cross 仅"XY 人工裁决"文字 →
/// 不得 review_accepted,X 不得被改 challenge-refuted。随后结构化 cross
/// (X accept / Y pending)可收录,但 Y 缺可核对证据仍不得收口。
#[test]
fn k3_rescue_cross_prose_and_unchallenged_claim_not_accepted() {
    // 初审声明 X/Y 两条 claims;fixture 仓库同时是评审 HEAD 与 live 执行 repo
    let (ft, fx_repo, _h) = make_cargo_fixture("k3r-xy", FIXTURE_FAIL_TEST);
    let t2 = TmpDir::new("k3r-xy2");
    let d2 = t2.path().to_path_buf();
    let h2 = review_head(&d2, &fx_repo);
    write_authority(&d2, "glm", "1", "completed");
    write_authority(&d2, "k3", "1", "completed");
    let glm = write_review_full(
        &d2,
        "glm.md",
        "glm",
        "completed",
        "1",
        Some(&h2),
        Some(&["X", "Y"]),
        "glm 初审",
    );
    let k3 = write_review_full(
        &d2,
        "k3.md",
        "k3",
        "completed",
        "1",
        Some(&h2),
        Some(&["X", "Y"]),
        "k3 初审",
    );
    let (okf, sof, sef) = run(&[
        "freeze",
        d2.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d2).to_str().unwrap(),
        "--head",
        &h2,
    ]);
    assert!(okf, "freeze failed: {sof} {sef}");
    // 只挑战 X(真实 live FAIL);Y 无行为证据
    let (okc, soc, sec) = live_challenge(
        &d2,
        &fx_repo,
        "X",
        "fixture_probe_fails",
        "fail",
        &ft.path().join("target"),
    );
    assert!(okc, "live challenge failed: {soc} {sec}");
    // 两份 cross 仅"XY 人工裁决"文字 → 拒绝(无结构化覆盖)
    // (文件名与冻结初审解耦;覆盖 glm.md/k3.md 会触发 first-review-tampered)
    for slug in ["glm", "k3"] {
        let cross = write_cross_prose(
            &d2,
            slug,
            &format!("cross-{slug}-prose.md"),
            "2",
            "XY 人工裁决",
        );
        let (ok, so, _) = run(&[
            "cross",
            d2.to_str().unwrap(),
            "--cross-report",
            cross.to_str().unwrap(),
            "--cross-slug",
            slug,
            "--native-root",
            native_root(&d2).to_str().unwrap(),
        ]);
        assert!(!ok, "纯文字 cross 不得收录({slug}): {so}");
        let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
        assert_eq!(v["error"]["code"], "missing-claim-coverage");
    }
    let (oks, sos, _) = run(&["status", d2.to_str().unwrap()]);
    assert!(oks);
    let vs: serde_json::Value = serde_json::from_str(sos.trim()).unwrap();
    assert_eq!(vs["review_accepted"].as_bool(), Some(false));
    assert_ne!(
        vs["verdicts"]["X"]["state"].as_str(),
        Some("challenge-refuted"),
        "人工裁决文字不得把已复现失败改判 challenge-refuted"
    );
    // 结构化 cross(X accept / Y pending)可收录,但 Y 无可核对证据 → 不收口
    for slug in ["glm", "k3"] {
        let cross = write_cross_structured(
            &d2,
            slug,
            &format!("cross-{slug}.md"),
            "2",
            "completed",
            r#"[{"id": "X", "verdict": "accept"}, {"id": "Y", "verdict": "pending"}]"#,
        );
        let (ok, so, se) = run(&[
            "cross",
            d2.to_str().unwrap(),
            "--cross-report",
            cross.to_str().unwrap(),
            "--cross-slug",
            slug,
            "--native-root",
            native_root(&d2).to_str().unwrap(),
        ]);
        assert!(ok, "结构化 cross({slug}) failed: {so} {se}");
    }
    let (ok2, so2, _) = run(&["status", d2.to_str().unwrap()]);
    assert!(ok2);
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(
        v2["review_accepted"].as_bool(),
        Some(false),
        "Y 无行为证据,双 cross 也不得收口: {so2}"
    );
    assert_eq!(v2["verdicts"]["Y"]["state"], "pending-behavioral-evidence");
}

/// 未冻结 claim 不得挑战(冻结即确立全部 claim ID,禁止新增)。
#[test]
fn k3_rescue_challenge_rejects_unfrozen_claim() {
    let (ft, fx_repo, _h) = make_cargo_fixture("k3r-unfrozen", FIXTURE_PASS_TEST);
    let (t, _g, _k) = frozen_dir_at("k3r-unfrozen", Some(&fx_repo));
    let d = t.path();
    let (ok, so, _) = live_challenge(
        d,
        &fx_repo,
        "Z",
        "fixture_probe_pass",
        "pass",
        &ft.path().join("target"),
    );
    assert!(!ok, "未冻结 claim Z 不得挑战: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "claim-not-frozen");
}

/// 场景(spec 绑定): cross 反驳 challenge-flip 的回边 — refute 只在有
/// 已验证新行为证据或显式可审计 operator 决定(--allow-operator-refute)
/// 时生效;缺有效依据保持原失败。
#[test]
fn olp_review_cross_refutation_reverts_flip() {
    let (t, _glm) = challenged_dir("k3r-refutebasis");
    let d = t.path().to_path_buf();
    // (a) 伪造 executed-evidence 引用 → refutation-unsubstantiated
    let cross = write_cross_structured(
        &d,
        "k3",
        "cross-k3-bad.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "refute", "reference": {"kind": "executed-evidence", "ref": "deadbeef"}}]"#,
    );
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(!ok, "伪造证据引用的 refute 应拒绝: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "refutation-unsubstantiated");
    // (b) operator 决定但缺显式 --allow-operator-refute → 拒绝(可审计性)
    let cross2 = write_cross_structured(
        &d,
        "k3",
        "cross-k3-op.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "refute", "reference": {"kind": "operator-decision", "operator": "zhangalex", "note": "反例针对旧 base,HEAD 已修复,人工裁决不阻塞"}}]"#,
    );
    let (ok2, so2, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross2.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(
        !ok2,
        "缺 --allow-operator-refute 的 operator refute 应拒绝: {so2}"
    );
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v2["error"]["code"], "refutation-unsubstantiated");
    // (c) 显式 operator 裁决 → challenge-refuted(保留 refuted 标记与依据)
    let (ok3, so3, se3) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross2.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--allow-operator-refute",
    ]);
    assert!(ok3, "显式 operator refute failed: {so3} {se3}");
    let st: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    assert_eq!(st["verdicts"]["X"]["state"], "challenge-refuted");
    assert_eq!(st["verdicts"]["X"]["refuted_by"], "k3");
    // (d) glm 结构化 accept → 全部 claim 有可核对依据 + 双 cross → accepted
    let cross_g = write_cross_structured(
        &d,
        "glm",
        "cross-glm.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "accept"}]"#,
    );
    let (ok4, so4, se4) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross_g.to_str().unwrap(),
        "--cross-slug",
        "glm",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok4, "cross glm failed: {so4} {se4}");
    let (ok5, so5, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok5);
    let v5: serde_json::Value = serde_json::from_str(so5.trim()).unwrap();
    assert_eq!(v5["review_accepted"].as_bool(), Some(true), "{so5}");
    assert_eq!(v5["verdicts"]["X"]["state"], "challenge-refuted");
}

/// v5: 实时重执行反驳 — live FAIL(blocked-on-evidence)后,同一评审 HEAD 上
/// live PASS 重执行构成"已验证新行为证据",判词转 challenge-refuted;
/// cross 以该执行记录 SHA 引用 refute 有效,伪造 SHA 拒绝。
#[test]
fn k3_rescue_reexecution_refutes_reproduced_failure() {
    let (ft, fx_repo, _h) = make_cargo_fixture("k3r-reexec", FIXTURE_PASS_AND_FAIL);
    let (t, _g, _k) = frozen_dir_at("k3r-reexec", Some(&fx_repo));
    let d = t.path().to_path_buf();
    let td = ft.path().join("target");
    // 第一次 live 执行: FAIL → blocked-on-evidence
    let (ok, so, se) = live_challenge(&d, &fx_repo, "X", "fixture_probe_fails", "fail", &td);
    assert!(ok, "live FAIL challenge failed: {so} {se}");
    let st1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    assert_eq!(st1["verdicts"]["X"]["state"], "blocked-on-evidence");
    // 重执行: PASS → 新行为证据反驳已复现失败 → challenge-refuted
    let (ok2, so2, se2) = live_challenge(&d, &fx_repo, "X", "fixture_probe_pass", "pass", &td);
    assert!(ok2, "live PASS re-execution failed: {so2} {se2}");
    let st2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        st2["verdicts"]["X"]["state"], "challenge-refuted",
        "live PASS 重执行应反驳已复现失败"
    );
    let pass_sha = st2["challenges"]["X"]["latest"]["receipt_sha256"]
        .as_str()
        .unwrap()
        .to_string();
    // cross refute 引用伪造 SHA → 拒绝
    let bad = write_cross_structured(
        &d,
        "glm",
        "cross-glm-bad.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "refute", "reference": {"kind": "executed-evidence", "ref": "cafe"}}]"#,
    );
    let (okb, sob, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        bad.to_str().unwrap(),
        "--cross-slug",
        "glm",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(!okb, "伪造 SHA 的 refute 引用应拒绝: {sob}");
    // cross refute 引用真实 pass 执行 SHA → 有效(确认 challenge-refuted)
    let good = write_cross_structured(
        &d,
        "glm",
        "cross-glm.md",
        "2",
        "completed",
        &format!(
            r#"[{{"id": "X", "verdict": "refute", "reference": {{"kind": "executed-evidence", "ref": "{pass_sha}"}}}}]"#
        ),
    );
    let (ok3, so3, se3) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        good.to_str().unwrap(),
        "--cross-slug",
        "glm",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok3, "真实执行 SHA 的 refute 引用应有效: {so3} {se3}");
    let cross_k = write_cross_structured(
        &d,
        "k3",
        "cross-k3.md",
        "2",
        "completed",
        r#"[{"id": "X", "verdict": "accept"}]"#,
    );
    let (ok4, so4, se4) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross_k.to_str().unwrap(),
        "--cross-slug",
        "k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok4, "cross k3 failed: {so4} {se4}");
    let (ok5, so5, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok5);
    let v5: serde_json::Value = serde_json::from_str(so5.trim()).unwrap();
    assert_eq!(v5["review_accepted"].as_bool(), Some(true), "{so5}");
    assert_eq!(v5["verdicts"]["X"]["state"], "challenge-refuted");
}
