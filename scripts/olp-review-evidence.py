#!/usr/bin/env python3
"""olp-review-evidence — OctoLoop 行为证据互审流程管理入口.

Spec: specs/task-evo-review-evidence.spec.md (23 scenarios)
Plan: docs/superpowers/plans/2026-09-09-review-evidence.md

子命令:
  init     — 初始化评审目录(HEAD 真实解析 + runtime/session/goal 复合身份)
  freeze   — 冻结两份独立初审(native result-N/turns.txt 同轮严格 completed +
             originator/goal 身份绑定 + 报告 turn 恰等 native 最新 + 报告
             session/goal/runtime/repo 与调用视角绑定 + --head 锚定 +
             claims 块 + 事务锁)
  challenge— 行为证据。唯一 live 入口: --live-cargo 由本入口实时执行固定的
             生产 adapter(scripts/olp-review-evidence-cargo.py);外部 JSON
             receipt 字段不是执行证明;--imported 一律 not-replayed;
             --exec-argv 任意执行体一律拒绝(executor-not-trusted)
  cross    — 收录交叉报告(原 reviewer slug + 新 native completed 轮次 +
             HEAD 绑定 + 结构化 cross_claims 逐 claim accept/refute/pending;
             子串包含不算覆盖;refute 需已验证新行为证据或显式
             --allow-operator-refute 人工裁决)
  status   — 汇总判词状态(claim 级两层词表 + review_accepted 收口标志;
             review_accepted 需全部冻结 claim 有可核对证据且两个原
             reviewer 各自新 native completed cross)

错误输出一律可解析 JSON: {"error": {"code": "...", "message": "..."}} 且 exit != 0.
冻结/manifest 写入: per-review-dir flock 包住 load→validate→save(事务),
落盘用临时文件 + os.replace(原子),并发/失败不出现半写状态。

v5(evidence-core-k3-rescue): 外层六反例最终门
(../outer-evidence-final-gate-probes.json 0/6 → 本实现修复):
1. turns.txt 同轮 outcome 与 result-N 冲突(errored)→ freeze 拒绝;
2. 报告 turn 为未来轮次(> native 最新编号)→ turn-mismatch 拒绝;
3. native originator/goal 属于别的 master/goal → peer-authority-mismatch;
4. 手写 JSON receipt(无执行)→ evidence-not-executed,challenge 不接纳;
5. 伪造 receipt + --imported → 一律 not-replayed,--imported 不旁路;
6. 纯文字 cross("人工裁决"等)+ 未挑战 claim → missing-claim-coverage /
   review 不收口,已复现失败不得被文字改判 challenge-refuted。
"""
from __future__ import annotations

import argparse
import contextlib
import fcntl
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
LOCK_FILENAME = ".review-state.lock"
REVIEWER_LANES = ("glm", "k3")

# cross claim 裁决词表(结构化 cross_claims 块)
CROSS_VERDICTS = ("accept", "refute", "pending")
# 由本入口真实执行得出的判词依据(review_accepted 的"可核对证据")
EXECUTED_REASONS = ("executed-probe-passed", "challenge-evidence-executed")

# --format human: main() 设置,emit_json 切换为人类渲染(非 JSON 信封)。
CLI_HUMAN_FORMAT = False

# ---------------------------------------------------------------------------
# error handling: structured JSON on stdout, non-zero exit
# ---------------------------------------------------------------------------


class ReviewError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


