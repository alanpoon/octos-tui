A renovation crew arrives at a split-level house. One side of the house (labeled "pending_approvals") has workers actively framing a doorway — a hard-hat boss holds up a "BLOCKED: approval required" sign. The other side (labeled "user_question") still has scaffolding up and a pending "please answer this question" banner hanging from the window.

The foreman (labeled "apply_hydrated_pending_questions") checks the pending_questions clipboard, finds it empty, and immediately radios the project manager: "All clear! Unblock everything — IDLE state!" — even though the approval crew is still visibly mid-work on the other side of the house.

The project manager flips the master switch to IDLE. All the "BLOCKED" warning lights go out. The approval crew, confused, looks up as their scaffolding warning lights die — but the doorway is still unfinished and the approval is still waiting for a decision.

The workers on both sides stand around in an IDLE state, unable to proceed, because the status board says "nothing pending" while both blockers remain physically present.

Bug: In `apply_hydrated_pending_questions` (and symmetrically `apply_hydrated_pending_approvals`), when the questions list is empty the handler clears `state.user_question` and calls `set_run_state_idle()` unconditionally — even if `state.approval` is still `Some` for this session (set moments earlier by `apply_hydrated_pending_approvals`). The final `run_state` becomes `Idle` while an approval is still blocking.

Fix: Before calling `set_run_state_idle()`, check whether the peer blocker (`state.approval` or `state.user_question`) is still set for this session. Only transition to Idle when both are clear.
