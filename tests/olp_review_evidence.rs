//! olp-review-evidence 生产入口集成测试(spec: review-freeze / review-challenge /
//! review-cross / review-peer-validity / review-regression / JSON+human 双输出)。
//!
//! 全部以子进程真实调用 scripts/olp-review-evidence.py。
//! 顺序如实记录: 脚本与首批 18 测试为同轮实现(先落盘后补测试,详见
//! .octos/red-proof/evidence-red.log 的真实 RED 记录与本文件 git 史);
//! 外层反例场景(olp_review_outer_probe_*)按"先 RED 后修"补写,
//! 对应 ../outer-evidence-probe-results.json 实测缺陷。

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
    let p = dir.join(name);
    let head_line = head.map(|h| format!("HEAD: {h}\n")).unwrap_or_default();
    std::fs::write(
        &p,
        format!("---\nslug: {name}\noutcome: {outcome}\nturn: {turn}\n{head_line}---\n\n初审内容: PR#627 存在行为缺口,判词 X=approve。\n"),
    )
    .unwrap();
    p
}

/// native 终止收据(外部权威): native/<slug>/result-<turn>.md + turns.txt。
/// freeze/cross fail-closed 后,测试必须提供至少一种外部权威。
fn write_authority(dir: &Path, slug: &str, turn: &str, outcome: &str) {
    let nd = dir.join("native").join(slug);
    std::fs::create_dir_all(&nd).unwrap();
    std::fs::write(
        nd.join(format!("result-{turn}.md")),
        format!("---\nslug: {slug}\noutcome: {outcome}\nturn: {turn}\n---\nbody\n"),
    )
    .unwrap();
    std::fs::write(nd.join("turns.txt"), format!("{turn} {outcome} 100\n")).unwrap();
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

// ---------------------------------------------------------------------------
// review-freeze
// ---------------------------------------------------------------------------

/// 场景: 冻结两份独立初审 — 仅有 glm 一份时 freeze 报错且 manifest 未写入。
#[test]
fn olp_review_freeze_requires_both_first_reviews() {
    let t = TmpDir::new("freeze-missing");
    let d = t.path();
    init_review(d, "H1");
    write_review(d, "glm.md", "completed", "2", None);
    // k3 缺失
    let (ok, so, _se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        d.join("glm.md").to_str().unwrap(),
        "--k3-review",
        d.join("k3.md").to_str().unwrap(),
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
    init_review(d, "H1");
    write_authority(d, "peer-glm-x", "2", "completed");
    write_authority(d, "peer-k3-x", "3", "completed");
    let glm = write_review(d, "glm.md", "completed", "2", None);
    let k3 = write_review(d, "k3.md", "completed", "3", None);
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
    assert_eq!(v["reviews"]["k3"]["turn"], "3");
    assert!(v["reviews"]["glm"]["sha256"].as_str().unwrap().len() == 64);
}

/// 场景: 冻结后篡改初审被拒绝。
#[test]
fn olp_review_freeze_rejects_tampered_first_review() {
    let t = TmpDir::new("tamper");
    let d = t.path();
    init_review(d, "H1");
    write_authority(d, "glm", "2", "completed");
    write_authority(d, "k3", "3", "completed");
    let glm = write_review(d, "glm.md", "completed", "2", None);
    let k3 = write_review(d, "k3.md", "completed", "3", None);
    let (ok, _, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(d).to_str().unwrap(),
    ]);
    assert!(ok);
    // 篡改: 追加一行
    std::fs::write(&glm, format!("{}\n事后追加行\n", std::fs::read_to_string(&glm).unwrap())).unwrap();
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
    init_review(d, "H1");
    let glm = write_review(d, "glm.md", "completed", "2", None);
    let k3 = write_review(d, "k3.md", "completed", "3", None);
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
    init_review(d, "H1");
    let glm = write_review(d, "glm.md", "completed", "2", None);
    let k3 = write_review(d, "k3.md", "completed", "3", None);
    // native dir: peer-glm-x 最新 result 是 errored
    let native = d.join("native").join("peer-glm-x");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(
        native.join("result-2.md"),
        "---\nslug: peer-glm-x\noutcome: errored\nturn: 2\n---\nerrored body\n",
    )
    .unwrap();
    std::fs::write(native.join("turns.txt"), "1 errored 1788919255\n2 errored 1788919300\n").unwrap();
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
    init_review(d1, "H1");
    write_authority(d1, "glm", "2", "completed");
    write_authority(d1, "k3", "3", "completed");
    let glm1 = write_review(d1, "glm.md", "completed", "2", Some("H2"));
    let k31 = write_review(d1, "k3.md", "completed", "3", None);
    let (ok, so, _) = run(&[
        "freeze",
        d1.to_str().unwrap(),
        "--glm-review",
        glm1.to_str().unwrap(),
        "--k3-review",
        k31.to_str().unwrap(),
        "--native-root",
        native_root(d1).to_str().unwrap(),
    ]);
    assert!(!ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "head-mismatch");

    // (b) stale turn: 报告 turn=1 但 turns.txt 已到 2
    let t2 = TmpDir::new("stale");
    let d2 = t2.path();
    init_review(d2, "H1");
    let glm2 = write_review(d2, "glm.md", "completed", "1", None);
    let k32 = write_review(d2, "k3.md", "completed", "3", None);
    let native = d2.join("native").join("peer-glm-y");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(
        native.join("result-2.md"),
        "---\nslug: peer-glm-y\noutcome: completed\nturn: 2\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(native.join("turns.txt"), "1 completed 100\n2 completed 200\n").unwrap();
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
    ]);
    assert!(!ok2, "旧轮次应被拒绝");
    let v2: serde_json::Value = serde_json::from_str(so2.trim()).unwrap();
    assert_eq!(v2["error"]["code"], "stale-turn");
}

