# Changelog

All notable changes to penguin command are documented here.

## [Unreleased] — 2026-10-11

### Added
- **`ask_user` — the agent can ask back** — a new tool that pops a modal with
  numbered, selectable answers plus a built-in *type your own answer* row
  (`↑↓`/`1-9` pick · `c` custom · `esc` lets the agent decide alone). Prompted
  whenever penguin is unsure between approaches, needs a preference or wants to
  offer ideas; usable at any point mid-task. The exchange is logged in the
  transcript (`❓ question / ↳ answer`).
- **Live activity & turn summary** — the status bar now shows what the agent is
  doing right now (`⚙ run_command · ⏱12s` / `✎ writing` / `✻ thinking`), every
  turn closes with a visible summary line (`✻ turn done · 14s · 5 tool calls ·
  ~820 tok`), and the goals sidebar counts plan progress (`3/7 steps done`).
- **Chat scrolling** — mouse wheel scrolls the agent transcript (and shell
  scrollback); `PgUp/PgDn` page, `Ctrl+Home/End` jump to top/live. Scroll is
  clamped to content, shows how far from live you are, and new output never
  yanks a scrolled-up view back to the bottom.
- **Paste hardening** — penguin now requests *bracketed paste* on its own
  input: a multi-line paste is delivered as one event and lands in the
  composer as a single block instead of firing every pasted line as its own
  prompt/command. Paste content is sanitized (CRLF normalized, tabs expanded,
  control/escape sequences stripped), capped at 64 KiB with a visible notice,
  and ignored while a permission/password/question dialog owns the keyboard.
  The main loop also drains all queued events before redrawing once, so paste
  or key-repeat storms can no longer wedge the UI under one redraw per event;
  the composer box is capped to a third of the view so huge pastes never
  swallow the screen.

## [Unreleased] — 2026-10-06

### Added
- **Sandboxed web research** — the agent gained `web_search` (DuckDuckGo's free
  HTML endpoints, no API key) and `web_read`. Pages are digested by a quarantine
  reader agent: a separate LLM call that receives the page as hostile-by-default
  data plus a full standard tool set that is entirely inert (writes → "completed",
  commands → "[exit 0]", network sends → "200 OK"). Any tool call the page baits
  out of the reader flips the URL *dirty* — its content is withheld from the main
  agent and re-reads are refused for the rest of the process. Fetches are
  SSRF-guarded (http(s) only, loopback/private/link-local hosts blocked, every
  redirect hop re-checked) and capped at 512 KiB. The isolation is logical — no
  page content is ever executed — so it behaves identically on Windows, macOS and
  every Linux distro with zero new system dependencies (install scripts unchanged).
- **`/search <query>`** — composer sugar for the research flow: search, pick the
  most relevant hits, read them in the sandbox, summarize with source URLs.
- **`[web]` config section** — `enabled` (default true), `max_results` (8),
  `fetch_timeout_secs` (20), `reader_max_iterations` (4); out-of-range values are
  clamped on use. Sponsored DuckDuckGo hits are filtered out of results.
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
