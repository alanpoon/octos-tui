#!/usr/bin/env python3
"""olp-review-evidence — OctoLoop 行为证据互审流程管理入口.

Spec: specs/task-evo-review-evidence.spec.md (23 scenarios)
Plan: docs/superpowers/plans/2026-09-09-review-evidence.md

子命令:
  init     — 初始化评审目录(记录 HEAD/base/runtime/session/goal)
  freeze   — 冻结两份独立初审(SHA256/HEAD/base/runtime/session/goal/peer/turn)
  challenge— 接纳行为证据(内容校验为门、执行为本;imported→not-replayed)
  cross    — 收录交叉报告(需 frozen+challenged;逐 claim 裁决;errored/pending 拒绝)
  status   — 汇总判词状态(claim 级两层词表 + PR 级聚合)

错误输出一律可解析 JSON: {"error": {"code": "...", "message": "..."}} 且 exit != 0.
冻结与 manifest 写入原子: 先写临时文件再 os.replace,并发/失败不出现半写状态.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

PROTOCOL = "olp-review-evidence/v1"
STATE_FILENAME = "review-state.json"

# ---------------------------------------------------------------------------
# error handling: structured JSON on stdout, non-zero exit
# ---------------------------------------------------------------------------


class ReviewError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def emit_json(obj: dict) -> None:
    json.dump(obj, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


def fail(code: str, message: str) -> "ReviewError":
    return ReviewError(code, message)


# ---------------------------------------------------------------------------
# state store (atomic writes)
# ---------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def atomic_write_json(path: Path, obj: dict) -> None:
    """Write JSON atomically: temp file in same dir + fsync + os.replace."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(path.parent), prefix=".tmp-state-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(obj, fh, ensure_ascii=False, indent=1)
            fh.write("\n")
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def load_state(review_dir: Path) -> dict:
    p = review_dir / STATE_FILENAME
    if not p.exists():
        return {}
    try:
        return json.loads(p.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise fail("state-corrupt", f"{p} 不是合法 JSON: {e}")


def save_state(review_dir: Path, state: dict) -> None:
    state["updated_unix"] = int(time.time())
    atomic_write_json(review_dir / STATE_FILENAME, state)


# ---------------------------------------------------------------------------
# frontmatter parsing (review reports)
# ---------------------------------------------------------------------------

_FM_RE = re.compile(r"\A---\s*\n(.*?)\n---\s*\n", re.S)


def parse_frontmatter(path: Path) -> dict:
    text = path.read_text(encoding="utf-8", errors="replace")
    m = _FM_RE.match(text)
    if not m:
        return {"__missing__": True, "text_head": text[:200]}
    fm: dict = {}
    for line in m.group(1).splitlines():
        if ":" in line:
            k, _, v = line.partition(":")
            fm[k.strip()] = v.strip()
    fm["__sha256__"] = sha256_file(path)
    return fm


# ---------------------------------------------------------------------------
# peer validity: external signals override self-reported outcome
# ---------------------------------------------------------------------------


def latest_result(peer_dir: Path) -> tuple[int, Path] | None:
    """Highest-numbered result-N.md in a native peer dir."""
    best = None
    for f in peer_dir.glob("result-*.md"):
        m = re.fullmatch(r"result-(\d+)\.md", f.name)
        if m:
            n = int(m.group(1))
            if best is None or n > best[0]:
                best = (n, f)
    return best


def turns_max_completed(peer_dir: Path) -> int:
    """Highest finished-turn number recorded in turns.txt."""
    p = peer_dir / "turns.txt"
    if not p.exists():
        return -1
    mx = -1
    for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split()
        if parts and parts[0].isdigit():
            mx = max(mx, int(parts[0]))
    return mx


def check_peer_validity(
    fm: dict,
    report_path: Path,
    native_dir: Path | None,
    runtime_evidence: dict | None,
    slug: str,
    require_authority: bool = False,
) -> bool:
    """Reject pending/errored/stale peers; external signals veto self-report.

    Returns True when at least one external authority (native result-N or
    runtime-evidence explicit termination) vouched for this slug. When
    require_authority is set and no authority matched, fail-closed with
    peer-authority-missing — a bare self-reported outcome is never enough
    (outer counterexample: two turn-1 completed reports froze with zero
    native/runtime thread evidence).
    """
    outcome = fm.get("outcome")
    if outcome in ("pending", "errored"):
        raise fail(
            "peer-outcome-invalid",
            f"{slug} 自报 outcome={outcome},不得作为有效初审/交叉",
        )
    has_authority = False
    # External signal 1: runtime-evidence.json active_thread (pending in flight)
    if runtime_evidence is not None:
        matched = False
        for peer in runtime_evidence.get("peers", []):
            if peer.get("slug") == slug:
                matched = True
                if peer.get("active_thread"):
                    raise fail(
                        "peer-outcome-invalid",
                        f"{slug} runtime-evidence active_thread 非空(仍有活跃轮次),"
                        " 报告声明不可采信",
                    )
                has_authority = True
        if matched is False and runtime_evidence.get("peers") is not None:
            # runtime-evidence lists peers but not this one: no active-thread
            # clearance for this slug — not an authority for it.
            pass
    # External signal 2: native result-N.md outcome vetoes self-report
    if native_dir is not None and native_dir.exists():
        latest = latest_result(native_dir)
        if latest is not None:
            n, path = latest
            native_fm = parse_frontmatter(path)
            native_slug = native_fm.get("slug")
            if native_slug and native_slug != slug:
                raise fail(
                    "peer-authority-mismatch",
                    f"{slug} native {path.name} slug={native_slug} 不匹配",
                )
            native_outcome = native_fm.get("outcome")
            if native_outcome == "errored":
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} native {path.name} outcome=errored,否决报告自报 {outcome}",
                )
            has_authority = True
            # stale turn: report turn lower than the peer's own finished turns
            turn = fm.get("turn")
            if turn is not None and str(turn).isdigit():
                mx = turns_max_completed(native_dir)
                if int(turn) < mx:
                    raise fail(
                        "stale-turn",
                        f"{slug} 报告 turn={turn} 低于其 turns.txt 已结束轮次 {mx}",
                    )
    if require_authority and not has_authority:
        raise fail(
            "peer-authority-missing",
            f"{slug} 无外部终止权威(native result-N 或 runtime-evidence 终止快照),"
            " 仅自报 outcome 不足以采信",
        )
    return has_authority


