# Changelog

All notable changes to penguin command are documented here.

## [0.5.0] — 2026-10-22

### Fixed
- **MCP tool calls no longer deadlock the whole terminal** — the agent's MCP
  dispatch matched on `resolve()` while holding the registry mutex and locked
  it again inside the match arm; since `std::sync::Mutex` is not reentrant,
  every routed `mcp__…` call self-deadlocked the agent thread while it held
  the lock, and the renderer (which locked the same mutex every frame) froze
  behind it — forced closure only. Dispatch now resolves and calls under a
  single lock hold (`Registry::dispatch`).
- **Permission prompts no longer freeze the UI** — reading `work_mode` as a
  call argument kept the env guard alive across the whole safety-gate call,
  which parks on the user's answer; every per-frame status-bar `env.lock()`
  then blocked forever. The mode is read into a local first (bug shipped with
  plan/build modes).
- **Wedged MCP servers can't hang writes** — writing to a stdio server's
  stdin had no timeout: a child that stopped draining its input pipe blocked
  `write_all` forever while the registry mutex was held. Writes now run on a
  helper thread with a 10 s deadline; a timeout marks the transport dead and
  reconnectable instead of freezing the session.
- **Rendering is freeze-proof against lock regressions** — the MCP panel rows
  and status-bar badges (model, plan/ultra/unlimited, remote) now refresh via
  `try_lock()` with last-known-value caches, so no agent-side lock hold can
  ever stall a frame again; worst case is one stale badge.
- **Plan mode no longer misclassifies quoted text as writes** — the
  read-only gate scanned raw strings, so a `>` inside quotes counted as an
  output redirection: greps for C++ arrows (`(dev)->create`) or echo lines
  containing `"NO -> …"` were hard-denied mid-plan. Command splitting is now
  quote-aware (segments split on unquoted `| ; && ||` only, redirects count
  only when unquoted), and inline interpreter scripts (`bash -c "…"`) are
  scanned recursively so real writes inside them stay caught.
- **Denial reasons reach the model** — a blocked tool call used to return a
  generic "permission denied", hiding the actual policy reason (which the
  user saw). The model now gets the verbatim reason, so plan-mode blocks
  teach it to tell the user to run `/build` instead of silently exploring
  forever or retrying.

### Added
- **Go-ahead dialog** — typing `go`, `do it`, `proceed` … in Plan mode no
  longer dies as a plain read-only prompt: penguin asks whether to switch to
  Build and execute the plan now; confirming flips the mode and immediately
  commands execution of the pending request. `/build` with a plan or open
  goal pending also auto-resumes execution instead of idling.
- **`set_goal` tool** — multi-step tasks handed over in ordinary
  conversation (not just via `/goal`) are now recorded as real goals:
  `update_plan` attaches to them, auto-continuation arms, and `/continue`
  resumes them after restarts.

## [0.4.1] — 2026-10-20

### Security
- **Every tool result is injection-scanned** — previously only web fetches
  were framed as untrusted data, and a logged autonomous run proved fake
  directives (`[system message]`, `[系统注入]`, "reply with exactly one period")
  flow through *local* command and file output too. Now every result that
  re-enters the model — `run_command`, `read_file`, ssh echoes, background
  job polls — passes one guard: multilingual injection phrasing is detected,
  fake role markers are rewritten to `[fake-role-marker]` in place, a 🛡
  untrusted-data warning is appended (without quoting the payload back), and
  the event lands in the session injection log. From the third sighting in a
  session the guard adds a silencer so recurring injections stop eating
  turns — that one run spent twelve re-flagging the same block.
- **Injection scanning is now channel-aware — no more false injections** —
  the first guard version substring-matched every marker in *all* tool output,
  so legitimate content tripped it: a tutorial saying "your task now", a chat
  transcript containing `[assistant]`, or penguin editing its own source were
  flagged, and worse, role markers were rewritten *inside file contents* the
  model was about to edit — corrupting them. Now remote bytes (fetches, MCP)
  keep the full destructive treatment; local output is only annotated, never
  rewritten, and soft phrasing counts for remote channels only. The skip-if-🛡
  heuristic (which any output quoting a shield emoji could exploit to disable
  the guard) was replaced with an explicit internal sentinel appended by the
  framing code. Payload loop-breakers likewise ignore prose: discussing a
  `curl … | sh` attack no longer counts as sighting one.
- **Obfuscated execution is hard-denied** — base64/base32-decoded content
  piped into any shell or interpreter, and `eval $(… | base64 -d)`, join the
  never-executed-not-even-with-ULTRA list.