def emit_json(obj: dict) -> None:
    if CLI_HUMAN_FORMAT:
        sys.stdout.write(render_human(obj) + "\n")
        return
    json.dump(obj, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


def fail(code: str, message: str) -> "ReviewError":
    return ReviewError(code, message)


# ---------------------------------------------------------------------------
# state store (transactional: flock around load→validate→save; atomic write)
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


@contextlib.contextmanager
def review_lock(review_dir: Path):
    """Per-review-dir 排他锁:包住 load→validate→save 全事务。"""
    review_dir.mkdir(parents=True, exist_ok=True)
    fd = os.open(review_dir / LOCK_FILENAME, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


# ---------------------------------------------------------------------------
# frontmatter parsing (review reports)
# ---------------------------------------------------------------------------

_FM_RE = re.compile(r"\A---\s*\n(.*?)\n---\s*\n", re.S)
_CLAIMS_BLOCK_RE = re.compile(r"```json\s*\n(.*?)```", re.S)


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


def _json_block(path: Path) -> dict | None:
    """报告正文首个 ```json 块解析为 dict;块非法 JSON → claims-block-invalid。"""
    text = path.read_text(encoding="utf-8", errors="replace")
    m = _CLAIMS_BLOCK_RE.search(text)
    if not m:
        return None
    try:
        obj = json.loads(m.group(1))
    except json.JSONDecodeError as e:
        raise fail("claims-block-invalid", f"{path.name} claims 块 JSON 非法: {e}")
    return obj if isinstance(obj, dict) else None


def parse_claims(path: Path) -> list[dict]:
    """解析初审报告的显式 JSON claims 块: {"claims": [{"id","verdict","evidence"}]}.

    无块返回 [];块存在但非 claims 结构 → claims-block-invalid。
    """
    obj = _json_block(path)
    if obj is None:
        return []
    claims = obj.get("claims")
    if not isinstance(claims, list) or not claims:
        raise fail("claims-block-invalid", f"{path.name} claims 块缺非空 claims 数组")
    for c in claims:
        if not isinstance(c, dict) or not isinstance(c.get("id"), str) or not c["id"]:
            raise fail("claims-block-invalid", f"{path.name} claim 缺合法 id")
    return claims


def parse_cross_claims(path: Path) -> list[dict] | None:
    """解析 cross 报告的结构化 cross_claims 块.

    无块或无 cross_claims 键 → None(由覆盖校验报 missing-claim-coverage);
    cross_claims 存在但形态非法 → cross-claim-invalid。每个条目必须含
    合法 id 与 verdict(accept/refute/pending);reference 为可选 dict。
    子串包含不算覆盖 — 只认此结构化块。
    """
    obj = _json_block(path)
    if obj is None:
        return None
    claims = obj.get("cross_claims")
    if claims is None:
        return None
    if not isinstance(claims, list) or not claims:
        raise fail("cross-claim-invalid", f"{path.name} cross_claims 缺非空数组")
    for c in claims:
        if not isinstance(c, dict) or not isinstance(c.get("id"), str) or not c["id"]:
            raise fail("cross-claim-invalid", f"{path.name} cross claim 缺合法 id")
        if c.get("verdict") not in CROSS_VERDICTS:
            raise fail(
                "cross-claim-invalid",
                f"{path.name} claim {c['id']} verdict={c.get('verdict')} 非法"
                f"(须为 {CROSS_VERDICTS})",
            )
        ref = c.get("reference")
        if ref is not None and not isinstance(ref, dict):
            raise fail("cross-claim-invalid", f"{path.name} claim {c['id']} reference 须为对象")
    return claims


def resolve_head(repo: Path) -> str:
    """解析 repo 真实 HEAD;失败 fail-closed,不回退到调用方自报。"""
    try:
        out = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=str(repo),
            capture_output=True, text=True, timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        out = None
    head = (out.stdout.strip() if out else "")
    if out and out.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}", head):
        return head
    raise fail("head-unresolvable", f"无法解析 repo 真实 HEAD: {repo}")


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


def turns_index(peer_dir: Path) -> dict[int, str]:
    """turns.txt 轮次索引: {turn: outcome}(每行 '<turn> <outcome> ...')。"""
    p = peer_dir / "turns.txt"
    idx: dict[int, str] = {}
    if not p.exists():
        return idx
    for line in p.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split()
        if parts and parts[0].isdigit():
            idx[int(parts[0])] = parts[1] if len(parts) > 1 else ""
    return idx


def _identity_file_matches(peer_dir: Path, name: str, expected: str) -> bool:
    p = peer_dir / name
    if not p.exists():
        return False
    try:
        return p.read_text(encoding="utf-8", errors="replace").strip() == expected
    except OSError:
        return False


def check_peer_validity(
    fm: dict,
    report_path: Path,
    native_dir: Path | None,
    runtime_evidence: dict | None,
    slug: str,
    require_authority: bool = False,
    min_turn_exclusive: int = 0,
    expected_session: str | None = None,
    expected_goal: str | None = None,
) -> bool:
    """Reject pending/errored/stale/foreign peers; external signals veto self-report.

    正向证明完整才采信(fail-closed),不是只排除 errored:
    - 报告自报 outcome 必须精确 completed;
    - native 最高编号 result-N: slug 匹配、outcome 精确 completed、
      frontmatter turn 恰等编号 N;
    - turns.txt 必须存在且同轮 N 精确 completed(缺项/冲突/未来轮拒绝);
    - 报告 turn 恰等 native 最新编号 N(< N stale-turn,> N turn-mismatch);
    - native originator/goal 文件必须与调用视角 session/goal 精确绑定
      (任意外来 native-root 不得冒充 → peer-authority-mismatch);
    - runtime-evidence 终止快照须 session 匹配当前视角;
    - cross 阶段报告 turn 必须晚于冻结轮次(min_turn_exclusive)。
    """
    outcome = fm.get("outcome")
    if outcome != "completed":
        raise fail(
            "peer-outcome-invalid",
            f"{slug} 自报 outcome={outcome}(非 completed),不得作为有效初审/交叉",
        )
    has_authority = False
    # External signal 1: runtime-evidence(终止快照须绑定当前 session 视角)
    if runtime_evidence is not None:
        peers = runtime_evidence.get("peers")
        if peers is not None:
            for peer in peers:
                if peer.get("slug") == slug:
                    if peer.get("active_thread"):
                        raise fail(
                            "peer-outcome-invalid",
                            f"{slug} runtime-evidence active_thread 非空"
                            "(仍有活跃轮次),报告声明不可采信",
                        )
                    if (
                        peer.get("status") == "terminated"
                        and expected_session is not None
                        and peer.get("session") == expected_session
                        and (
                            peer.get("goal") is None
                            or peer.get("goal") == expected_goal
                        )
                    ):
                        has_authority = True
    # External signal 2: native result-N.md(严格对账)
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
            if native_outcome != "completed":
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} native 最高编号 {path.name} outcome={native_outcome}"
                    "(非 completed),不构成终止权威",
                )
            native_turn = native_fm.get("turn")
            if native_turn is None or not str(native_turn).isdigit() or int(native_turn) != n:
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} native {path.name} turn={native_turn} 与编号 {n} 不一致",
                )
            idx = turns_index(native_dir)
            if not idx:
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} native turns.txt 缺失/为空,轮次索引不完整",
                )
            mx = max(idx)
            if n < mx:
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} native 最高 result-{n} 低于 turns.txt 已结束轮次 {mx}"
                    "(turns 索引不严格)",
                )
            if n not in idx:
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} turns.txt 缺最新轮 {n} 的记录(缺项不构成终止权威)",
                )
            if idx[n] != "completed":
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} turns.txt 第 {n} 轮 outcome={idx[n]} 与 {path.name}"
                    " completed 冲突(同轮必须精确 completed)",
                )
            turn = fm.get("turn")
            if turn is None or not str(turn).isdigit():
                raise fail(
                    "peer-outcome-invalid",
                    f"{slug} 报告 turn 缺失/非法,无法与 native 权威对账",
                )
            if int(turn) < n:
                raise fail(
                    "stale-turn",
                    f"{slug} 报告 turn={turn} 低于 native 最新轮次 {n}",
                )
            if int(turn) > n:
                raise fail(
                    "turn-mismatch",
                    f"{slug} 报告 turn={turn} 超过 native 最新轮次 {n}(未来轮次)",
                )
            # originator/goal 身份绑定: 外来 native-root 不得冒充本视角
            if expected_session is not None and not _identity_file_matches(
                native_dir, "originator", expected_session
            ):
                raise fail(
                    "peer-authority-mismatch",
                    f"{slug} native originator 与当前 session 视角不符/缺失"
                    "(外来 native-root 不得冒充)",
                )
            if expected_goal is not None and not _identity_file_matches(
                native_dir, "goal", expected_goal
            ):
                raise fail(
                    "peer-authority-mismatch",
                    f"{slug} native goal 与当前 goal 不符/缺失"
                    "(跨 goal 收据不得作权威)",
                )
            has_authority = True
    turn = fm.get("turn")
    if (
        min_turn_exclusive > 0
        and turn is not None
        and str(turn).isdigit()
        and int(turn) <= min_turn_exclusive
    ):
        raise fail(
            "stale-turn",
            f"{slug} 报告 turn={turn} 不晚于冻结轮次 {min_turn_exclusive}"
            "(旧初审不得冒充 cross)",
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
_ANCHOR_PANIC = re.compile(r"panicked at [^\s:]+:\d+:\d+")
_ANCHOR_SUMMARY = re.compile(r"test result: FAILED\..*\d+ failed")
_PASS_SUMMARY = re.compile(r"test result: ok\.", re.M)


def structural_anchors(ev_path: Path, test_name: str) -> list[str]:
    """Return list of missing structural anchors for a behavioral log."""
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


def production_adapter_path() -> Path:
    """固定的生产 Cargo adapter(本入口唯一可执行体,路径不可由调用方指定)。"""
    return Path(__file__).resolve().with_name("olp-review-evidence-cargo.py")


def load_execution_receipt(ev_path: Path, state_head: str) -> tuple[dict, str, str]:
    """校验本入口刚执行生产 adapter 落盘的 receipt 并做身份绑定校验.

    只用于 --live-cargo 实时执行后的自产 receipt 复核;外部传入的 JSON
    receipt 永不进入本函数(cmd_challenge 前置拒绝/分层 not-replayed)。
    Returns (receipt, observed, selector)。
    """
    try:
        rc = json.loads(ev_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        raise fail("receipt-invalid", f"{ev_path.name} 非合法 JSON,不是执行 receipt")
    if not isinstance(rc, dict) or rc.get("receipt_kind") != "cargo-test-execution":
        raise fail(
            "receipt-invalid",
            f"{ev_path.name} 缺 receipt_kind=cargo-test-execution"
            "(非生产 cargo adapter 执行回执)",
        )
    observed = rc.get("observed")
    if observed not in ("pass", "fail"):
        raise fail(
            "receipt-not-conclusive",
            f"receipt observed={observed} 非终态(pass/fail),不得作行为证据",
        )
    head_before = rc.get("head_before")
    if state_head and re.fullmatch(r"[0-9a-f]{40}", state_head or ""):
        if head_before != state_head:
            raise fail(
                "receipt-head-mismatch",
                f"receipt head_before={head_before} != 评审 HEAD={state_head}",
            )
    if rc.get("head_after") != head_before:
        raise fail(
            "receipt-head-mismatch",
            "执行期间 HEAD 发生变化(head_before != head_after),树不一致",
        )
    if not rc.get("stdout_sha256") or rc.get("exit_code") is None:
        raise fail("receipt-invalid", "receipt 缺输出摘要/退出码绑定")
    selector = rc.get("selector") or ""
    return rc, observed, selector


def run_live_adapter(review_dir: Path, state: dict, args: argparse.Namespace, claim: str) -> dict:
    """--live-cargo: 由本入口实时执行固定的生产 Cargo adapter.

    可靠解析 repo/manifest/精确 selector/HEAD/source 与完整输出工件/退出码
    均由 adapter 保证(其语义经外层 8 真实 PR 探针重放验证,SHA 2e17a359);
    本函数只做参数转发、deadline 约束与自产 receipt 复核,不引入任何
    通用任意 shell 执行旁路。
    """
    adapter = production_adapter_path()
    if not adapter.exists():
        raise fail("adapter-missing", f"生产 cargo adapter 缺失: {adapter}")
    if not args.selector:
        raise fail("live-args-missing", "--live-cargo 需 --selector 精确测试名")
    if args.expect not in ("pass", "fail"):
        raise fail("live-args-missing", "--live-cargo 需 --expect pass|fail(显式预期)")
    repo = Path(args.exec_repo).resolve() if args.exec_repo else Path(state["repo"])
    if not repo.is_dir():
        raise fail("repo-missing", f"live 执行 repo 不存在: {repo}")
    ev_dir = review_dir / "live-evidence"
    ev_dir.mkdir(parents=True, exist_ok=True)
    stamp = f"{claim}-{int(time.time())}-{os.getpid()}"
    receipt_path = ev_dir / f"{stamp}.receipt.json"
    argv = [
        sys.executable or "python3",
        str(adapter),
        "--repo", str(repo),
        "--selector", args.selector,
        "--expect", args.expect,
        "--receipt", str(receipt_path),
        "--artifact-dir", str(ev_dir),
        "--timeout", str(args.timeout),
    ]
    if args.lib:
        argv.append("--lib")
    if args.test_target:
        argv += ["--test-target", args.test_target]
    if args.manifest:
        argv += ["--manifest", args.manifest]
    if args.cargo_target_dir:
        argv += ["--target-dir", args.cargo_target_dir]
    try:
        proc = subprocess.run(
            argv, capture_output=True, text=True, timeout=args.timeout + 120,
        )
    except FileNotFoundError:
        raise fail("adapter-missing", f"无法执行生产 adapter: {argv[0]}")
    except subprocess.TimeoutExpired:
        raise fail(
            "evidence-not-executed",
            f"adapter 执行超时(>{args.timeout + 120}s),已回收子进程",
        )
    expectation_violated = False
    if proc.returncode != 0:
        envelope = {}
        try:
            envelope = json.loads(proc.stdout or "")
        except json.JSONDecodeError:
            envelope = {}
        err = envelope.get("error") if isinstance(envelope, dict) else None
        code = (err or {}).get("code") or "adapter-failed"
        if code == "expectation-violated" and receipt_path.exists():
            # 真实执行但结果与预期相反 — 仍是有意义的行为证据(如实标记)
            expectation_violated = True
        else:
            raise fail(
                "evidence-not-executed",
                f"生产 adapter 未产生可信终态 [{code}]: {(err or {}).get('message', '')}",
            )
    if not receipt_path.exists():
        raise fail("evidence-not-executed", "adapter 未落 receipt,无执行证明")
    rc, observed, selector = load_execution_receipt(receipt_path, state.get("head") or "")
    if selector != args.selector:
        raise fail(
            "receipt-invalid",
            f"receipt selector={selector} 与请求 {args.selector} 不一致",
        )
    return {
        "receipt_path": receipt_path,
        "receipt": rc,
        "observed": observed,
        "expectation_violated": expectation_violated,
    }


# ---------------------------------------------------------------------------
# verdict state machine
# ---------------------------------------------------------------------------

VERDICT_STATES = (
    "approve",
    "flipped",
    "blocked-on-evidence",
    "pending-behavioral-evidence",
    "unverified",
    "not-replayed",
    "challenge-refuted",
)


def claim_evidence_backed(verdict: dict) -> bool:
    """判词是否有可核对证据(本入口真实执行或有效反驳依据)。"""
    if verdict.get("state") == "challenge-refuted":
        return True
    return verdict.get("reason") in EXECUTED_REASONS


def record_challenge(state: dict, claim: str, rec: dict) -> None:
    """每个冻结 claim 独立保存证据/执行结果(latest + history)。"""
    slot = state.setdefault("challenges", {}).setdefault(claim, {"history": []})
    slot["latest"] = rec
    slot.setdefault("history", []).append(rec)
    if rec.get("accepted"):
        # 兼容字段: 最近一次被接纳的挑战(旧测试/外层 status 依赖)
        state["challenge"] = rec


def apply_live_verdict(state: dict, claim: str, live: dict) -> str:
    """按 live 执行结果推进判词状态机,返回新状态。"""
    prev = state["verdicts"].get(claim, {}).get("state")
    observed = live["observed"]
    rc = live["receipt"]
    if observed == "fail":
        new_state, reason = "blocked-on-evidence", "challenge-evidence-executed"
    elif prev in ("blocked-on-evidence", "flipped"):
        # 已验证新行为证据: 实时重执行通过,反驳已复现失败
        new_state, reason = "challenge-refuted", "executed-probe-passed"
    else:
        new_state, reason = "approve", "executed-probe-passed"
    rec = {
        "accepted": True,
        "claim": claim,
        "observed": observed,
        "expectation_violated": live["expectation_violated"],
        "receipt": str(live["receipt_path"]),
        "receipt_sha256": sha256_file(live["receipt_path"]),
        "executed": {
            "adapter": rc.get("adapter"),
            "argv": rc.get("argv"),
            "exit_code": rc.get("exit_code"),
            "stdout_sha256": rc.get("stdout_sha256"),
            "stderr_sha256": rc.get("stderr_sha256"),
            "head_before": rc.get("head_before"),
            "head_after": rc.get("head_after"),
            "run_dir": rc.get("cwd"),
            "selector": rc.get("selector"),
            "selector_qualified": rc.get("selector_qualified"),
            "test_target": rc.get("test_target"),
            "manifest_sha256": rc.get("manifest_sha256"),
            "test_target_sha256": rc.get("test_target_sha256"),
            "artifacts": rc.get("artifacts"),
        },
        "executed_unix": int(time.time()),
        "result": "probe-failed" if observed == "fail" else "probe-passed",
    }
    record_challenge(state, claim, rec)
    verdict = {
        "state": new_state,
        "reason": reason,
        "evidence": rec["receipt"],
    }
    if new_state == "challenge-refuted":
        verdict["refuted_by"] = "live-reexecution"
    state["verdicts"][claim] = verdict
    return new_state


# ---------------------------------------------------------------------------
# subcommands
# ---------------------------------------------------------------------------


def cmd_init(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    with review_lock(review_dir):
        state = load_state(review_dir)
        if state:
            raise fail("already-initialized", f"{review_dir} 已初始化")
        # HEAD 真实解析 fail-closed;--head 仅作显式锚定交叉校验。
        head = resolve_head(Path(args.repo))
        if args.head and args.head != head:
            raise fail(
                "head-mismatch",
                f"--head={args.head} 与 repo 真实 HEAD={head} 不一致",
            )
        state = {
            "protocol": PROTOCOL,
            "head": head,
            "base": args.base,
            "runtime": args.runtime,
            "session": args.session,
            "goal": args.goal,
            "repo": str(Path(args.repo).resolve()),
            "reviews": {},
            "frozen": False,
            "challenge": None,
            "challenges": {},
            "cross": [],
            "verdicts": {},
        }
        save_state(review_dir, state)
        emit_json({"ok": True, "state": "initialized", "head": state["head"]})


def _check_report_identity(fm: dict, label: str, state: dict) -> None:
    """初审 frontmatter 身份与调用视角绑定(session/goal 必备;runtime/repo 在则验)。"""
    for key in ("session", "goal"):
        expected = state.get(key)
        v = fm.get(key)
        if not v:
            raise fail(
                "report-identity-missing",
                f"{label} 初审缺 {key} frontmatter(身份缺失不得绕过校验)",
            )
        if expected and v != expected:
            raise fail(
                "report-identity-mismatch",
                f"{label} 初审 {key}={v} != 评审视角 {expected}",
            )
    for key in ("runtime", "repo"):
        expected = state.get(key)
        v = fm.get(key)
        if v and expected:
            if str(Path(v).resolve()) != str(Path(expected).resolve()):
                raise fail(
                    "report-identity-mismatch",
                    f"{label} 初审 {key}={v} != 评审视角 {expected}",
                )


def cmd_freeze(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    with review_lock(review_dir):
        state = load_state(review_dir)
        if not state:
            raise fail("not-initialized", "先 init")
        if state.get("frozen"):
            raise fail("already-frozen", "初审已冻结")

        # --head 显式锚定(不可省略),且必须与 init 基线一致。
        if not args.head:
            raise fail("head-anchor-missing", "freeze 需显式 --head 锚定评审基线")
        if state.get("head") and args.head != state["head"]:
            raise fail(
                "head-mismatch",
                f"--head={args.head} != 评审 HEAD={state['head']}",
            )

        glm_path, k3_path = Path(args.glm_review), Path(args.k3_review)
        for label, p in (("glm", glm_path), ("k3", k3_path)):
            if not p.exists():
                raise fail("first-reviews-incomplete", f"{label} 初审文件缺失: {p}")

        glm_slug, k3_slug = args.glm_slug or "glm", args.k3_slug or "k3"
        if glm_slug == k3_slug or glm_path.resolve() == k3_path.resolve():
            raise fail(
                "duplicate-reviewer",
                "两份初审必须来自不同评审方(同 peer/同文件不得计为两独立报告)",
            )

        glm_fm, k3_fm = parse_frontmatter(glm_path), parse_frontmatter(k3_path)
        native_root = Path(args.native_root) if args.native_root else None
        runtime_evidence = None
        if args.runtime_evidence and Path(args.runtime_evidence).exists():
            runtime_evidence = json.loads(Path(args.runtime_evidence).read_text())

        claims_by_lane: dict[str, list[dict]] = {}
        for label, p, fm, slug in (
            ("glm", glm_path, glm_fm, glm_slug),
            ("k3", k3_path, k3_fm, k3_slug),
        ):
            check_peer_validity(
                fm,
                p,
                (native_root / slug) if native_root else None,
                runtime_evidence,
                slug,
                require_authority=True,
                expected_session=state.get("session"),
                expected_goal=state.get("goal"),
            )
            head = fm.get("HEAD") or fm.get("head")
            if not head:
                raise fail(
                    "report-head-missing",
                    f"{label} 初审缺 HEAD frontmatter(缺失不得绕过校验)",
                )
            if args.head != head:
                raise fail(
                    "head-mismatch",
                    f"{label} 初审 HEAD={head} != --head={args.head}",
                )
            _check_report_identity(fm, label, state)
            claims = parse_claims(p)
            if not claims:
                raise fail(
                    "claims-block-missing",
                    f"{label} 初审缺显式 JSON claims 块(冻结即确立全部初始 claim ID)",
                )
            claims_by_lane[label] = claims

        # 冻结即确立全部初始 claim ID(两 lane 并集,记录 lane 归属)
        verdicts: dict[str, dict] = {}
        claim_lanes: dict[str, list[str]] = {}
        for label, claims in claims_by_lane.items():
            for c in claims:
                cid = c["id"]
                claim_lanes.setdefault(cid, []).append(label)
                if cid not in verdicts:
                    verdicts[cid] = {
                        "state": c.get("verdict", "approve"),
                        "evidence": c.get("evidence", []),
                        "lanes": claim_lanes[cid],
                    }

        # freeze: 记录复合身份并翻转标志 — 同事务一次落盘
        state["reviews"] = {
            "glm": {
                "path": str(glm_path),
                "sha256": glm_fm["__sha256__"],
                "peer": glm_slug,
                "turn": glm_fm.get("turn"),
                "outcome": glm_fm.get("outcome"),
            },
            "k3": {
                "path": str(k3_path),
                "sha256": k3_fm["__sha256__"],
                "peer": k3_slug,
                "turn": k3_fm.get("turn"),
                "outcome": k3_fm.get("outcome"),
            },
        }
        state["verdicts"] = verdicts
        state["frozen"] = True
        state["frozen_at_unix"] = int(time.time())
        save_state(review_dir, state)
        emit_json({"ok": True, "state": "frozen", "reviews": state["reviews"],
                   "claims": sorted(verdicts)})


def require_frozen(state: dict) -> None:
    if not state.get("frozen"):
        raise fail("not-frozen", "初审未冻结")


def verify_no_tamper(state: dict) -> None:
    for label in REVIEWER_LANES:
        rec = state["reviews"].get(label)
        if not rec:
            continue
        p = Path(rec["path"])
        if not p.exists():
            # fail-closed: 冻结后初审文件被删除与被改写同罪
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
    with review_lock(review_dir):
        state = load_state(review_dir)
        if not state:
            raise fail("not-initialized", "先 init")
        require_frozen(state)
        verify_no_tamper(state)

        claim = args.claim
        if not claim:
            raise fail("claim-required", "challenge 需 --claim 判词 id")
        # 冻结即确立全部 claim ID — 禁止挑战未冻结/新增 claim
        if claim not in state.get("verdicts", {}):
            raise fail(
                "claim-not-frozen",
                f"claim {claim} 不在冻结 claims 集合中(禁止新增未冻结 claim)",
            )
        if args.live_cargo and args.imported:
            raise fail("args-conflict", "--live-cargo 与 --imported 互斥")

        # imported 分层(先于一切内容/JSON 判定): 外部导入的 receipt/log
        # 一律 not-replayed,--imported 不能旁路执行门。
        if args.imported:
            if not args.evidence:
                raise fail("evidence-missing", "--imported 需 --evidence 导入文件")
            ev_path = Path(args.evidence)
            if not ev_path.exists():
                raise fail("evidence-missing", f"证据文件不存在: {ev_path}")
            state["verdicts"][claim] = {
                "state": "not-replayed",
                "reason": "imported-evidence-not-replayed",
                "evidence": str(ev_path),
            }
            record_challenge(
                state,
                claim,
                {
                    "accepted": False,
                    "imported": True,
                    "claim": claim,
                    "evidence": str(ev_path),
                    "executed_unix": int(time.time()),
                },
            )
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

        # 唯一 live 入口: 本入口实时执行固定的生产 Cargo adapter
        if args.live_cargo:
            live = run_live_adapter(review_dir, state, args, claim)
            new_state = apply_live_verdict(state, claim, live)
            save_state(review_dir, state)
            emit_json(
                {
                    "ok": True,
                    "state": new_state,
                    "claim": claim,
                    "observed": live["observed"],
                    "executed": {
                        "exit_code": live["receipt"].get("exit_code"),
                        "receipt": str(live["receipt_path"]),
                    },
                }
            )
            return

        # 非 live 路径: 只承担负向判定 — 任何现成文件都不是执行证明
        if not args.evidence:
            raise fail(
                "evidence-missing",
                "challenge 需 --live-cargo 实时执行,或 --evidence 配合 --imported 分层",
            )
        ev_path = Path(args.evidence)
        if not ev_path.exists():
            raise fail("evidence-missing", f"证据文件不存在: {ev_path}")
        body = ev_path.read_text(encoding="utf-8", errors="replace")

        # 外部 JSON 执行回执: receipt_kind 字符串不是执行证明(外层反例 4/5)
        if ev_path.suffix == ".json":
            try:
                probe = json.loads(body)
            except json.JSONDecodeError:
                probe = None
            if isinstance(probe, dict) and probe.get("receipt_kind"):
                raise fail(
                    "evidence-not-executed",
                    "外部执行回执(receipt_kind)不作执行证明: 行为证据只认本入口 "
                    "--live-cargo 实时执行;外层导入请用 --imported(判 not-replayed)",
                )

        # Gate 1 (cheap, negative): doc-only content 永不被采信
        doc_like = (
            _ANCHOR_PANIC.search(body) is None
            and _ANCHOR_SUMMARY.search(body) is None
            and _PASS_SUMMARY.search(body) is None
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
            state["verdicts"][claim] = {
                "state": "unverified",
                "reason": "evidence-not-behavioral",
            }
            save_state(review_dir, state)
            raise fail("evidence-not-behavioral", "; ".join(missing))

        harness_roots = [Path(p) for p in (args.harness_root or []) if p]
        if args.test_name and harness_roots:
            if not test_name_resolvable(args.test_name, harness_roots):
                raise fail(
                    "evidence-not-behavioral",
                    f"测试名 {args.test_name} 无法在 harness 源解析到定义",
                )

        # Gate 3: --exec-argv 任意执行体一律拒绝(python 打印 cargo 样式文本、
        # /usr/bin/false 等均不作执行证明)。唯一 live 入口是 --live-cargo。
        if args.exec_argv:
            executor = Path(args.exec_argv.split()[0]).name if args.exec_argv.split() else ""
            raise fail(
                "executor-not-trusted",
                f"执行体 {executor or args.exec_argv} 不可信: --exec-argv 不作行为证据;"
                " 唯一 live 入口为 challenge --live-cargo(本入口实时执行固定的"
                " 生产 adapter scripts/olp-review-evidence-cargo.py)",
            )

        # Gate 4: 结构形似不构成执行证明
        raise fail(
            "evidence-not-executed",
            "结构锚齐全但不构成执行证明: 需 --live-cargo 由本入口实时执行生产 adapter",
        )


def cmd_cross(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    with review_lock(review_dir):
        state = load_state(review_dir)
        if not state:
            raise fail("not-initialized", "先 init")
        require_frozen(state)
        verify_no_tamper(state)
        chs = state.get("challenges") or {}
        any_accepted = any(
            (slot.get("latest") or {}).get("accepted") for slot in chs.values()
        )
        if not any_accepted:
            raise fail("challenge-pending", "挑战证据尚未接纳,不得收录 cross")

        report = Path(args.cross_report)
        if not report.exists():
            raise fail("cross-missing", f"cross 报告不存在: {report}")
        fm = parse_frontmatter(report)
        native_root = Path(args.native_root) if args.native_root else None
        slug = args.cross_slug or ""
        runtime_evidence = None
        if args.runtime_evidence and Path(args.runtime_evidence).exists():
            runtime_evidence = json.loads(Path(args.runtime_evidence).read_text())

        # cross 必须来自冻结的原 reviewer(各自新的 native completed 轮次)
        reviewers = {
            r.get("peer") for r in state.get("reviews", {}).values() if r.get("peer")
        }
        if slug not in reviewers:
            raise fail(
                "cross-reviewer-unknown",
                f"cross slug={slug or '(missing)'} 不是冻结的原 reviewer {sorted(reviewers)}",
            )

        # HEAD 绑定(与初审同一基线,缺失不得绕过)
        head = fm.get("HEAD") or fm.get("head")
        if not head:
            raise fail(
                "report-head-missing",
                "cross 报告缺 HEAD frontmatter(缺失不得绕过校验)",
            )
        if state.get("head") and head != state["head"]:
            raise fail(
                "head-mismatch",
                f"cross 报告 HEAD={head} != 评审 HEAD={state['head']}",
            )

        # 冻结轮下限: 旧 turn1 初审不得冒充 cross turn2;报告 turn 恰等
        # native 最新编号且 originator/goal 身份绑定(与 freeze 同一严格度)
        frozen_turns = [
            int(r["turn"])
            for r in state["reviews"].values()
            if str(r.get("turn") or "").isdigit()
        ]
        min_turn_exclusive = max(frozen_turns, default=0)
        check_peer_validity(
            fm,
            report,
            (native_root / slug) if native_root else None,
            runtime_evidence,
            slug,
            require_authority=True,
            min_turn_exclusive=min_turn_exclusive,
            expected_session=state.get("session"),
            expected_goal=state.get("goal"),
        )

        # 结构化逐 claim 覆盖: 冻结确立的全部 claim ID 必须逐项出现于
        # cross_claims 块(子串包含不算覆盖);--expect-claims 只能扩大校验
        entries = parse_cross_claims(report) or []
        frozen_claims = set(state["verdicts"].keys())
        required = set(frozen_claims)
        if args.expect_claims:
            required |= {c for c in args.expect_claims.split(",") if c}
        covered = {c["id"] for c in entries}
        missing = sorted(required - covered)
        if missing:
            raise fail(
                "missing-claim-coverage",
                f"cross 报告未在结构化 cross_claims 块覆盖判词: {missing}",
            )
        extra = sorted(covered - required)
        if extra:
            raise fail(
                "claim-not-frozen",
                f"cross 引入未冻结 claim: {extra}(冻结即确立全部 claim ID)",
            )

        # refute 回边: 只能基于已验证新行为证据(本入口 live 执行记录)或
        # 显式可审计 operator 决定(--allow-operator-refute);缺有效依据
        # 保持 pending/原失败,不得凭字符串标记升格。
        refutations = []
        for c in entries:
            cid = c["id"]
            if cid not in state["verdicts"] or c["verdict"] != "refute":
                continue
            cur = state["verdicts"][cid].get("state")
            if cur not in ("flipped", "blocked-on-evidence", "challenge-refuted"):
                raise fail(
                    "refutation-without-challenge",
                    f"claim {cid} 当前状态 {cur},无已复现失败可反驳",
                )
            ref = c.get("reference") or {}
            kind = ref.get("kind")
            substantiated = False
            if kind == "executed-evidence":
                want = ref.get("ref") or ""
                history = (state.get("challenges", {}).get(cid) or {}).get("history", [])
                substantiated = any(
                    h.get("accepted")
                    and h.get("observed") == "pass"
                    and h.get("receipt_sha256") == want
                    for h in history
                )
            elif kind == "operator-decision":
                substantiated = bool(
                    args.allow_operator_refute and ref.get("operator") and ref.get("note")
                )
            if not substantiated:
                raise fail(
                    "refutation-unsubstantiated",
                    f"claim {cid} refute 缺有效依据: 需已验证新行为证据"
                    "(本入口 live PASS 执行记录 SHA)或显式 --allow-operator-refute"
                    " 的 operator 决定(operator+note 齐全)",
                )
            state["verdicts"][cid] = {
                **state["verdicts"][cid],
                "state": "challenge-refuted",
                "refuted_by": slug,
                "refutation_reference": ref,
            }
            refutations.append(cid)

        state.setdefault("cross", []).append(
            {
                "slug": slug,
                "sha256": fm["__sha256__"],
                "turn": fm.get("turn"),
                "refuted": refutations,
            }
        )
        save_state(review_dir, state)
        emit_json({"ok": True, "state": "cross-recorded", "refuted": refutations})


def cross_completed_slugs(state: dict) -> set[str]:
    return {c.get("slug") for c in state.get("cross", []) if c.get("slug")}


def any_challenge_accepted(state: dict) -> bool:
    chs = state.get("challenges") or {}
    return any((slot.get("latest") or {}).get("accepted") for slot in chs.values())


def review_accepted(state: dict) -> bool:
    """收口条件: 冻结 + ≥1 已接纳挑战 + 两个原 reviewer 各自新 native
    completed cross + 全部冻结 claim 有可核对证据(真实执行或有效反驳)。"""
    if not (state.get("frozen") and any_challenge_accepted(state)):
        return False
    recorded = cross_completed_slugs(state)
    reviewers = {
        r.get("peer") for r in state.get("reviews", {}).values() if r.get("peer")
    }
    if not (len(reviewers) >= 2 and reviewers <= recorded):
        return False
    verdicts = state.get("verdicts", {})
    return all(claim_evidence_backed(v) for v in verdicts.values())


def build_status(state: dict) -> dict:
    verdicts = {k: dict(v) for k, v in (state.get("verdicts") or {}).items()}
    # 两模型一致不能替代行为实验: 无本入口执行依据的 approve 逐 claim
    # 降级为暂态(仅 challenge 一次不代表所有 claim)
    for c, v in verdicts.items():
        if v.get("state") == "approve" and v.get("reason") != "executed-probe-passed":
            verdicts[c] = {**v, "state": "pending-behavioral-evidence"}
    return {
        "protocol": PROTOCOL,
        "frozen": state.get("frozen", False),
        "challenge_accepted": any_challenge_accepted(state),
        "review_accepted": review_accepted(state),
        "verdicts": verdicts,
        "cross": state.get("cross", []),
        "identity": {
            k: state.get(k)
            for k in ("head", "base", "runtime", "session", "goal", "repo")
        },
    }


def cmd_status(args: argparse.Namespace) -> None:
    review_dir = Path(args.review_dir)
    state = load_state(review_dir)
    if not state:
        raise fail("not-initialized", "先 init")
    verify_no_tamper(state)
    emit_json(build_status(state))


# ---------------------------------------------------------------------------
# human rendering (--format human)
# ---------------------------------------------------------------------------


def render_human(obj: dict) -> str:
    if "error" in obj:
        e = obj["error"]
        return f"错误 [{e.get('code')}]: {e.get('message')}"
    lines = []
    ident = obj.get("identity") or {}
    lines.append("== olp-review-evidence 状态 ==")
    if ident:
        lines.append(
            f"identity: head={ident.get('head')} base={ident.get('base')} "
            f"runtime={ident.get('runtime')} session={ident.get('session')} "
            f"goal={ident.get('goal')}"
        )
    lines.append(f"lifecycle: frozen={obj.get('frozen')} "
                 f"challenge_accepted={obj.get('challenge_accepted')} "
                 f"review_accepted={obj.get('review_accepted')}")
    verdicts = obj.get("verdicts") or {}
    if verdicts:
        lines.append("verdicts:")
        for cid, v in sorted(verdicts.items()):
            lines.append(f"  - {cid}: {v.get('state')} ({v.get('reason', '')})")
    cross = obj.get("cross") or []
    if cross:
        lines.append("cross: " + ", ".join(c.get("slug", "?") for c in cross))
    if not ident and not verdicts and obj.get("state"):
        lines.append(f"state: {obj.get('state')}")
    if obj.get("observed"):
        lines.append(f"observed: {obj.get('observed')}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


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
    sp.add_argument("--head")
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
    sp.add_argument("--evidence", help="证据文件(非 live 路径仅作负向判定/--imported 分层)")
    sp.add_argument("--claim", required=True)
    sp.add_argument("--kind")  # hint only — never a gate
    sp.add_argument("--test-name")
    sp.add_argument("--harness-root", action="append")
    sp.add_argument("--exec-argv", help="已移除: 任意执行体一律 executor-not-trusted")
    sp.add_argument("--run-dir", help="兼容保留,不再使用")
    sp.add_argument("--timeout", type=int, default=600, help="live 执行超时(秒)")
    sp.add_argument("--imported", action="store_true",
                    help="外部导入证据: 一律 not-replayed,不能旁路执行门")
    # 唯一 live 入口: 本入口实时执行固定的生产 Cargo adapter
    sp.add_argument("--live-cargo", action="store_true",
                    help="由本入口实时执行固定的生产 adapter(唯一 live 入口)")
    sp.add_argument("--selector", help="live: 精确测试名(裸名或全限定)")
    sp.add_argument("--expect", choices=["pass", "fail"], help="live: 显式预期结果")
    sp.add_argument("--exec-repo", help="live: 执行 repo(真实 HEAD 须等于评审 HEAD;默认评审 repo)")
    sp.add_argument("--lib", action="store_true", help="live: --lib 单元测试入口")
    sp.add_argument("--test-target", help="live: 集成测试 target")
    sp.add_argument("--manifest", help="live: Cargo.toml 路径(须归属执行 repo)")
    sp.add_argument("--cargo-target-dir", help="live: 转发 adapter --target-dir")
    sp.set_defaults(func=cmd_challenge)

    sp = sub.add_parser("cross")
    common(sp)
    sp.add_argument("--cross-report", required=True)
    sp.add_argument("--cross-slug")
    sp.add_argument("--native-root")
    sp.add_argument("--runtime-evidence")
    sp.add_argument("--expect-claims")
    sp.add_argument("--allow-operator-refute", action="store_true",
                    help="显式允许 operator-decision 型 refute(可审计人工裁决)")
    sp.set_defaults(func=cmd_cross)

    sp = sub.add_parser("status")
    common(sp)
    sp.set_defaults(func=cmd_status)
    return p


def main(argv: list[str] | None = None) -> int:
    global CLI_HUMAN_FORMAT
    parser = build_parser()
    args = parser.parse_args(argv)
    CLI_HUMAN_FORMAT = getattr(args, "format", "json") == "human"
    human = CLI_HUMAN_FORMAT
    try:
        args.func(args)
        return 0
    except ReviewError as e:
        if human:
            sys.stdout.write(render_human({"error": {"code": e.code, "message": e.message}}) + "\n")
        else:
            emit_json({"error": {"code": e.code, "message": e.message}})
        return 1
    except BrokenPipeError:
        return 1


if __name__ == "__main__":
    sys.exit(main())
