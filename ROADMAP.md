# Architecture Contract

This section preserves the user's exact answers from architecture Q&A sessions. These answers are binding stipulations and must be read before asking more questions, resuming work after compaction, or producing an architecture plan.

## Q&A Session: 2026-07-16 10:35

Q: In 1-3 sentences, how does your desired agent system differ from OpenCode?
A: "Mostly a Rust-native clone, but with a stronger project system that you can manually edit and add sources/tools/configurations to (rather than it being per-repo in opencode, since I dislike the per-repo structure and how .opencode and other personal configs either have to be gitignored entirely or committed alongside code), and with a key distinction that the server is long-running rather than per-opencode process, which is better for long-running client implementers like a Kimaki-esque discord bridge, since they no longer have to do special things but rather can just behave as a normal client for tau and forward to/from discord instead of drawing a gui/tui"

Q: You said WebSocket+axum. OpenCode uses REST+SSE (WebSocket only for PTY). What's your intended wire model between server and client(s)?
A: "Decide later / advise"

Q: How should clients relate to the server? (OpenCode supports many clients to one server + an in-process 'embedded' mode sharing the same router.)
A: "Many clients, one server"

Q: Which LLM providers must work at MVP? (OpenCode supports ~10.)
A: "We should use `rig` for the main agent loop, as it is already quite mature. We will support all providers that rig supports from day one, and if we need to add more providers later we can either vendor rig and modify it or potentially use rig's API for external providers if one exists"

Q: Is this a solo build toward a polished product, or an MVP you want running fast?
A: "MVP fast, then iterate"

## Q&A Session: 2026-07-16 10:42

Q: Transport recommendation (Pure WS JSON-RPC + binary frames) — confirm?
A: "Pure WS JSON-RPC + binary frames (Recommended)" — confirmed.