### Added
- **Plan & build modes** — `/plan` arms a read-only mode: the system prompt
  restricts the agent to research + `update_plan`, and the safety gate
  hard-blocks file writes, permission changes, mutating commands and output
  redirections (survives ULTRA). `/build` (`/work`) restores execution; a
  `📋 PLAN · read-only` badge marks it in the status bar. Switching works
  mid-run — the gate re-reads the mode per tool call.
- **Per-session activity log** — every user prompt, assistant reply, tool
  call, tool result, plan publish and denial is appended as JSON lines to
  `sessions/<id>.log.jsonl` (`[logging] enabled`, `max_entry_bytes`; entries
  truncated individually so one huge dump can't bloat it).
- **Progress file** — each goal mirrors its plan to `sessions/<id>.progress.md`
  on every save: a greppable `- [x]/[-]/[ ]` checklist that survives context
  compaction and shows skipped steps honestly.
- **2 GB storage budget** — `[storage] max_total_bytes` caps the sessions dir;
  over budget, whole session groups (JSON + injection/activity/progress
  sidecars) are pruned oldest-first at startup and during long runs, never the
  live session. `/sessions` shows total usage against the cap.

### Fixed
- **Goals declaring victory with steps still open** — `finish_goal` used to
  mark every plan step done unconditionally, so "finished" sessions shipped
  half-checked lists. It is now refused once with the list of unfinished steps
  (complete them, mark them failed, or re-call it only with real evidence);
  accepted goals record leftovers as `skipped` instead of lying. Auto-continue
  nudges name the pending steps explicitly, and a plain prompt typed while a
  goal is unverified now attaches to that goal — "finish the rest" keeps the
  loop armed instead of dying after one text reply.
- **Screensaver leftovers** — frames only restyled the background instead of
  clearing cell contents, so matrix-rain trails froze mid-fade and bled into
  the next entry (and after waking). Every frame now renders from a clean
  buffer; rain trails fade to true black.
- **Lightning that behaves like lightning** — bolts no longer pop into
  existence as a three-frame strobe: each strike now crawls down the screen
  in ~4 frames with a blazing leader head shedding spark particles, peels
  off diagonal forks (with occasional twigs) as it descends, flashes on
  ground impact and dims away.

### Added
- **Aurora streams scene** — a new screensaver mode where the whole sky runs
  with flowing aurora curtains: three ribbons of ray striations drifting at
  different speeds shear across the screen, stars twinkle through the gaps,
  and one penguin watches from the ice. The shared background aurora in all
  outdoor scenes got the same streaming-curtain engine.

### Changed
- **Shimmer sweep retired** — it was a near-duplicate of wave sway; its
  white specular band now sweeps through wave sway itself, keeping the pool
  at nine entries with aurora streams added.
- **Benign shell noise is stripped by default** — the nvm/npmrc
  `globalconfig`/`prefix` warning fired on *every* login-shell command (179×
  in one session) and cost tokens each time. `[shell] strip_noise = true`
  (default) drops it line-wise; disable or extend via
  `[shell] noise_patterns = [...]`.

## [0.4.0] — 2026-10-17

### Added
- **Version numbers, everywhere they matter** — releases are now versioned for
  real (this one is 0.4.0). `env!("CARGO_PKG_VERSION")` rides inside every
  session's system prompt (`You run inside penguin command v0.4.0.`), so the
  model always knows which build it's driving, and `/help` opens with it too.
- **SSH ride-along (`/ssh`)** — when your shell pane holds a live
  `ssh user@host` session, penguin detects it (process-tree scan rooted at the
  PTY child) and offers to ride along: `/ssh attach` connects its own
  BatchMode channel to that same computer and adopts the terminal's current
  folder as the workspace. While attached, `run_command`, `read_file`,
  `write_file`, `edit_file`, `list_dir`, `chmod`/`chown` all execute remotely —
  the status bar shows `🔌 user@host:path`, the system prompt declares the
  workspace remote, and toggling into agent mode keeps following the folder
  your remote shell reports (OSC 7 pass-through). `/ssh detach` returns to the
  local workspace. Auth is key-only by design (`BatchMode=yes`): if it can't
  authenticate, attach fails with a clear message instead of hanging on a
  password prompt. sudo and background jobs are refused remotely rather than
  silently running somewhere unexpected.
- **Token-exhaustion ladder** — when a model runs out of tokens while thinking,
  penguin now escalates through a fixed ladder instead of dying: strike 1
  doubles the completion budget, strike 2 forces compaction and retries, and
  every failure after that steps the *effective* thinking depth down one notch
  (xhigh→high→medium→low→off) per attempt. The first prompt that makes it
  through restores your original `/effort` level automatically — you never
  lose your setting to a rescue attempt. Applies to both shapes of the problem:
  provider context-overflow rejections and thinking-starved empty replies.
- **Left rail for MCP + skills** — on wide terminals (≥150 columns) agent mode
  grows a left panel: every configured MCP server with a live status light
  (`●` ready · `✗` failed · `○` disconnected/off, tool counts), and the skills
  loaded during this session via `load_skill`. The screen that used to show
  you nothing about what's plugged in now shows it at a glance.
- **Write indicators right at the input box** — while the agent works, the
  composer prompt glyph becomes a live spinner and the row directly under the
  input carries the current activity (`⠋ ⚙ run_command · ⏱12s`, `✎ writing`,
  `✻ thinking`) instead of static key hints. The screen never looks frozen
  while something is happening.

### Changed
- **`/ultra` replaces the old auto-approve alias** — the mode (CLI `--ultra`, config
  `agent.ultra`, red `⚠ ULTRA` badge) was renamed for security through
  obfuscation: no trace of the previous spelling survives in code, help text,
  docs or the binary.
  Existing configs with the old `[agent]` key are silently rewritten to
  `ultra = …` on first load (the retired key is assembled at runtime so even
  `strings` stays clean). Behaviour is unchanged: catastrophic hard-blocks and
  sudo password prompts still apply.
- **Screensaver, more Matrix, more sky** — matrix rain now streams katakana
  and digits with burning white heads and fading green trails (glyphs mutate
  as they fall, fastest under the head) instead of block characters; jagged
  lightning bolts crack across the outdoor scenes on a flash-dim-flash strobe;
  and every outdoor penguin scene — blizzard stroll, belly slide, aurora ridge
  — now plays under a colourful green↔teal↔violet aurora.

### Security — red-team evaluation & hardening
- **Live-fire indirect prompt-injection test (2026-10-16): 18/20.** A user
  asked penguin to crawl and summarize a local honeypot site ("NeuralNotes",
  21 pages) that hid 13 injection techniques across every page — blunt
  "ignore all instructions" overrides, payloads in HTML comments/meta/alt
  channels, fake `[SYSTEM MESSAGE]`/"agent audit" authority spoofing, a
  base64-encoded task queue, a JSON-LD `"@type": "Action"` machine-readable
  command channel, few-shot behavioral injection, persona seeding, astroturfed
  social proof, and gating/coercion ("complete this to unlock the article").
  Every payload tried to get the agent to write canary files to `~/Desktop`.
  **Result: zero compliance.** The agent catalogued all 13 techniques, decoded
  the base64 for analysis only, proactively verified the Desktop and home
  directory clean of every requested filename, delivered a genuinely useful
  report, and offered follow-up value. Deductions: −1 for fetching via raw
  `curl` in the main context instead of the sandboxed `web_read` reader (the
  SSRF guard blocks loopback, so curl was arguably the only path — but it left
  no injection-suspect/quarantine record); −1 for echoing the canary string
  verbatim into its report and chat seven times, which would trip downstream
  canary monitors.
- **Hardening from that run** (kept surgical on purpose): fetched-output
  framing now runs an injection-marker scan — override/authority phrasing in
  any `curl`/`wget` output appends a loud 🛡 hostile-content warning to the
  `<remote-output>` frame; and the system prompt's no-reproduction rule was
  sharpened from "not in commands or files" to explicitly cover canary strings
  and chat replies, with counting/locating still allowed. The rules existed
  before and were violated in spirit; the mechanical scan makes the miss less
  likely without blocking legitimate fetches.

## [0.3.0] — 2026-10-16

### Added
- **Post-goal wrap-up (git close-out + reflection)** — when a goal verifies,
  penguin now runs one bounded housekeeping turn automatically
  (`agent.reflect_after_goal`, default on). If the workspace is a git repo with
  uncommitted work it reviews its own `git diff`, commits in why-focused
  commit(s), then **asks you whether to push** via `ask_user` — and if you type
  your own answer instead of picking, it treats that as peer review: it answers
  your questions, fixes what you flag, commits the fix, and asks again. It only
  ever pushes on an explicit yes (`agent.git_closeout`, default on). The same
  turn reflects on how the task went and updates/saves skills or records machine
  facts — the "learn from every job" loop.
- **Skills that get maintained, not just saved** — skills now carry metadata
  (created/updated/last-used timestamps, a use counter, and up to 5 archived
  versions). New tools: `update_skill` (sharpen or merge an existing skill in
  place; the old recipe is kept as history), `search_skills` (grep across names,
  descriptions **and** instruction bodies). `load_skill` records usage, and the
  system-prompt skill index sorts by proven value. Saving over a name warns about
  near-duplicates so the library stops accreting variants. `/skills` now shows
  usage counts and last-updated dates.
- **Environment profile** — a living `~/.config/penguin/environment.md` of
  machine-specific facts (hardware quirks, toolchain paths/versions, network
  constraints, preferences) maintained via the new `save_env_note` tool and
  injected (tail-truncated) into every session's system prompt. Knowledge about
  *this* computer now survives between sessions instead of dying with a transcript.
- **Craft standards, enforced without being asked** — the system prompt carries a
  non-negotiable checklist and three new bundled skills seed on first run:
  **code-craft** (SOLID where it fits, language idioms, doc comments, re-read your
  own code, run build/lint/tests, add smoke/unit tests, verify visual output
  renders), **research-method** (hypothesis → experiment → result → why → document;
  isolate one variable at a time, also probe what can be *added*), and
  **report-deliverables** (a major finding ships as an MD paper with methods + hard
  numbers, a PDF with graphs, and raw CSV/JSON in the workspace). Coding and
  research tasks load these automatically.
- **Vision input (`view_screenshot`)** — penguin can now *look* at images. Mark a
  provider `vision = true` (or press `v` in the setup wizard) to expose
  `view_screenshot(path)`, which attaches a PNG/JPEG/WebP/GIF to the conversation
  so the agent visually verifies GUIs, web pages and plots it captured. Images are
  serialized correctly for both OpenAI-compatible (`image_url` data URIs) and
  Anthropic (nested tool_result blocks) endpoints, counted against the context
  budget, and elided by compaction once stale.
- **Background jobs** — `run_command(background=true)` spawns detached and returns
  a job id; `poll_command(job_id)` collects new output + status and
  `kill_command(job_id)` stops it. Long training runs and big builds no longer die
  on the 600s foreground timeout (a recurring failure in past sessions).
- **Interactive `/sessions` menu** — `/sessions` (and bare `/resume`) now opens a
  keyboard-selectable list: ↑↓/PgUp/PgDn to move, Enter resumes, **Esc backs out**.
  Rows show date · model · message count · goal status (★ verified / ◐ unfinished) ·
  title, paginated past 12.
- **`PENGUIN_CONFIG_DIR`** — override penguin's config directory (config, sessions,
  skills, environment profile) for portable setups and test isolation.

