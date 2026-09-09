# 计划 — OctoLoop 行为证据互审流程 + Herdr pane 监控入口 (goal_01)

日期: 2026-09-09(Phase 5 修订版)
worktree: /Users/zhangalex/.local/tmp/octoloop-evolution-20260909/octoscode-behavior-review
HEAD 基线: 0a174d95ddec2b123adb3498432e29eb13affb81
契约: specs/task-evo-review-evidence.spec.md (lint 100%, --min-score 0.7 通过, 23 场景)
证据源: /Users/zhangalex/Work/Projects/FW/octoscode/.octos/reviews/pr-627-630-20260909/
Phase 5 裁决纪要: .octos/cross-design-dispositions.md(GLM/k3/外层逐条采纳/驳回)

## 阶段分步

1. [x] Phase 0 勘察: git status clean、brief/Active 读取、证据源与 MANIFEST 结构确认
2. [x] Phase 1 原生 goal: goal_create → goal_01;progress.md 落盘
3. [x] Phase 2 契约: specs/task-evo-review-evidence.spec.md 写入,agent-spec parse+lint 通过(100%,非零 scenarios)
4. [x] Phase 3 本计划落盘
5. [x] Phase 4 双只读 peer 独立设计审查(GLM primary 继承 / k3 model=strong,worktree=false,显式 goal_id):
   - 各自读 spec+plan+证据源,产出 .octos/design-glm.md / .octos/design-k3.md ✅ 已收齐
   - 收齐前不互读 ✅
6. [x] Phase 5 定点互审: 互读对方设计审查 → 逐条采纳/反驳/待验证 → 修 spec/plan → 重新 lint
   - 裁决落盘 .octos/cross-design-dispositions.md;外层 outer-feedback.md 全部 13 条定点改判吸收
   - spec 16→23 场景(pending peer/native 信号否决/伪造 cargo 日志/imported not-replayed/
     errored cross/flip 回边/approve happy path/分层 fallback/unknown-lifetime/哈希 ACK)
   - 删 estimate=2d;Allowed Changes 增 plans、scripts 通配、olp_watch_board.rs 定点修复条款
7. [ ] Phase 6 实现(master 单写,peers 只读):
   a. fixtures/review-evidence/**: 从前轮真实证据复制 8 反例 + 元数据;
      复制前后源目录 SHA256 复核,fixtures 自带 SHA256 清单 + provenance 头
   b. scripts/olp-review-evidence.py(可拆 olp-review-*.py 共享 helper): freeze/challenge/cross/status 子命令
      - freeze: 两份初审齐全+可解析+outcome 有效(外部信号: runtime-evidence active_thread、
        native result-N.md 否决报告自报) → manifest(SHA256/HEAD/base/runtime/session/goal/peer/turn),
        原子写入(临时文件+rename),错误输出可解析 JSON
      - challenge: **内容校验为门+执行为本**: 结构锚(测试名行/panicked at/test result: FAILED/
        测试名可解析 harness 源)为必要条件;执行验证由生产入口实际执行证据携带的 argv/脚本,
        捕获退出码+stdout/stderr,绑定 HEAD/运行目录/输出摘要;imported 未独立执行 → not-replayed;
        kind 字段仅 hint;纯构造日志 → evidence-not-executed 拒绝
      - cross: 要求 frozen+challenged;每条判词必须有 采纳/反驳/待验证 裁决;errored/pending
        cross 拒绝;反驳成立 → challenge-refuted 回边
      - status: 汇总,无证据绑定的两模型一致判词 → pending-behavioral-evidence;
        PR 级 = f(claim × severity × introduced_vs_existing),期望分类来自外层
        MANIFEST outer_recommendation(工具聚合,禁 PR 编号硬编码)
   c. scripts/olp-review-monitor.py: --watch 模式渲染 lifecycle/current-turn/last-outcome/deliverables
      四区块;runtime 复合身份;last-outcome=最近终止结果(旧 completed+新 running →
      last-outcome=completed, current-turn=running, accepted=false 分层);
      有 authority 时优先 lifetime.json(turn_id/generation),无时 unknown;
      fallback 按 result-N.md+turns.txt+ui-protocol thread 完成态交叉核对
      (**禁 next_seq vs turn 数值比较**,量纲不同);inplace ACK 哈希比对观测;
      与 olp-watch-board.sh 并行不干扰(只读板文件)
   d. tests/olp_review_evidence.rs + tests/olp_review_monitor.rs: 每场景一个 #[test],
      以子进程真实调用 scripts/olp-review-*.py(RED→GREEN: 先写测试跑出断言级失败,
      编译错误不算 RED,输出存 .octos/red-proof/ 附 SHA256+HEAD 清单);
      至少一条集成测试调用真实运行入口执行历史生产反例(harness 成功与产品测试
      失败分别记录);前置检测 python3 在 PATH
   e. docs/OLP_REVIEW_EVIDENCE.md: 使用手册 + 判词状态机两层词表 + PR 级聚合规则
   f. housekeeping commit(单独,先于功能实现): tests/olp_watch_board.rs:411
      `while !read(...).map(...).unwrap_or(false) || true` → `loop { deadline break }`
      语义等价修复(非行为改变,保留完整等待时长与原断言),解 clippy 基线
8. [ ] Phase 7 双 peer 独立 review(同一 peer 名续轮)→ independent-glm/k3.md → 互读 → cross-glm/k3.md
9. [ ] Phase 8 必跑验证:
   - cargo test --all-targets -- --test-threads=8 (CARGO_BUILD_JOBS=4)
   - cargo clippy --all-targets -- -D warnings(housekeeping 后应 0 基线错误;豁免清单文件化防吞新告警)
   - cargo fmt --check
   - agent-spec lifecycle 相关 spec
   - grep 核对 23 个过滤名全部落地为 #[test]
   - 更新 progress.md + 提交(仅 git add 本任务文件)
10. [ ] Phase 9 外层复验采认(外层 Codex 复核,非本内环自报;内环完成后留 commit+报告+ACK(done),
    不无限轮询)

## 验收硬门

- 8 反例经完整 freeze→challenge→cross 流程后: #627/#628/#630 → blocked, #629 → residual
  (派生自外层 MANIFEST outer_recommendation + evidence,非硬编码)
- 负向: 伪造(含结构齐全的假 cargo 日志)/篡改/errored+pending peer/旧轮次/异HEAD/
  非行为证据/imported 未复验 → 全部被拒绝且报错码明确、错误输出可解析
- 监控: 四区块 + 复合 runtime 身份 + 分层 last-outcome + fallback 交叉核对(无 lifetime→unknown)
  + inplace ACK 哈希观测
- RED 阶段证据保留: 测试先失败(断言级)输出存 .octos/red-proof/ + SHA256+HEAD 清单
- 没有运行的检查必须标"未验证"
