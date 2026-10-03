# 🐧 penguin command

A terminal that runs inside *any* terminal — with an AI system assistant one keystroke away.

Penguin wraps your real shell (`bash`, `zsh`, `fish`, `pwsh`) in a polished TUI. Type commands exactly as you always do. Hit the toggle key and the screen shifts into **agent mode**: ask questions, delegate tasks, or hand over a whole goal — while your shell keeps running above.

Works on Linux, macOS and Windows, inside Ghostty, WezTerm, kitty, foot, Alacritty, Terminal.app, Windows Terminal, ConEmu… anything VT100-ish.

## Quick start

**1 · Install** (builds from source; bootstraps Rust if needed)

```sh
./install.sh                        # Linux / macOS  → ~/.local/bin/pc
powershell -File install.ps1        # Windows        → %LOCALAPPDATA%\Programs\pc
```

**2 · Connect a model** — run `pc setup`, press `a` to add a provider:

| Field | Example |
|---|---|
| name | `Ollama` |
| kind | `openai` (Ollama, LM Studio, OpenRouter, llama.cpp, vLLM, any custom endpoint) or `anthropic` |
| base_url | `http://localhost:11434/v1` — press **Enter** in this field to test & fetch the model list |
| api_key | paste a key, or `$MY_ENV_VAR` |
| ctx | context window in tokens — drives the usage meter and auto-compaction |

Press `t` on a provider to **ping the default model**: penguin measures time-to-first-token, so a model that takes 90 seconds to load off disk is visibly *alive* rather than hung. Back on the provider list, press `s` to make it active. Done — config lives in `~/.config/penguin/config.toml`.

**3 · Go** — run `pc`. Your shell opens exactly as normal. Press `Ctrl+Space` any time and just ask:

```
✻ ❯ what's eating all my disk space?
✻ ❯ rotate the nginx logs and compress anything older than a week
✻ ❯ /goal set up a python venv for this repo and install the deps
```

## Features

- **Drop-in wrapper** — a real PTY session with full terminal emulation (colors, alt-screen apps like `vim`/`top`, scrollback). Zero interference in shell mode.
- **Two unmistakable modes, one window** — deep dark Omarchy palette throughout; neutral chrome for SHELL, glowing violet `✦ AGENT MODE` badge for the agent. Agent mode takes over the *whole* screen (your shell keeps running underneath) so answers, tool output and goals get maximum room. Toggle off and your terminal is exactly where you left it. An animated ASCII penguin hangs out in the console. 🐧
- **Built for slow thinking models** — tuned for local reasoning models (Qwen3.x and friends) that spend minutes thinking before they speak:
  - Chain-of-thought streams in its own live panel (`✻ reasoning · 42s`), so the screen never looks frozen while the model works; the answer itself stays clean.
  - Elapsed timer plus a live `tok/s` readout once answering starts, and per-tool run timers — you always know what it's doing and for how long.
  - Generous, configurable timeouts (10 min to first byte, 5 min between bytes) so loading a big model over the network isn't mistaken for a stall.
  - Thinking depth is a dial: `low · medium · high · xhigh`, switched live with `/effort`.