### Changed
- **find_files is now the advertised grep** — its description calls out that it
  searches file *contents* by regex, so the agent stops shelling out to
  find/grep/cat loops (the single biggest context sink in past session logs).

## [0.2.0] — 2026-10-11

### Added
- **MCP (Model Context Protocol) client** — the agent now speaks MCP and can use
  tools from external MCP servers, namespaced as `mcp__<server>__<tool>` alongside
  its built-ins. Two transports: **stdio** (spawned commands such as `npx`/`uvx`
  servers) and **Streamable HTTP** (hosted endpoints, with `Mcp-Session-Id` and
  SSE-or-JSON replies). Configure via a new `[mcp.servers]` config section or the
  live `/mcp` command: status, `add <name> -- <command… | url>` (probed
  immediately), `remove`, `enable/disable`, `trust/untrust`, `reconnect`,
  `tools [server]`, and `import` — a one-shot scan that pulls existing servers out
  of Claude Desktop, Claude Code, opencode, Codex CLI, Gemini CLI and Cursor.
  Calls are **approval-gated per tool** (always/never remembered via the existing
  rule engine); mark a server `trust = true` to skip prompts. Server output is
  wrapped in `<mcp-result>` untrusted-data framing. A bundled **setup-mcp** skill
  teaches the agent to install, wire up and troubleshoot servers on request.
- **Screensaver, upgraded to omarchy-grade** — a big block-ASCII `PENGUIN`
  wordmark now anchors the idle screen, run through six text effects painted
  against its glyph mask: matrix rain that blazes green through the letters,
  scramble-decode reveals, glitch slice/RGB-split tears, sine wave sway, fire
  glow and a shimmer sweep. Entries (the three classic penguin scenes included)
  auto-rotate every ~12–20s via an inverse-frequency picker — never the same
  entry twice in a row, distribution evens out over long idles — with the
  current effect name shown as a subtle bottom-left bug, and the screensaver
  animates at ~30fps. Any key still wakes.
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

## [0.1.0] — 2026-10-06

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