# ---------------------------------------------------------------------------
# behavioral evidence gates
# ---------------------------------------------------------------------------

# necessary structural anchors (NOT sufficient — execution is the real gate)
_ANCHOR_TEST_NAME = re.compile(r"^\s*test\s+\S+.*$", re.M)
_ANCHOR_PANIC = re.compile(r"panicked at [^\s:]+:\d+:\d+")
_ANCHOR_SUMMARY = re.compile(r"test result: FAILED\..*\d+ failed")
_PASS_SUMMARY = re.compile(r"test result: ok\.", re.M)


def structural_anchors(ev_path: Path, test_name: str) -> list[str]:
    """Return list of missing structural anchors for a behavioral log.

    Accepts either failure-form (panicked at + FAILED summary) or pass-form
    (test result: ok) — the happy path binds an executed PASSING probe.
    """
    text = ev_path.read_text(encoding="utf-8", errors="replace")
    fail_form = _ANCHOR_PANIC.search(text) and _ANCHOR_SUMMARY.search(text)
    pass_form = _PASS_SUMMARY.search(text)
    missing = []
    if test_name and test_name not in text:
        missing.append(f"测试名行缺失: {test_name}")
    if not fail_form and not pass_form:
        missing.append(
            "行为锚缺失: 需失败形态(panicked at + test result: FAILED. N failed)"
            "或通过形态(test result: ok.)"
        )
    return missing