- **Workspace-aware** — the agent adopts your shell's current directory (via OSC 7 shell integration) as its workspace on toggle.
- **Easy provider setup** — a TUI wizard (`pc setup` or `/providers`): add OpenAI-compatible endpoints (OpenAI, Ollama, LM Studio, OpenRouter, llama.cpp, vLLM, anything) or Anthropic. Name each provider as you create it, test the connection, fetch the model list, ping for latency, pick your default, and set the context window (`ctx`) used for usage tracking. Names are validated unique. Editing `config.toml` by hand still works.
- **System-assistant tools** — run commands, read/write/edit files (anywhere it may read; writes are guarded), find/grep, list dirs, chmod/chown. Coding and project builds work too via the same tools.
- **YOLO mode** — `pc --yolo` or `/yolo` auto-approves every action the safety gate would prompt for (sudo, writes outside the workspace, `curl | sh`). Catastrophic hard-blocks (`rm -rf /`, raw disk writes, fork bombs…) and interactive sudo password prompts always remain. A red `⚠ YOLO` badge stays in the status bar while it's on.
- **Goals sidebar** — `/goal <thing>` gives the agent autonomy. A panel on the right of the screen shows the active goal and its steps ("minor goals") with live status — ○ pending, ◐ running, ✓ done, ✗ failed — plus a ★ verified banner once the agent proves success with a real check. If the model stops before verifying, penguin **auto-continues** it (up to `agent.goal_continuations` rounds); when even that runs dry, `/continue [guidance]` picks the goal back up **with its plan intact** — no replanning round-trip. Want it to simply never stop? **`/limit off`** lifts the iteration and continuation caps so a run keeps going until it verifies (or you hit `Esc`) — re-read every step, so it also works mid-run; a `∞ UNLIMITED` badge stays in the status bar while it's on, and `/limit on` restores the caps.
- **Mid-run steering** — typing while the agent works used to mean "wait for the turn to end". Now anything you enter (`Enter`, or explicitly `/queue <text>`) is injected into the conversation **at the agent's next step**: it folds your extra instruction into its current work without stopping. `/queue` lists what's waiting, `/queue clear` drops it; whatever a turn never got to (e.g. after an abort) still flushes when it ends.
- **Sessions** — every conversation is saved under `~/.config/penguin/sessions/`, checkpointed after *every* agent step so a crash or timeout loses at most one move. The active goal, its plan and its verified flag ride along: `/resume` (or `pc --continue`) restores the sidebar exactly as it was, and an unfinished goal can be resumed later with `/continue`. `/new` starts fresh, `/sessions` lists history.
- **Skills that grow the system** — after finishing a goal the agent can distill what it learned into a reusable *skill* (`save_skill`) stored in `~/.config/penguin/skills/`. Skills are injected into every future session's system prompt; the agent loads and follows them with `load_skill` when one matches. `/skills` lists the library.
- **Context meter & auto-compaction** — the status bar shows live context usage (`ctx 42%`) against the provider's window. At ~85% (configurable: `agent.compact_at_percent`) penguin summarizes older turns and elides stale tool output automatically, without breaking tool-call pairing. If a provider still rejects a prompt as too long ("maximum context length…"), penguin **compacts hard and continues** the turn instead of dying on the error. `/context` for details, `/compact` to force it.
- **Never wonder what to press** — every screen carries visible key bars: mode-toggle hints in the status bar, a legend under the composer (`enter send · alt+enter newline · …`), slash-command suggestions as you type `/` — and `Enter` on a partial command **completes and sends it** (ambiguous prefixes fill the common part so you can keep typing) — plus full key bars in the provider wizard.
- **Idle penguin screensaver** — leave the terminal untouched for a few minutes and animated ASCII penguins take over: waddling through a blizzard, waving under a shimmering aurora, or belly-sliding across the ice, with a per-character shimmer title (à la the Omarchy screensaver). Any key wakes it instantly. Force it anytime with `/screensaver`; tune or disable via `ui.idle_secs`.
- **Sudo that just works** — prefix a command with `sudo` and penguin rewrites it to read the password from a pipe, detects the `[sudo] password for …:` prompt on stderr, and pops a **hidden password modal**. Children are detached with `setsid` so nothing can steal your keystrokes or echo the secret. The agent is explicitly told never to ask you to paste a password into chat.
- **Safety engine**
  - *Hard deny*: `rm -rf /`, raw device writes, `mkfs`, fork bombs… never executed, not even with approval.
  - *Permission gate*: anything touching paths outside the workspace or risky system commands pops a dialog: **Allow once / Always allow / Never allow** (`1`·`2`·`3`, or `y`/`a`/`n`; `Esc` denies once). Rules persist.

## Build & install

**One-liner (from a clone of this repo):**

```sh
./install.sh          # Linux / macOS — builds and installs to ~/.local/bin
                      # PC_INSTALL_DIR=/opt/bin ./install.sh to override
powershell -File install.ps1   # Windows — installs to %LOCALAPPDATA%\Programs\pc
```

