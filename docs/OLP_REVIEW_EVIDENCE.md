# OLP Review Evidence — 行为证据互审流程

> 入口: `scripts/olp-review-evidence.py` · 监控: `scripts/olp-review-monitor.py`
> Spec: [specs/task-evo-review-evidence.spec.md](../specs/task-evo-review-evidence.spec.md)
> Plan: [docs/superpowers/plans/2026-09-09-review-evidence.md](superpowers/plans/2026-09-09-review-evidence.md)

## 流程总览

```
init ──▶ freeze ──▶ challenge ──▶ cross(×2) ──▶ status
          两份初审     行为证据       逐 claim 裁决    汇总判词
          (冻结+SHA)  (执行为本)     (采纳/反驳/待验证)
```

四个子命令按顺序推进;任一步失败输出可解析 JSON
`{"error": {"code": "...", "message": "..."}}` 且 exit != 0。

## init — 初始化评审目录

```bash
python3 scripts/olp-review-evidence.py init <review_dir> \
  --repo <repo> --base <base_sha> --head <head_sha> \
  --runtime <runtime_path> --session <session_id> --goal <goal_id>
```

记录 HEAD/base/runtime/session/goal 到 `review-state.json`(原子写入)。

## freeze — 冻结两份独立初审

```bash
python3 scripts/olp-review-evidence.py freeze <review_dir> \
  --glm-review glm.md --k3-review k3.md \
  --glm-slug <slug> --k3-slug <slug> \
  --native-root <runtime>/data/peers   # 外部权威收据根
  [--runtime-evidence runtime-evidence.json]
```

**前置校验(fail-closed)**:

| 校验 | 错误码 |
|---|---|
| 两份初审文件都存在且可解析 frontmatter | `first-reviews-incomplete` |
| peer outcome 非 pending/errored(自报) | `peer-outcome-invalid` |
| runtime-evidence 该 slug 无 active_thread | `peer-outcome-invalid` |
| native result-N.md 非 errored(外部否决) | `peer-outcome-invalid` |
| native 收据 slug 与报告 slug 匹配 | `peer-authority-mismatch` |
| **有至少一种外部终止权威**(native 收据或 runtime-evidence 终止快照) | `peer-authority-missing` |
| 报告 turn ≥ turns.txt 已结束轮次 | `stale-turn` |
| 报告 HEAD == 评审 HEAD(可选) | `head-mismatch` |

冻结记录每份初审的 SHA256/peer/turn/outcome;此后任何改写或**删除**
均触发 `first-review-tampered`(verify fail-closed)。

## challenge — 行为证据(内容校验为门、执行为本)

```bash
python3 scripts/olp-review-evidence.py challenge <review_dir> \
  --evidence ev.log --claim <claim_id> \
  --test-name outer_review_627_x \
  --harness-root tests/ --harness-root src/ \
  --exec-argv "cargo test --test x outer_review_627_x" \
  --run-dir <repo> [--timeout 300] [--imported]
```

**判定顺序**:

1. **内容门**: 证据含结构锚(测试名行 + panicked at + FAILED 汇总,
   或通过形态 `test result: ok.`);纯文档/字符串断言 →
   `evidence-not-behavioral`,claim 置 `unverified`。
2. **测试名解析**: `--test-name` 必须能在 `--harness-root` 源码中解析到
   `fn <name>(` 定义。
3. **imported 分层**: 外层导入未独立执行的日志 → `not-replayed`,
   不自动 accepted。
4. **执行为本(核心)**: 生产入口实际执行 `--exec-argv`,捕获退出码与
   stdout/stderr;执行输出必须含测试名与失败/通过锚。
   `/usr/bin/false` 等无输出无关命令 → `evidence-not-executed`。
   manifest 绑定 HEAD/run_dir/test_name/argv/exit_code/输出摘要。

**判别式**(executed 后):

- exit≠0 + FAILED 汇总(PR HEAD 真实复现) → `blocked-on-evidence`
- exit=0 + `test result: ok.`(真实通过) → `approve`
- 仅遗留路径可达 → `flipped`(PR 级聚合 residual)

## cross — 交叉互审

```bash
python3 scripts/olp-review-evidence.py cross <review_dir> \
  --cross-report cross.md --cross-slug <slug> \
  --native-root <runtime>/data/peers --expect-claims X,Y,Z
```

前置: frozen + challenge 已接纳。cross 报告须逐 claim 覆盖
(`missing-claim-coverage`);errored/pending 拒绝;同样要求外部权威
收据。cross 以新证据/行号反驳 flip → `challenge-refuted` 回边。

## status — 汇总

```bash
python3 scripts/olp-review-evidence.py status <review_dir>
```

- 无执行证据的两模型一致 approve → `pending-behavioral-evidence`
- PR 级聚合 = f(claim × severity × introduced_vs_existing),
  期望分类来自外层 MANIFEST `outer_recommendation`(禁 PR 编号硬编码)

## 判词状态机(claim 级两层词表)

```
approve ──challenge(fail)──▶ flipped ──cross反驳──▶ challenge-refuted
   │                            │
   │ 无证据                     │ executed FAIL @HEAD
   ▼                            ▼
pending-behavioral-evidence   blocked-on-evidence
   │ 收口仍缺
   ▼
unverified          not-replayed(imported 未独立复验)
```

## 测试真实性边界

- `tests/olp_review_evidence.rs`(23 tests): 子进程真实调用生产入口;
  外层反例回归 4 条先 RED 后修(证据 `.octos/red-proof/`)。
- `olp_review_real_store_harness_executes_historical_negative_probes`:
  临时 clone 固定 PR 合成树 + `include!` 外层诊断反例源码,真实 cargo
  编译执行 8 探针(约 30s,独立昂贵验收,不在每个单测重复)。
- Python 打印 cargo 样式文本的伪造 argv 无法成为行为 verified。
- 顺序如实记录: 首批 18 测试与脚本同轮实现(先落盘后补测试);
  外层反例与新场景按先 RED 后修。
