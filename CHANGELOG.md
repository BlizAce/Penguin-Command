# Changelog

All notable changes to penguin command are documented here.

## [Unreleased] — 2026-10-02

### Added
- **Slash-command autocomplete on Enter** — while typing a `/command`, pressing
  `Enter` now completes a unique prefix and sends it immediately (e.g. `/comp ↵`
  runs `/compact`). Ambiguous prefixes fill the longest common part without
  sending, so you can keep typing to disambiguate; exact commands submit as typed.
- **`/queue <text>` — mid-run steering** — instructions typed while the agent is
  working are injected into the conversation at its next step instead of waiting
  for the turn to end. Plain `Enter` while busy uses the same queue. Bare
  `/queue` lists what's waiting, `/queue clear` empties it; anything a turn never
  reached (e.g. after an abort) still flushes when it ends.
- **`/limit off|on` — remove run-length caps** — lifts `agent.max_iterations` and
  `agent.goal_continuations` so goals auto-continue until verified instead of
  pausing for `/continue`. Re-read every step, so it takes effect mid-run; the
  choice persists in config (`agent.unlimited_run`) and shows a `∞ UNLIMITED`
  badge in the status bar while active. Esc still aborts and all safety gates
  still apply.

### Changed
- **Context overflow self-heals** — when a provider rejects a prompt as too
  long ("maximum context length…", "prompt is too long…", `n_ctx` overflows),
  penguin now force-compacts (summarize → elide → hard-truncate, escalating)
  and continues the turn instead of failing it; only a third consecutive
  rejection ends the turn.