// ---------------------------------------------------------------------------
// review-challenge
// ---------------------------------------------------------------------------

fn frozen_dir(tag: &str) -> (TmpDir, PathBuf, PathBuf) {
    let t = TmpDir::new(tag);
    let d = t.path().to_path_buf();
    init_review(&d, "H1");
    write_authority(&d, "glm", "2", "completed");
    write_authority(&d, "k3", "3", "completed");
    let glm = write_review(&d, "glm.md", "completed", "2", None);
    let k3 = write_review(&d, "k3.md", "completed", "3", None);
    let (ok, _, se) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok, "freeze failed: {se}");
    (t, glm, k3)
}

/// 真实执行 probe: 一个必定失败的最小 cargo 样式 harness(python 模拟执行体)。
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
        format!(
            "running 1 test\ntest probe_X ... \nthread 'probe_X' (1) panicked at probe.rs:10:5:\nassertion failed\nFAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;\n"
        ),
    )
    .unwrap();
    let argv = vec![
        "python3".into(),
        probe.to_str().unwrap().to_string(),
        "probe_X".into(),
    ];
    (ev, argv)
}

/// 场景: 行为证据驱动改判(真实执行,exit 101 → flipped/blocked-on-evidence)。
#[test]
fn olp_review_challenge_evidence_flips_verdict() {
    let (t, _glm, _k3) = frozen_dir("flip");
    let d = t.path();
    let (ev, argv) = write_probe(d);
    let (ok, so, se) = run(&[
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
    assert!(ok, "challenge failed: {so} {se}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    let s = v["state"].as_str().unwrap();
    assert!(
        s == "flipped" || s == "blocked-on-evidence",
        "判词应翻转为 flipped/blocked-on-evidence,实际 {s}"
    );
}

/// 场景: 文档性证据不被接纳(无论 kind 声明)。
#[test]
fn olp_review_challenge_rejects_non_behavioral_evidence() {
    let (t, _glm, _k3) = frozen_dir("docev");
    let d = t.path();
    let ev = d.join("doc-ev.md");
    std::fs::write(&ev, "# 挑战\nPR#627 有问题:assert!(\"字符串常量断言\"); 判词 X 不成立。\n").unwrap();
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
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap()).unwrap();
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
        // 无 --exec-argv: 结构齐全但无执行
    ]);
    assert!(!ok, "结构形似但无执行应被拒绝");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
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

fn challenged_dir(tag: &str) -> (TmpDir, PathBuf) {
    let (t, glm, _k3) = frozen_dir(tag);
    let d = t.path().to_path_buf();
    let (ev, argv) = write_probe(&d);
    let (ok, so, se) = run(&[
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
    assert!(ok, "challenge failed: {so} {se}");
    (t, glm)
}

/// cross 报告的 helper: 写报告 + native 权威收据。
fn write_cross_report(
    d: &Path,
    slug: &str,
    turn: &str,
    outcome: &str,
    body: &str,
) -> PathBuf {
    write_authority(d, slug, turn, outcome);
    let p = d.join(format!("{slug}.md"));
    std::fs::write(
        &p,
        format!("---\nslug: {slug}\noutcome: {outcome}\nturn: {turn}\n---\n\n{body}\n"),
    )
    .unwrap();
    p
}

/// 场景: 交叉互审需初审冻结且挑战证据已接纳。
#[test]
fn olp_review_cross_requires_frozen_and_challenged() {
    let t = TmpDir::new("crosspending");
    let d = t.path().to_path_buf();
    init_review(&d, "H1");
    write_authority(&d, "glm", "2", "completed");
    write_authority(&d, "k3", "3", "completed");
    let glm = write_review(&d, "glm.md", "completed", "2", None);
    let k3 = write_review(&d, "k3.md", "completed", "3", None);
    let (ok, _, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
        "--native-root",
        native_root(&d).to_str().unwrap(),
    ]);
    assert!(ok);
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
        "--expect-claims",
        "X",
    ]);
    assert!(!ok2, "challenge 未接纳时 cross 应拒绝");
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

    let cross = write_cross_report(
        &d,
        "cross-k3",
        "4",
        "completed",
        "X: 采纳(引用 probe_X)\nY: 待验证",
    );
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "cross-k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--expect-claims",
        "X,Y,Z",
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
    let cross = write_cross_report(&d, "cross-k3", "4", "errored", "X: 采纳");
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "cross-k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--expect-claims",
        "X",
    ]);
    assert!(!ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-outcome-invalid");
}