Q: What exactly is a 'project' in tau? (replaces opencode's per-repo `.opencode/` config)
A: "One working directory, but each subdirectory of that directory is treated equally as a root, while the parent dir that contains them all is grouping construct only. This is ideal for different worktrees that are sibling folders of each other all inside of a single parent dir."

Q: When you said 'add sources/tools/configurations to' a project — what does 'sources' mean?
A: "Similar to how things like helix, vscode, opencode, etc. does it, a project dir should have a `.tau` folder, which contains the same content as `~/.config/tau`, and can override any arbitrary config of the editor for a specific project. Note that this is separate from the config settings in the GUI side since the GUI is an arbitrary binary that implements the client interface, so overriding client settings in a project-by-project way is not possible or planned--server/client separation should be much stronger than opencode. In fact, the server should actually support being registered as a systemd service as the first-class use-case. This is distinct from opencode's `.opencode` approach, mainly because the project folder is not a git repo, so gitignoring/committing opencode user-specific configs is not relevant"

Q: How do clients find & authenticate to the daemon?
A: "Loopback socket, no auth"

Q: Daemon startup model?
A: "Clients should not need to auto-spawn the server to conform to use. The default gui client, however, should auto-spawn if it's not up, since GUI apps have an expectation of being ootb working by double-click. If the gui client auto-started the daemon, it should toast the user with some sort of warning popup that advises them to run the daemon separately with 'okay', 'quit,' and 'don't show again' options. The tui client is more power-user friendly so if the daemon isn't up, it should just crash and print an error to launch the daemon first (as well as provide a simple help string like 'help: to fix this, run `tau serve` before running the tui')."

## Q&A Session: 2026-07-16 10:55

(Note: user corrected mid-session — **gpui is now published on crates.io** as a normal crate dependency. GUI crate will depend on the crates.io `gpui`.)

Q: Which agent-runtime features must ship at MVP?
A: Permissions/approval, Subagents/task tool, Skills, MCP client, Revert/undo snapshots, Compaction, and **Plan mode — but reworked**: "unlike opencode's structured plan mode, we should have a more detailed loop: Discrete stages, each stage is marked airtight or not airtight, etc. See the plan-architecture skill; this should be the default planning loop and should execute if we are in plan mode (different primary agents accessible via `tab` is a first-class MVP feature, since I use it a lot in opencode). Basically, we want to replace ROADMAP.md with a per-conversation `plan` tool that is structured to enforce the exact ROADMAP.md structure while also being able to integrate with tools to guard against the LLM getting overzealous and implementing before stuff is airtighted. The basic tools that opencode has should also be added, though we want hashline instead of regular oldString/newString edit (see my fork of opencode at https://github.com/quinntyx/opencode for how I want this to be implemented)."

Q: Which built-in tools at MVP?
A: read/write/edit, bash, grep/glob/list, question, task — AND: "edit tool should use hashline editing (see my fork of opencode) and plan tool should explicitly gate that it be called and that a plan that is airtighted exists before any editing tools (edit/write mainly) can be called. The specific guard should be that the LLM has to call the `plan` tool to update its status to what phase/step it's currently working on, and if it moves to a phase has not been airtighted by the user yet (unless the user is in full-autonomous mode) it will temporarily reject all tools that try to modify files (and some class of common bash commands that edit files, like `sed`, though there's little we can do if the model ignores all the errors returned by the harness and insists on editing through something like `python3 -c`) and tell the model that the stage needs to be airtighted first"

Q: How heavy should the permission/approval system be at MVP?
A: "Full ruleset (opencode-style)"

Q: How should LLM API keys be supplied/stored at MVP?
A: "Whatever OpenCode does" → OS keychain with file fallback (per provider), plus env-var support; a `tau auth`/GUI login flow to populate it.

Q: Which gpui GUI features are MVP?
A: "Chat + streaming, Tool-call cards, Permission prompts, Diff accept/reject, Model/agent picker, Session & project pickers, **A sidebar that shows you stats like the plan, LSP usage, Session input/output tokens, context tokens and percentage used, session ID, project name, etc. (the stuff that OpenCode has).**"

Q: gpui source?
## Q&A Session: 2026-07-16 11:05

Q: How should the per-conversation `plan` tool + airtighting work? (replaces ROADMAP.md, gates mutating tools)
A: "I think you misinterpreted what I meant by 'phase.' What I mean by phase is really step, but each step can have subitems to check off as per the plan-architecture skill. I want each step to be airtighted separately but also for sub-items to be allowed (eg. 5-10 checkbox items per step, 5-10 steps per project, but can vary widely from like 1 or 2 up to like 30 depending on project scale)."

Q: Where does 'full-autonomous mode' (bypass airtight gating) live?
A: "Per-session toggle, default OFF"

Q: Which primary agents ship by default (Tab cycles through these)?
A: "Plan + Code"

Q: You listed 'LSP usage' in the stats sidebar. LSP is a heavy subsystem — is it MVP or deferred?
A: "Full LSP"

Q: MCP transport breadth and compaction depth at MVP?
A: "MCP stdio + full epochs"

Q (follow-up, out-of-band): After compaction triggers, what happens to the plan?
A: "after compaction triggers, the entire ROADMAP.md (or in this case the plan tool's managed architecture contract and content, which is not really md since we are switching to a structured setup but should be rendered as such for the model's consumption) should automatically be injected back into the model's context; we just assume that the roadmap/plan text is not larger than the model's context window for now, since for the vast majority of models it shouldn't be"

## Q&A Session: 2026-07-16 11:12

Q (follow-up, design of tau's own plan/question tools): In opencode/plan-architecture, Q&A sessions are cached into ROADMAP.md by asking the model to do it. For tau?
A: "Currently, we cache Q&A sessions in ROADMAP.md. However, the harness does not need to ask the model to do this, it can do it automatically. Whenever the question tool is called, the results should automatically be cached into the session data, and the plan can then reference them in a more safe and clean way that is less prone to hallucination, by linking to a particular Q&A session's ID in the plan item."

## Q&A Session: 2026-07-16 11:30 (MVP descope)

Q: For the basic MVP, should we keep the project system?
A: "Actually, for the basic MVP, let's toss the project system for now and just do one conversation per directory and leave a comment to add the project system later; the other machinery is more important to set up first and I want to play with the app a bit after it's working to decide how the project system UI and management should work since opencode's project system has not felt very good to me and I have a hard time quantifying why"

Q: And the directory picker?
A: "The directory picker can also be descoped. For now, we don't need a picker; that falls under project system. Just treat the 'directory picker' as 'Whichever folder the tau-gui process was run from in a terminal' since while I develop this I'll primarily be running it from a terminal"

## Q&A Session: 2026-07-16 11:45 (M0 tightening)

Q: JSON-RPC impl for M0?
A: "Hand-rolled over axum WS (Recommended)" — hand-roll the JSON-RPC 2.0 envelope over `axum::extract::ws`; text frames = JSON-RPC, binary frames reserved.

Q: Crate scope at M0?
A: "Lean: proto/server/client/cli (Recommended)" — create workspace + `tau-core` (stub), `tau-proto`, `tau-server`, `tau-client`, `tau-cli`; defer `tau-tui`/`tau-gui` to M12/M13.

Q: Socket path + single-instance?
A: "XDG_RUNTIME_DIR + lockfile (Recommended)" — default socket `$XDG_RUNTIME_DIR/tau.sock` (fallback `~/.tau/tau.sock`), single-instance via advisory lockfile (`~/.tau/tau.lock`), overridable via `--socket` flag and config.

Q: systemd depth at M0?
A: "Type=simple + SIGTERM now (Recommended)" — `Type=simple` unit template + clean SIGTERM graceful shutdown now; defer Type=notify + socket-activation to M14.

Q: Toolchain?
A: "Edition 2024 + GHA, no Nix (Recommended)" — Rust edition 2024, MSRV = current stable, GitHub Actions (`fmt --check` + `clippy -D warnings` + `test`), no Nix flake.

Q: Binary-frame convention at M0?
A: "Define convention now, impl later (Recommended)" — text frames = JSON-RPC; binary frames = `[1-byte channel-id][payload]` (`0x01`=PTY, `0x02`=attachment, …); no binary channel implemented until bash/PTY lands.

## Q&A Session: 2026-07-16 12:10 (M1 tightening)

Q: Should 'session-per-directory anchoring' stay in M1 or move to M2?
A: "Config + credentials only (Recommended)" — M1 = config + credentials ONLY; session anchoring moves to M2 where the session table lives.

Q: How much of the config schema to define at M1?
A: "Minimal, grow per-milestone (Recommended)" — `Config { model, default_agent, providers{api_base, api_key_env} }`; all `Option` + `#[serde(default)]`; grow per milestone.

Q: Credential resolution precedence (env / keyring / file fallback)?
A: "env > keyring > file (Recommended)" — env wins (systemd/CI injection), then keyring (`tau auth`), then file fallback when Secret Service/Keychain is unavailable.

Q: Add JSON-RPC protocol methods in M1, or keep them server-internal?
A: "None — server-internal + `tau auth` (Recommended)" — no new protocol methods; config + credentials are server-internal state + the `tau auth` CLI.

## Behavior Invariants

- The server is a **long-running daemon** (not per-process / not spawned-and-died per client). Long-lived client implementers (e.g. a Discord bridge) must be able to act as ordinary clients over the normal client protocol — no special bridges or side channels. The daemon must be **systemd-registerable as a first-class use case**.
- **Wire protocol**: single bidirectional **WebSocket per client (axum), JSON-RPC 2.0** for all requests/responses + streamed events (token deltas, tool events, session events, permission prompts). **Binary WS frames** are multiplexed for PTY bytes and file/attachment blobs. No REST in v1.
- **Discovery/auth**: loopback socket (Unix domain socket preferred), **no auth** (trusted local user). Remote/non-loopback access is out of scope for MVP.
- **Client auto-spawn contract**: the normal client contract is "daemon already running." The **default GUI client** MAY auto-spawn the daemon (double-click expectation) and, when it does, must toast a warning ("run the daemon separately") with Okay / Quit / Don't-show-again. The **TUI client** must NOT auto-spawn: if the daemon is down, crash with an error + help string (`help: to fix this, run \`tau serve\` before running the tui`).
- **Project system — DEFERRED past MVP** (parent-dir grouping, equal subdirectory roots, `.tau/` server-config overrides). Add a `TODO` where it will slot in. **MVP = one conversation per working directory**, where the working directory is simply **the client process's cwd at launch** (no picker). Rationale (2026-07-16 11:30): user wants to use the app before designing project UI/management; opencode's project UX "has not felt very good."
- **No directory/project picker at MVP** — GUI/TUI operate on the cwd they were launched from. A picker is part of the deferred project system.
- The LLM/provider layer is **`rig`**. Do not hand-roll provider adapters; reuse rig's provider set. Adding providers means vendoring/modifying rig or using rig's external-provider API.
- Build cadence is **MVP fast, then iterate**. Cut aggressively to a working GUI agent first; defer enterprise/sharing/plugins/etc.
- **Plan tool (MVP centerpiece, replaces ROADMAP.md per-conversation)**: a structured, server-stored plan per session with the ROADMAP.md shape — Architecture Contract, Behavior Invariants, Architecture Overview, and an ordered **Roadmap of steps**, where each **step** has an `airtight` boolean plus an ordered list of **checkbox sub-items** (~5–10 sub-items per step, ~5–10 steps per project, but may range 1–2 to ~30 by scale). The model calls `plan` to read/update the plan and to set its **current step**. **Airtight gate**: if the model's current step is not yet airtighted (and the session is not in autonomous mode), the harness **rejects mutating tools** (`edit`, `write`, and a best-effort class of file-editing `bash` commands like `sed`) with a message telling the model to get the step airtighted first. Best-effort against a model that ignores errors and edits via e.g. `python3 -c`.
- **Question↔plan auto-cache (no model pasting)**: when the agent calls the `question` tool, the harness **automatically** persists the exchange as a structured **Q&A record with a stable ID** in the session data. Plan items **cite Q&A sessions by ID** (a clean link) rather than the model copying verbatim answers into plan text — less hallucination-prone. The model is never asked to hand-cache Q&A into the plan.
- **Autonomous mode**: per-session toggle, **default OFF**. When ON, the airtight gate is bypassed.
- **Default primary agents**: **Plan** (gated planning loop; read-only tools until the current step is airtighted) and **Code** (implements airtighted steps; may mutate). **Tab cycles primary agents** — first-class MVP. Agents remain fully user-configurable in config regardless.
- **Compaction (full context-epoch machinery)**: baseline system context + mid-conversation system messages + epoch reset on compaction, mirroring opencode semantics. **After each compaction, the full plan (Architecture Contract + Behavior Invariants + Architecture Overview + Roadmap, rendered as markdown for model consumption) is automatically injected into the new baseline context.** Assume the plan text fits the model context window.
- **Hashline editing**: the `edit` tool addresses lines by content-hash refs, not oldString/newString. Scheme (from `https://github.com/quinntyx/opencode` → `packages/opencode/src/tool/hashline.ts`): line hash = `sha1(line).hex()[:N].upper()`, N=3 (≤4096 lines) or 4 (>4096); anchor = `sha1(prev ‖ 0x241E ‖ line ‖ 0x241E ‖ next)[:N]`; file REV = `sha1(eol-normalized content)[:8]`; ref format `<line>#<hash>[#<anchor>]`; `read` annotates lines with `#HL <line>#<hash>#<anchor>|` + a `#HL REV:<8hex>` header; `#HL` prefixes inside replacement `content` are stripped on apply; ref/REV mismatch raises a stale/invalid error.
- **MVP feature set (authoritative)**: permissions (full opencode-style ruleset), subagents/`task`, skills, **MCP client (stdio only)**, revert/undo snapshots, **full context-epoch compaction**, **plan mode + `plan` tool with per-step airtighting**, **full LSP**. OUT of MVP (deferred): plugin system, session sharing, enterprise, cloud identity, MCP server-exposure, web client, non-stdio MCP transports.
- **MVP tool set**: `read, write, edit (hashline), bash (with sed-class edit guard), grep, glob, list, question, task, plan`.
- **GUI MVP surface**: chat+streaming, tool-call cards, permission prompts, diff accept/reject, model/agent picker (Tab cycles agents), **stats sidebar** (plan state, LSP usage/diagnostics, session input/output tokens, context tokens + % used, session ID, working-directory). **No session/project/directory picker at MVP** — the session is rooted at the client's launch cwd.

## Architecture Overview

Greenfield Rust workspace. Crates (names provisional, adjustable):

- **`tau-core`** — pure domain + runtime library (no network). Contains: config model + cascade loader (`~/.config/tau` → `<project>/.tau`), project model (parent dir + equal subdirectory roots), sessions/messages/usage/Q&A persistence (SQLite via `sqlx`), tool registry + built-in tools (`read`, `write`, hashline `edit`, `bash`, `grep`, `glob`, `list`, `question`, `task`, `plan`), permission ruleset engine, hashline module (sha1 refs/REV), plan store + airtight gate + Q&A store, revert/snapshot store, LSP client manager, skills discovery, MCP stdio client manager (via `rmcp`), compaction/context-epoch engine, and the **agent runner** that wraps `rig`'s `AgentRunner`/`AgentHook` to drive the loop and intercept each tool call (permissions, airtight gate, auto Q&A caching, streaming events, revert snapshots).
- **`tau-proto`** — JSON-RPC 2.0 method + event schema (request/response/notification types, binary-frame channel descriptors). Shared by server and clients. Typed in Rust (serde); a neutral IR is kept so additional client emitters are possible later.
- **`tau-server`** — long-running daemon binary (`tau serve`). axum `WebSocket` upgrade; one JSON-RPC connection per client; binary frames for PTY bytes + attachment blobs. Binds a loopback Unix domain socket (default `~/.tau/tau.sock` or `XDG_RUNTIME_DIR`). systemd-unit-friendly (clean SIGTERM shutdown, socket-activatable, single-instance lock). Owns the single source of truth: SQLite DB, project configs, credentials, managed outputs. No REST in v1.
- **`tau-client`** — async client library (tokio) implementing the WS/JSON-RPC protocol: high-level capability groups (`session`, `tool`, `plan`, `question`, `permission`, `model`, `agent`, `pty`, `fs`, `event`) over the stream. Consumed by every client. Exposes an event stream (token deltas, tool events, permission requests, Q&A records, plan updates, LSP diagnostics).
- **`tau-cli`** — the `tau` dispatcher binary: `tau serve`, `tau tui`, `tau auth`, `tau config …`, etc. (The GUI is a separate double-clickable binary.)
- **`tau-tui`** — minimal secondary client (e.g. `ratatui`). No auto-spawn; on missing daemon, prints `help: to fix this, run \`tau serve\` before running the tui` and exits. MVP parity: chat + streaming, tool cards, permission prompts, basic diff view, Tab agent switch (not full GUI parity).
- **`tau-gui`** — primary client, `gpui` (crates.io `gpui`). Full MVP GUI surface (see invariants). Auto-spawns the daemon if unreachable and toasts (Okay / Quit / Don't-show-again). Bridges async `tau-client` events onto gpui's main loop.
- **`tau-discord`** (reference long-lived client, can land post-MVP) — an ordinary `tau-client` consumer forwarding to/from Discord, demonstrating the daemon model. No special server support.

Data flow: client →(WS JSON-RPC)→ `tau-server` → `tau-core` agent runner →(rig)→ provider; tool calls intercepted by `AgentHook` → permission/airtight/snapshot checks → execute → stream `ToolEvent`s back over the WS; token deltas stream as `SessionEvent`s. PTY bytes ride binary frames. SQLite is the durable store; managed tool-output files live under `~/.local/share/tau/tool-output/`.

Config/credentials: API keys via OS keyring (file fallback) + env vars, populated by `tau auth` / GUI; per-provider, like opencode. **Config is global-only (`~/.config/tau`) at MVP**; the per-project `.tau/` override cascade is deferred with the project system.

# Roadmap

Top-level milestones. Each is decomposed into sub-items; the first milestone (`M0`) will be tightened to airtight before implementation begins. Milestones are loose until tightened in turn.

- [x] **M0 — Workspace skeleton, protocol, and daemon hello-world** — AIRTIGHT (tightened 2026-07-16 11:45)
   - [x] Airtight? **Yes**
   - **Workspace** (edition 2024, MSRV = stable): root `Cargo.toml` (workspace, members `tau-core` `tau-proto` `tau-server` `tau-client` `tau-cli`); `rust-toolchain.toml` (stable channel). NO `tau-tui`/`tau-gui` yet (deferred M12/M13). CI `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`. (cite: 2026-07-16 11:45 — toolchain)
   - **`tau-proto`** (`src/lib.rs`, serde-only, NO tokio): JSON-RPC 2.0 envelope — `Request<P>`, `Response<R>`, `Error{code,message,data}`, `Notification<P>`, `Id`, `ErrorCode` consts (-32700 parse / -32600 invalid req / -32603 internal / -32601 method not found / -32000 server err). Method enum: `ping` (params `()`, result `String`→`"pong"`) and `health` (params `()`, result `Health{version,uptime_ms,pid}`). `binary` module: doc-only convention `[u8 channel][payload]` with consts `CH_PTY=0x01`, `CH_ATTACHMENT=0x02`. (cite: 2026-07-16 11:45 — JSON-RPC impl + binary convention)
   - **`tau-server`** (`src/main.rs` + `src/lib.rs` so tests can drive it in-process): axum WebSocket upgrade at `/`; per-connection JSON-RPC text-frame dispatch table (`ping`, `health`); binary frames logged+ignored (reservation). Socket bind `$XDG_RUNTIME_DIR/tau.sock` (fallback `~/.tau/tau.sock`) with `--socket <path>` override. Single-instance via advisory lockfile `~/.tau/tau.lock` (held for lifetime; exit-with-message if already held). Graceful shutdown on SIGTERM/SIGINT (`tokio::signal`). `tracing`/`tracing-subscriber` logging. (cite: 2026-07-16 11:45 — socket + systemd)
   - **`tau-client`** (`src/lib.rs`, async/tokio): connect to a Unix socket path; send/recv JSON-RPC over text frames; expose `ping()`, `health()`, a generic `call()`, and an `events()` notification-stream primitive (no notifications emitted yet in M0). Binary-frame parse helpers present but unused. (cite: 2026-07-16 11:45 — JSON-RPC impl)
   - **`tau-cli`** (`src/main.rs`): `tau serve [--socket <path>]` runs the daemon; `tau ping` / `tau health` debug helpers connect via `tau-client` and print the result; `tau tui`/`tau gui`/`tau auth`/`tau config` are `unimplemented!()` stubs so the dispatcher shape exists. (cite: 2026-07-16 11:45 — crate scope)
   - **systemd**: `packaging/tau.service` — `Type=simple`, `ExecStart=%h/bin/tau serve`; note that `Type=notify` + socket-activation arrive in M14. (cite: 2026-07-16 11:45 — systemd)
   - **Test (proves it works)**: `tau-server/tests/round_trip.rs` — in-process: bind the server on a socket under `tempfile::tempdir()`, connect a `tau-client`, assert `ping()` → `"pong"` and `health()` returns version+pid. No subprocess spawn (fast/deterministic). (cite: 2026-07-16 11:45 — wire round-trip is the only behavior to prove)
   - **Behavior preservation**: none (greenfield). M0 establishes only the skeleton + wire round-trip; no agent/tools/sessions.

- [x] **M1 — Config + credentials** — AIRTIGHT (tightened 2026-07-16 12:10)
   - [x] Airtight? **Yes**
   - **Scope**: config loading + credential storage ONLY. Session-per-directory anchoring moved to **M2** (needs the session table). (cite: 2026-07-16 12:10 — scope boundary)
   - **`tau-core` config module** (`src/config.rs`): minimal schema `Config { model: Option<String>, default_agent: Option<String>, providers: BTreeMap<String, ProviderConfig> }`, `ProviderConfig { api_base: Option<String>, api_key_env: Option<String> }`, all `#[serde(default)]`, unknown fields allowed (forward-compat). File `~/.config/tau/config.toml` (`dirs::config_dir().join("tau").join("config.toml")`); missing ⇒ defaults (no error). `Config::load()` / `Config::load_from(path)`. (cite: 2026-07-16 12:10 — schema breadth)
- **`tau-core` credentials module** (`src/credentials.rs`): `CredentialStore { file_path, use_keyring }`. Resolution **env > keyring > file**. Env via provider→env map (`anthropic`→`ANTHROPIC_API_KEY`, `openai`→`OPENAI_API_KEY`, `gemini`/`google`→`GEMINI_API_KEY`, …) overridable by `ProviderConfig.api_key_env`; fallback `{PROVIDER upper}_API_KEY`. Keyring entry service `"tau"`, account = provider-id (`keyring` crate). File fallback `~/.config/tau/credentials.toml` (TOML `[provider] api_key=…`), `0600` on unix. Methods `get / set / delete / list`. `set` **always writes file first** (source of truth), then best-effort keyring — keyring v3 with no backend features uses an in-memory mock that "succeeds" but doesn't persist cross-process, so file-first is required for reliability. `CredentialStore::for_test(path)` disables keyring for hermetic tests. (cite: 2026-07-16 12:10 — precedence; updated 2026-07-16 12:25 — file-first set)
   - **Daemon startup** (`tau-server/src/lib.rs` `run`): `Config::load()` at startup; store `Arc<Config>` in `AppState`; log source path or "no config (defaults)". **No JSON-RPC methods added.** (cite: 2026-07-16 12:10 — protocol methods: none)
   - **`tau auth` CLI** (`tau-cli/src/main.rs`): subcommands `set <provider> <key>`, `get <provider>` (prints presence, never the full key), `delete <provider>`, `list`. Uses `CredentialStore::new()`.
   - **Crates added**: `toml`, `keyring`, `serde` into `tau-core`.
   - **Tests**: module tests in `tau-core/src/config.rs` (parse sample TOML, defaults when absent, unknown fields ignored) and `tau-core/src/credentials.rs` (file-fallback set/get/delete round-trip in tempdir via `for_test`; env-var override beats file; custom env-var override). (cite: 2026-07-16 12:10 — behavior to prove)
   - **Behavior preservation**: M0 daemon still builds + its round-trip test still passes; config/credentials are additive, no protocol change.

- [ ] **M2 — Persistence layer (SQLite) + sessions (cwd-anchored) + messages/usage/Q&A schema**
   - [ ] Airtight? No
   - `sqlx` migrations: sessions (with a `cwd` column — one conversation per working directory), messages, usage (tokens in/out/cached), Q&A records (id, session, question, answer, ts), plan store, snapshots/revert, permissions-saved.
   - Session/message/Q&A domain types + repository in `tau-core`. **Session-per-directory anchoring lands here** (moved from M1): a session is rooted at the **client process's cwd at launch**, passed on session create; one conversation per directory. TODO(`project-system`): multi-root projects + registry.
   - Tests: migration idempotency; session create/list/message-append + Q&A-record round-trip; session records its cwd.

- [ ] **M3 — rig integration: provider construction + single-turn completion + streaming**
   - [ ] Airtight? No
   - Provider/model registry mapping config → rig clients (all rig providers); API-key injection from credentials store.
   - Single-turn prompt + streaming token deltas through `tau-core`.
   - Tests: mock provider (`CompletionModel` impl) streaming deterministic deltas.

- [ ] **M4 — Tool registry + read/grep/glob/list + hashline read**
   - [ ] Airtight? No
   - Tool trait + registry; `read` (with `#HL` annotation + REV), `grep` (ripgrep), `glob`, `list`.
   - Hashline module (sha1 line/anchor/REV) in `tau-core`; `read` emits refs.
   - Tests: hashline round-trip (compute refs → parse → verify); ripgrep/glob on a temp tree.

- [ ] **M5 — Hashline edit + write + bash (with sed-class guard)**
   - [ ] Airtight? No
   - `edit` (ref / startRef+endRef / operations[] / fileRev staleness; strip `#HL` on apply), `write`, `bash` (managed; sed-class mutating commands flagged for airtight/permission gates).
   - Revert snapshot hook: snapshot before any mutation.
   - Tests: edit apply/verify/stale-REV cases (golden fixtures); bash sed-class detection.

- [ ] **M6 — Agent runner on rig (AgentRunner + AgentHook) + agentic loop + question tool**
   - [ ] Airtight? No
   - Wrap rig `AgentRunner`; implement `AgentHook` (`on_tool_call`/`on_completion_call`/`on_tool_result`) to: enforce permission ruleset, take pre-mutation snapshots, emit structured tool events, parallel tool concurrency config.
   - `question` tool: routes a clarifying question to the active client; **on reply, the harness auto-persists a Q&A record (stable ID) to the session store** — the model never hand-caches it.
   - Subagent `task` tool (nested isolated-context runner).
   - Tests: scripted multi-turn tool-loop against a mock provider asserting hook invocations + permission gating; `question` reply creates a Q&A record.

- [ ] **M7 — Permissions engine (full opencode-style ruleset)**
   - [ ] Airtight? No
   - Ordered allow/ask/deny rules with glob-over-(tool+args); persisted "saved" decisions; permission-request lifecycle (request → client reply → cache).
   - Tests: rule-matching matrix; persist/replay; ask-resolution.

- [ ] **M8 — Plan tool + airtight gate + Q&A citation-by-ID**
   - [ ] Airtight? No
   - Structured plan (Architecture Contract / Behavior Invariants / Architecture Overview / ordered Steps each with `airtight` + checkbox sub-items); `plan` tool methods (read/update/set-current-step/mark-subitem/airtight-step); plan items **cite Q&A records by ID** (resolved server-side into rendered context, never model-pasted).
   - Airtight gate in the agent runner: reject `edit`/`write`/sed-class-`bash` when current step not airtighted and autonomous=OFF; autonomous per-session toggle (default OFF).
   - Tests: gate rejects/permits per state; autonomous bypass; plan render-to-markdown includes cited Q&A by ID.

- [ ] **M9 — Compaction (full context-epoch) + plan re-injection**
   - [ ] Airtight? No
   - Baseline system context + mid-conversation system messages + epoch reset on compaction.
   - On compaction: render full plan (incl. cited Q&A) to markdown and inject into the new baseline (assume fits context window).
   - Tests: compaction produces a new epoch; plan present post-compaction in the assembled request.

- [ ] **M10 — Agents (Plan/Code defaults) + Tab cycling + skills**
   - [ ] Airtight? No
   - Agent config + Plan/Code defaults (Plan = read-only until airtight; Code = may mutate); primary-agent switching (Tab) surfaced via protocol.
   - Skills discovery (`.tau/skills`, global) + skill context-source injection + `skill` tool.
   - Tests: Plan agent cannot mutate pre-airtight; skill injection changes system context.

- [ ] **M11 — MCP stdio client + revert/undo + LSP client**
   - [ ] Airtight? No
   - MCP stdio client manager (`rmcp`); expose MCP tools/prompts as agent tools/commands.
   - Revert: snapshot list / rollback / commit.
   - LSP manager: spin up configured language servers; surface diagnostics as a context source + sidebar feed.
   - Tests: MCP stdio server fixture exposes a tool the agent can call; revert rollback restores files; LSP diagnostics flow on a sample project.

- [ ] **M12 — `tau-tui` minimal client**
   - [ ] Airtight? No
   - `ratatui` client: connect, chat+streaming, tool cards, permission prompts, basic diff, Tab agent switch, daemon-down crash+help.
   - Tests: snapshot render of transcript against a scripted session.

- [ ] **M13 — `tau-gui` primary client (gpui)**
   - [ ] Airtight? No
   - gpui (crates.io) app: chat+streaming, tool-call cards, permission prompts, diff accept/reject, model/agent picker + Tab cycling, **stats sidebar** (plan, LSP diagnostics, tokens in/out, context tokens + %, session ID, working-directory). **No picker at MVP** — session rooted at the process's launch cwd.
   - Auto-spawn daemon + toast (Okay/Quit/Don't-show-again); bridge async client events onto gpui main loop.
   - Tests: component-level render tests; headless smoke test driving a scripted session end-to-end.

- [ ] **M14 — Hardening: systemd unit, docs, packaging, end-to-end**
   - [ ] Airtight? No
   - First-class systemd unit + install docs; `tau serve` single-instance + socket activation; packaging (.deb/.rpm/.dmg/.msi as feasible); end-to-end happy-path test across daemon+GUI.

(Out of scope for MVP, tracked separately: **project system (multi-root projects, project registry, `.tau/` config cascade, directory/project pickers)**, plugin system, session sharing, enterprise/SSO, cloud identity, MCP server-exposure, web client, non-stdio MCP transports, `tau-discord` reference bridge.)