Both scripts bootstrap [Rust](https://rustup.rs) if it's missing (pass `--no-rustup` to fail loudly instead). Running the installer as root defaults to `/usr/local/bin`.

**Manually:**

```sh
cargo build --release
# binary at target/release/pc — copy anywhere on your PATH
```

Then just run `pc`.

**Uninstall:**

```sh
./uninstall.sh              # remove the binary, keep config (providers, permission rules)
./uninstall.sh --purge      # remove everything including ~/.config/penguin
powershell -File uninstall.ps1    # Windows  (-Purge for full removal)
```

## Usage

### Command line

```
pc [OPTIONS]           start your shell with the agent one keystroke away
pc setup               configure providers (TUI wizard)

--shell <prog>         shell to run (default: $SHELL)
-w, --workspace <dir>  agent workspace (default: current dir)
-p, --prompt <text>    start in agent mode with this first prompt
-c, --continue         resume the most recent agent session
--yolo                 auto-approve every tool action (hard-blocks still apply)
```

### Keys

| Key | Action |
|---|---|
| `Ctrl+Space` | toggle agent mode (configurable: `ui.toggle_key`, e.g. `"ctrl-t"`, `"alt-i"`) |
| `Shift+PgUp/Dn` | scroll shell / transcript |
| `Esc` (agent busy) | abort the current agent turn — mid-stream, mid-retry-wait or mid-compaction |
| `Alt+Enter` | newline in composer |
| `Enter` (typing `/cmd`) | complete a slash command and send it; ambiguous prefixes fill the common part |
| `Enter` (agent busy) | queue the prompt — injected at the agent's next step |
| `Up` / `Down` | composer prompt history |
| `Tab` (agent mode) | expand/collapse full tool output |
| `Ctrl+Q` | force-quit penguin |

> **Why Ctrl+Space and not Ctrl+I?** In terminal protocols Ctrl+I *is* Tab — they are the same byte. Binding it would destroy tab-completion. The default is therefore `Ctrl+Space`; change `toggle_key` in config to any chord you like.

### Agent composer commands

```
/goal <text>      autonomous mode: plan → execute → verify (auto-continues if stopped early)
/continue [note]  resume the unfinished goal with its plan intact (+ optional guidance)
/queue <text>     add an instruction handed to the agent at its next step mid-run
                  (/queue lists the queue · /queue clear empties it · idle: runs now)
/limit off|on     remove/restore run-length caps — off never pauses for /continue (Esc still stops)
/new              start a fresh session
/sessions         list saved sessions
/resume [id]      resume a session (latest if no id)
/retry            resend the last prompt
/skills           list the agent's saved skill library
/compact          compact the context right now
/context          show context usage vs the provider window
/effort           cycle reasoning depth · /effort l|m|h|x · /effort off
/yolo             bypass approval prompts (dangerous) · /yolo off to restore them
/providers        open the provider setup wizard
/model [name]     list models / switch model
/workspace <dir>  set the agent workspace manually
/screensaver      wake the penguins (force the idle screen)
/clear            clear transcript
/quit             exit penguin
/help             all commands
```

Aliases: `/cont`, `/history`, `/ctx`, `/ws`, `/saver`, `/exit`.

Sessions live in `~/.config/penguin/sessions/`, skills in `~/.config/penguin/skills/`.
Start `pc --continue` (or `-c`) to pick up the most recent session — an unfinished
goal comes back with its plan; `/continue` sends it on its way again.

### Provider configuration (`~/.config/penguin/config.toml`)

```toml
active_provider = "Ollama"

[[providers]]
name          = "Ollama"
kind          = "openai"                     # or "anthropic"
base_url      = "http://localhost:11434/v1"  # any OpenAI-compatible endpoint
api_key       = "$OLLAMA_KEY"                # literal or $ENV_VAR (key file is chmod 600)
models        = ["qwen3:32b"]
default_model = "qwen3:32b"
context_window = 262144                      # drives ctx meter + auto-compaction

[agent]
max_iterations         = 40
goal_continuations     = 2       # extra rounds if a goal stops before verifying
unlimited_run          = false   # true (or /limit off) = no caps: runs until verified or Esc
compact_at_percent     = 85      # auto-compact threshold
auto_approve_workspace = true    # writes inside workspace don't prompt
reasoning_effort       = "xhigh" # low | medium | high | xhigh | off
max_tokens             = 16384   # completion ceiling, clamped per request to the room left
max_reasoning_tokens   = 0       # cap thinking tokens (llama.cpp/Ollama); 0 = don't send
yolo                   = false   # true = auto-approve all Ask prompts (hard-blocks remain)
first_byte_timeout_secs = 600    # wait for headers/first byte (model loading)
idle_timeout_secs       = 300    # max silence between streamed bytes
max_retries             = 2      # connect failures + 429/5xx, before any output

[ui]
toggle_key   = "ctrl-space"
idle_secs    = 150               # penguin screensaver after 2.5 min idle (0 = never)

[[rules]]                        # recorded by Always/Never allow
pattern  = "git status"
decision = "allow"               # or "deny"
```

Missing keys fall back to the values above; out-of-range ones are sanitized on load, so a typo can't wedge a run.

## How it works

```
your terminal
└─ pc (TUI)
   ├─ PTY ── your shell (bash/zsh/fish/pwsh + OSC7 integration)
   │         rendered through a vte grid emulator, scrollback included
   ├─ agent thread  ── provider chat w/ tool calling (async streaming SSE)
   │    tools: run_command · read/write/edit_file · find_files · list_dir
   │            set_permissions · update_plan · finish_goal
   │            save_skill · load_skill · list_skills
   └─ safety gate ── hard-deny regexes → rules → workspace/path analysis
```

- Agent commands run in a hidden background shell (your visible session is never polluted); output streams into the agent view.
- Reads are allowed anywhere on the machine; writes/deletes/permission changes outside the workspace require approval.
- On toggle, the agent captures your shell's live cwd as its workspace.

### Staying sane on slow or flaky endpoints

Local models can take minutes to load and can die mid-reply. Penguin assumes that's normal:

- **Retries with abortable backoff** — connect-phase failures and `429/500/502/503` are retried (honoring `Retry-After`) *only while nothing has been streamed*, so a retry can never duplicate a side effect. You see `↻ endpoint busy (HTTP 503 · retry 1/2) — waiting 3s`.
- **Esc really interrupts** — HTTP is async with an abort-aware select, so cancel lands in well under a second even while blocked on a socket, sleeping between retries, or compacting. A killed agent thread no longer leaves the UI spinning forever.
- **Truncated replies can't do damage** — if the model hits its token ceiling (`finish_reason=length`), half-streamed tool calls are dropped and *reported*, never executed with guessed arguments.
- **Thinking-starvation auto-recovery** — reasoning models sometimes spend the entire completion budget on chain-of-thought and return nothing usable. Penguin now escalates automatically: first it doubles the token budget and nudges "answer now, no more thinking"; if it starves again it drops the thinking depth one notch for that retry only (your `/effort` setting is untouched); only then does it fail with hints (`/effort off`, raise `max_tokens`, or set `max_reasoning_tokens`). Set `agent.max_reasoning_tokens` to cap thinking server-side and prevent the failure outright.
- **Context-budget-aware output size** — `max_tokens` is a ceiling that penguin clamps per request to the space actually left in the window (minus a safety margin), so deep answers get room without an `n_ctx` overflow.

## Development

```sh
cargo test           # VT emulator, safety, sudo-prompt, retry/abort, reasoning & budget unit tests
cargo run            # dev build
```

Optional live smoke test against a real endpoint (exercises streaming, `reasoning_effort`, the context clamp and clean termination):

```sh
PC_LIVE_URL=http://host:port/v1 PC_LIVE_KEY=… PC_LIVE_MODEL=… \
  [PC_LIVE_CTX=250000] cargo test live_endpoint -- --ignored --nocapture
```

Key modules: `src/term` (VT emulator), `src/app` (UI + event loop), `src/penguin.rs` (ASCII penguin sprites, animation frames & the idle screensaver scenes), `src/agent` (tool loop, goals), `src/safety` (policy engine), `src/providers` (OpenAI-compat + Anthropic clients, streaming, retries), `src/setup_tui.rs` (provider wizard), `src/integration.rs` (shell hooks).

## License

MIT