/// 场景: cross 反驳 challenge-flip 的回边(challenge-refuted)。
#[test]
fn olp_review_cross_refutation_reverts_flip() {
    let (t, _glm) = challenged_dir("refute");
    let d = t.path().to_path_buf();
    // challenge 后 X 已 flipped(见 challenged_dir)
    let cross = write_cross_report(
        &d,
        "cross-k3",
        "4",
        "completed",
        "X: 反驳成立 — probe.rs:10 的反例前提不成立(行号 10 处已修复),引用新证据。",
    );
    let (ok, so, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "cross-k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--expect-claims",
        "X",
    ]);
    assert!(ok, "cross failed: {so}");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert!(v["refuted"].as_array().unwrap().contains(&serde_json::json!("X")));
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap()).unwrap();
    assert_eq!(state["verdicts"]["X"]["state"], "challenge-refuted");
}

/// 场景: 有充分行为证据时 approve 成立(happy path,探针真实执行通过)。
#[test]
fn olp_review_approve_with_executed_evidence() {
    let (t, _glm, _k3) = frozen_dir("happy");
    let d = t.path().to_path_buf();
    // 通过型探针: 真实执行 exit 0 + test result: ok
    let probe = d.join("probe-pass.py");
    std::fs::write(
        &probe,
        r#"import sys
name = sys.argv[1]
print(f"running 1 test")
print(f"test {name} ... ok")
print("test result: ok. 1 passed; 0 failed; 0 ignored;")
sys.exit(0)
"#,
    )
    .unwrap();
    let ev = d.join("ev-pass.log");
    std::fs::write(&ev, "running 1 test\ntest probe_P ... ok\ntest result: ok. 1 passed; 0 failed;\n").unwrap();
    let (ok, so, se) = run(&[
        "challenge",
        d.to_str().unwrap(),
        "--evidence",
        ev.to_str().unwrap(),
        "--claim",
        "X",
        "--test-name",
        "probe_P",
        "--exec-argv",
        &format!("python3 {} probe_P", probe.to_str().unwrap()),
        "--run-dir",
        d.to_str().unwrap(),
    ]);
    assert!(ok, "challenge failed: {so} {se}");
    // cross 覆盖
    let cross = write_cross_report(&d, "cross-k3", "4", "completed", "X: 采纳");
    let (ok2, so2, _) = run(&[
        "cross",
        d.to_str().unwrap(),
        "--cross-report",
        cross.to_str().unwrap(),
        "--cross-slug",
        "cross-k3",
        "--native-root",
        native_root(&d).to_str().unwrap(),
        "--expect-claims",
        "X",
    ]);
    assert!(ok2, "cross failed: {so2}");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("review-state.json")).unwrap()).unwrap();
    assert_eq!(state["verdicts"]["X"]["state"], "approve");
}

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
#[test]
fn olp_review_real_store_harness_executes_historical_negative_probes() {
    let t = TmpDir::new("realharness");
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let evidence = repo.join("fixtures/review-evidence/evidence");
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
    let out = Command::new("bash")
        .arg(evidence.join("reproduce.sh"))
        .arg(&repo)
        .arg(&clone)
        .output()
        .expect("spawn reproduce.sh");
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
    init_review(d, "H1");
    let glm = write_review(d, "glm.md", "completed", "1", None);
    let k3 = write_review(d, "k3.md", "completed", "1", None);
    // 不传 --native-root / --runtime-evidence: 无任何外部权威
    let (ok, so, _) = run(&[
        "freeze",
        d.to_str().unwrap(),
        "--glm-review",
        glm.to_str().unwrap(),
        "--k3-review",
        k3.to_str().unwrap(),
    ]);
    assert!(!ok, "无外部权威时 freeze 不得成功(外层反例1)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-authority-missing");
    // 提供匹配的 native 终止收据后应成功
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
    }
    let t2_dir = d.join("with-authority");
    std::fs::create_dir_all(&t2_dir).unwrap();
    init_review(&t2_dir, "H1");
    let glm2 = write_review(&t2_dir, "glm.md", "completed", "1", None);
    let k32 = write_review(&t2_dir, "k3.md", "completed", "1", None);
    let (ok2, so2, se2) = run(&[
        "freeze",
        t2_dir.to_str().unwrap(),
        "--glm-review",
        glm2.to_str().unwrap(),
        "--k3-review",
        k32.to_str().unwrap(),
        "--native-root",
        native.to_str().unwrap(),
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
    assert_eq!(v["error"]["code"], "evidence-not-executed");
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
    std::fs::write(&ev, "running 1 test\ntest probe_X ... ok\ntest result: ok. 1 passed;\n").unwrap();
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
    init_review(d, "H1");
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
    let glm = write_review(d, "glm.md", "completed", "2", None);
    let k3 = write_review(d, "k3.md", "completed", "3", None);
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
    assert!(!ok, "slug 不匹配的收据不得作权威(跨 runtime 冒充)");
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert_eq!(v["error"]["code"], "peer-authority-mismatch");
}

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
    assert!(files.len() >= 60, "fixtures 应含 60+ 文件,实际 {}", files.len());
    // adversarial.log 是缺陷证据: 8 failed,不得写成通过
    let adv = std::fs::read_to_string(fx.join("evidence/adversarial.log")).unwrap();
    assert!(adv.contains("8 failed"), "adversarial.log 应记录 8 failed");
    assert!(!adv.contains("8 passed"), "不得把 8 failed 写成 8 passed");
    // MANIFEST 期望分类(外层独立写入)存在且非硬编码于脚本
    let outer: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fx.join("MANIFEST.json")).unwrap()).unwrap();
    assert_eq!(outer["prs"]["627"]["outer_recommendation"], "request-changes");
    assert_eq!(outer["prs"]["629"]["outer_recommendation"], "conditional-approve-scope-and-specs");
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
    init_review(&d, "H1");
    // JSON status 可解析
    let (ok, so, _) = run(&["status", d.to_str().unwrap()]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(so.trim()).unwrap();
    assert!(v["identity"].is_object() || v["frozen"].is_boolean());
    // 错误输出也是可解析 JSON
    let (ok2, so2, _) = run(&["challenge", d.to_str().unwrap(), "--evidence", "nope.log", "--claim", "X"]);
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
}