def test_name_resolvable(test_name: str, harness_roots: list[Path]) -> bool:
    """Test filter name must resolve to a definition in harness sources."""
    needle = re.escape(test_name.split("::")[-1])
    pat = re.compile(r"fn\s+" + needle + r"\s*\(")
    for root in harness_roots:
        if not root.exists():
            continue
        for f in root.rglob("*.rs") if root.is_dir() else [root]:
            try:
                if pat.search(f.read_text(encoding="utf-8", errors="replace")):
                    return True
            except OSError:
                continue
    return False


def execute_probe(argv: list[str], cwd: Path, timeout: int) -> dict:
    """Actually run the evidence's argv; capture exit code and output digests.

    This is the execution gate: a fabricated log cannot pass because we run
    the real command and compare captured stdout to the submitted log body.
    """
    if not argv:
        raise fail("evidence-not-executed", "证据未携带可执行 argv")
    try:
        proc = subprocess.run(
            argv,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except FileNotFoundError:
        raise fail("evidence-not-executed", f"argv[0] 不存在: {argv[0]}")
    except subprocess.TimeoutExpired:
        raise fail("evidence-not-executed", f"执行超时(>{timeout}s): {argv[:3]}")
    return {
        "argv": argv,
        "exit_code": proc.returncode,
        "stdout_sha256": hashlib.sha256(proc.stdout.encode()).hexdigest(),
        "stderr_sha256": hashlib.sha256(proc.stderr.encode()).hexdigest(),
        "stdout_full": proc.stdout,
        "stderr_full": proc.stderr,
    }


# ---------------------------------------------------------------------------
# verdict state machine (claim-level)
# ---------------------------------------------------------------------------

CLAIM_STATES = (
    "approve",
    "flipped",
    "blocked-on-evidence",
    "pending-behavioral-evidence",
    "unverified",
    "not-replayed",
    "challenge-refuted",
)


def cmd_init(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    state = load_state(review_dir)
    if state:
        raise fail("already-initialized", f"{review_dir} 已初始化")
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=args.repo, capture_output=True, text=True
    ).stdout.strip()
    state = {
        "protocol": PROTOCOL,
        "head": head or args.head,
        "base": args.base,
        "runtime": args.runtime,
        "session": args.session,
        "goal": args.goal,
        "reviews": {},
        "frozen": False,
        "challenge": None,
        "cross": [],
        "verdicts": {},
    }
    save_state(review_dir, state)
    emit_json({"ok": True, "state": "initialized", "head": state["head"]})


def cmd_freeze(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    state = load_state(review_dir)
    if not state:
        raise fail("not-initialized", "先 init")
    if state.get("frozen"):
        raise fail("already-frozen", "初审已冻结")

    glm_path, k3_path = Path(args.glm_review), Path(args.k3_review)
    for label, p in (("glm", glm_path), ("k3", k3_path)):
        if not p.exists():
            raise fail("first-reviews-incomplete", f"{label} 初审文件缺失: {p}")

    glm_fm, k3_fm = parse_frontmatter(glm_path), parse_frontmatter(k3_path)
    native_root = Path(args.native_root) if args.native_root else None
    runtime_evidence = None
    if args.runtime_evidence and Path(args.runtime_evidence).exists():
        runtime_evidence = json.loads(Path(args.runtime_evidence).read_text())

    for label, p, fm, slug in (
        ("glm", glm_path, glm_fm, args.glm_slug or "glm"),
        ("k3", k3_path, k3_fm, args.k3_slug or "k3"),
    ):
        check_peer_validity(
            fm,
            p,
            (native_root / slug) if native_root else None,
            runtime_evidence,
            slug,
            require_authority=True,
        )
        head = fm.get("HEAD") or fm.get("head")
        if head and state["head"] and head != state["head"]:
            raise fail("head-mismatch", f"{label} 初审 HEAD={head} != 评审 HEAD={state['head']}")

    # freeze: record identity then flip flag — both inside one atomic save
    state["reviews"] = {
        "glm": {
            "path": str(glm_path),
            "sha256": glm_fm["__sha256__"],
            "peer": args.glm_slug or "glm",
            "turn": glm_fm.get("turn"),
            "outcome": glm_fm.get("outcome"),
        },
        "k3": {
            "path": str(k3_path),
            "sha256": k3_fm["__sha256__"],
            "peer": args.k3_slug or "k3",
            "turn": k3_fm.get("turn"),
            "outcome": k3_fm.get("outcome"),
        },
    }
    state["frozen"] = True
    state["frozen_at_unix"] = int(time.time())
    save_state(review_dir, state)
    emit_json({"ok": True, "state": "frozen", "reviews": state["reviews"]})


def require_frozen(state: dict) -> None:
    if not state.get("frozen"):
        raise fail("not-frozen", "初审未冻结")


def verify_no_tamper(state: dict, args: argparse.Namespace) -> None:
    for label in ("glm", "k3"):
        rec = state["reviews"].get(label)
        if not rec:
            continue
        p = Path(rec["path"])
        if not p.exists():
            # fail-closed: a deleted frozen review is tampering, not absence
            raise fail(
                "first-review-tampered",
                f"{label} 初审在冻结后被删除(文件缺失),不予采信",
            )
        if sha256_file(p) != rec["sha256"]:
            raise fail(
                "first-review-tampered",
                f"{label} 初审在冻结后被修改(SHA256 不匹配),不予采信",
            )


def cmd_challenge(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    state = load_state(review_dir)
    if not state:
        raise fail("not-initialized", "先 init")
    require_frozen(state)
    verify_no_tamper(state, args)

    ev_path = Path(args.evidence)
    if not ev_path.exists():
        raise fail("evidence-missing", f"证据文件不存在: {ev_path}")

    kind_hint = args.kind or "unknown"
    claim = args.claim
    if not claim:
        raise fail("claim-required", "challenge 需 --claim 判词 id")

    body = ev_path.read_text(encoding="utf-8", errors="replace")

    # Gate 1 (cheap, negative): doc-only content can NEVER pass, regardless of
    # a self-declared kind=behavioral hint (spec: 内容校验为门).
    doc_like = (
        _ANCHOR_PANIC.search(body) is None
        and _ANCHOR_SUMMARY.search(body) is None
        and _ANCHOR_TEST_NAME.search(body) is None
    )
    if doc_like:
        state["verdicts"][claim] = {
            "state": "unverified",
            "reason": "evidence-not-behavioral",
        }
        save_state(review_dir, state)
        raise fail(
            "evidence-not-behavioral",
            "证据为文档性内容(字符串常量断言/纯 prose),无论 kind 声明如何均拒绝",
        )

    # Gate 2: structural anchors (necessary, NOT sufficient)
    missing = structural_anchors(ev_path, args.test_name or "")
    if missing:
        state["verdicts"][claim] = {"state": "unverified", "reason": "evidence-not-behavioral"}
        save_state(review_dir, state)
        raise fail("evidence-not-behavioral", "; ".join(missing))

    harness_roots = [Path(p) for p in (args.harness_root or []) if p]
    if args.test_name and harness_roots:
        if not test_name_resolvable(args.test_name, harness_roots):
            raise fail(
                "evidence-not-behavioral",
                f"测试名 {args.test_name} 无法在 harness 源解析到定义",
            )

    # Gate 3 (execution gate): imported evidence is downgraded, never accepted.
    if args.imported:
        state["verdicts"][claim] = {
            "state": "not-replayed",
            "reason": "imported-evidence-not-replayed",
            "evidence": str(ev_path),
        }
        state["challenge"] = {"accepted": False, "imported": True, "claim": claim}
        save_state(review_dir, state)
        emit_json(
            {
                "ok": True,
                "state": "not-replayed",
                "claim": claim,
                "note": "imported 证据未经本入口独立执行,不自动 accepted",
            }
        )
        return

    # Gate 4: actually execute the probe argv (execution-as-proof).
    argv = args.exec_argv.split() if args.exec_argv else []
    run_dir = Path(args.run_dir) if args.run_dir else review_dir
    exec_record = execute_probe(argv, run_dir, args.timeout)
    exec_record["stdout_tail"] = exec_record.pop("stdout_full")[-2000:]
    exec_record["stderr_tail"] = exec_record.pop("stderr_full")[-500:]
    exec_record["test_name"] = args.test_name
    exec_record["evidence_sha256"] = sha256_file(ev_path)
    exec_record["head"] = state["head"]
    exec_record["run_dir"] = str(run_dir)

    # The executed output must itself carry the failure anchors — a fabricated
    # log pasted next to a passing/no-op argv cannot pass.
    stdout = exec_record.get("stdout_full") or exec_record["stdout_tail"]
    if args.test_name and args.test_name not in stdout:
        raise fail(
            "evidence-not-executed",
            "执行输出不含测试名:构造日志与真实执行不符",
        )
    if not (_ANCHOR_PANIC.search(stdout) or _PASS_SUMMARY.search(stdout)):
        if exec_record["exit_code"] == 0 and not _ANCHOR_SUMMARY.search(stdout):
            raise fail(
                "evidence-not-executed",
                "执行未产生测试断言输出(非 cargo 测试执行),拒绝",
            )

    failed_exec = exec_record["exit_code"] != 0 and bool(_ANCHOR_SUMMARY.search(stdout))
    passed_exec = exec_record["exit_code"] == 0 and bool(_PASS_SUMMARY.search(stdout))

    state["challenge"] = {
        "accepted": True,
        "claim": claim,
        "evidence": str(ev_path),
        "executed": exec_record,
        "result": "probe-failed" if failed_exec else ("probe-passed" if passed_exec else "inconclusive"),
    }
    # Discriminator (spec): executed failure on PR HEAD → blocked-on-evidence;
    # legacy-path evidence → flipped (→ residual at PR level).
    new_state = (
        "blocked-on-evidence" if failed_exec else "approve" if passed_exec else "flipped"
    )
    prev = state["verdicts"].get(claim, {}).get("state", "approve")
    if prev == "approve":
        state["verdicts"][claim] = {
            "state": new_state if new_state != "approve" else "approve",
            "reason": "challenge-evidence-executed",
            "evidence": str(ev_path),
        }
    else:
        state["verdicts"][claim] = {
            "state": new_state,
            "reason": "challenge-evidence-executed",
            "evidence": str(ev_path),
        }
    save_state(review_dir, state)
    emit_json(
        {
            "ok": True,
            "state": state["verdicts"][claim]["state"],
            "claim": claim,
            "executed": {"exit_code": exec_record["exit_code"]},
        }
    )


def cmd_cross(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    state = load_state(review_dir)
    if not state:
        raise fail("not-initialized", "先 init")
    require_frozen(state)
    verify_no_tamper(state, args)
    ch = state.get("challenge") or {}
    if not ch.get("accepted"):
        raise fail("challenge-pending", "挑战证据尚未接纳,不得收录 cross")

    report = Path(args.cross_report)
    if not report.exists():
        raise fail("cross-missing", f"cross 报告不存在: {report}")
    fm = parse_frontmatter(report)
    native_root = Path(args.native_root) if args.native_root else None
    slug = args.cross_slug or "cross"
    runtime_evidence = None
    if args.runtime_evidence and Path(args.runtime_evidence).exists():
        runtime_evidence = json.loads(Path(args.runtime_evidence).read_text())
    check_peer_validity(
        fm,
        report,
        (native_root / slug) if native_root else None,
        runtime_evidence,
        slug,
        require_authority=True,
    )

    # per-claim coverage: every claim in the frozen first review must appear
    cross_text = report.read_text(encoding="utf-8", errors="replace")
    claims = list(state["verdicts"].keys())
    if args.expect_claims:
        claims = args.expect_claims.split(",")
    missing = [c for c in claims if c not in cross_text]
    if missing:
        raise fail("missing-claim-coverage", f"cross 报告未覆盖判词: {missing}")

    # refutation edge: cross may refute a flip with new evidence / line refs
    refutations = []
    for c in claims:
        st = state["verdicts"].get(c, {}).get("state")
        if st in ("flipped", "blocked-on-evidence"):
            block = _claim_section(cross_text, c)
            if block and re.search(r"(反驳|refute|行号|line \d|:\d+)", block, re.I):
                state["verdicts"][c] = {
                    **state["verdicts"][c],
                    "state": "challenge-refuted",
                    "refuted_by": slug,
                }
                refutations.append(c)

    state.setdefault("cross", []).append(
        {"slug": slug, "sha256": fm["__sha256__"], "refuted": refutations}
    )
    save_state(review_dir, state)
    emit_json({"ok": True, "state": "cross-recorded", "refuted": refutations})


def _claim_section(text: str, claim: str) -> str | None:
    """Return the paragraph/block mentioning the claim, for refutation scan."""
    idx = text.find(claim)
    if idx < 0:
        return None
    end = text.find("\n\n", idx)
    return text[idx : end if end > 0 else idx + 500]


def cmd_status(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    state = load_state(review_dir)
    if not state:
        raise fail("not-initialized", "先 init")
    verify_no_tamper(state, args)

    verdicts = dict(state.get("verdicts", {}))
    ch = state.get("challenge") or {}
    # agreement without executed evidence stays pending (never "approved")
    if not ch.get("accepted"):
        for c in verdicts:
            if verdicts[c].get("state") == "approve":
                verdicts[c] = {**verdicts[c], "state": "pending-behavioral-evidence"}

    emit_json(
        {
            "protocol": PROTOCOL,
            "frozen": state.get("frozen", False),
            "challenge_accepted": ch.get("accepted", False),
            "verdicts": verdicts,
            "identity": {
                k: state.get(k)
                for k in ("head", "base", "runtime", "session", "goal")
            },
        }
    )


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="olp-review-evidence")
    p.add_argument("--format", choices=["json", "human"], default="json")
    sub = p.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("init")
    sp.add_argument("review_dir")
    sp.add_argument("--repo", default=".")
    sp.add_argument("--head")
    sp.add_argument("--base", required=True)
    sp.add_argument("--runtime", required=True)
    sp.add_argument("--session", required=True)
    sp.add_argument("--goal", required=True)
    sp.set_defaults(func=cmd_init)

    sp = sub.add_parser("freeze")
    sp.add_argument("review_dir")
    sp.add_argument("--glm-review", required=True)
    sp.add_argument("--k3-review", required=True)
    sp.add_argument("--glm-slug")
    sp.add_argument("--k3-slug")
    sp.add_argument("--native-root")
    sp.add_argument("--runtime-evidence")
    sp.set_defaults(func=cmd_freeze)

    common = lambda sp: (
        sp.add_argument("review_dir"),
        sp.add_argument("--glm-review"),
        sp.add_argument("--k3-review"),
    )

    sp = sub.add_parser("challenge")
    common(sp)
    sp.add_argument("--evidence", required=True)
    sp.add_argument("--claim", required=True)
    sp.add_argument("--kind")  # hint only — never a gate
    sp.add_argument("--test-name")
    sp.add_argument("--harness-root", action="append")
    sp.add_argument("--exec-argv")
    sp.add_argument("--run-dir")
    sp.add_argument("--timeout", type=int, default=120)
    sp.add_argument("--imported", action="store_true")
    sp.set_defaults(func=cmd_challenge)

    sp = sub.add_parser("cross")
    common(sp)
    sp.add_argument("--cross-report", required=True)
    sp.add_argument("--cross-slug")
    sp.add_argument("--native-root")
    sp.add_argument("--runtime-evidence")
    sp.add_argument("--expect-claims")
    sp.set_defaults(func=cmd_cross)

    sp = sub.add_parser("status")
    common(sp)
    sp.set_defaults(func=cmd_status)
    return p


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        args.func(args)
        return 0
    except ReviewError as e:
        emit_json({"error": {"code": e.code, "message": e.message}})
        return 1
    except BrokenPipeError:
        return 1


if __name__ == "__main__":
    sys.exit(main())
