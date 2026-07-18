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

## Q&A Session: 2026-07-16 12:45 (M2 tightening — round 1)

Q: Which SQLite library should the persistence layer use? The ROADMAP placeholder says 'sqlx migrations' but this wasn't a binding Q&A decision.

A: "rusqlite (Recommended)" — sync, bundled SQLite via libsqlite3-sys, no compile-time DB needed. Wrap calls in spawn_blocking for the async daemon. Simpler CI (no SQLX_OFFLINE). Single connection in Arc<Mutex> or r2d2 pool.

Q: Should M2 create ALL 7 tables (sessions, messages, usage, Q&A, plan store, snapshots/revert, permissions-saved) upfront, or just the core 4 (sessions, messages, usage, Q&A) and defer feature-specific tables to their milestones?

A: "Core 4 only (Recommended)" — sessions + messages + usage + Q&A in M2. Plan store → M10, snapshots/revert → M11, permissions → permissions milestone. Each feature adds its own migration. YAGNI.

Q: The architecture contract says 'one conversation per working directory' at MVP. When you launch tau from the same cwd again, should it resume the existing session or always create a new one?

A: "Always new + resume command" — Each launch creates a new session. A `tau resume` command (or session picker later) lets you reopen an old one. More flexible, no surprising auto-resume. **This updates the earlier "one conversation per working directory" stipulation**: sessions are still cwd-anchored (each session records its cwd), but there can be multiple sessions per directory. (cite: 2026-07-16 12:45 — supersedes 2026-07-16 11:30 "one conversation per working directory")

Q: How should messages be stored? The agent loop (M3+) will need to persist tool calls and tool results alongside text.

A: "Structured content blocks" — messages table + content_blocks child table (polymorphic: text, tool_use, tool_result). Matches LLM API structure exactly but adds join complexity and over-engineers for M2.

## Q&A Session: 2026-07-16 12:50 (M2 tightening — round 2)

Q: Should M2 wire the Db into the server's AppState (open DB + run migrations in run(), hold Arc<Db> in AppState), or is M2 purely the tau-core layer (types + repo + tests) with server integration deferred to M3?

A: "Wire into server now (Recommended)" — run() opens the DB + runs migrations, AppState gains a `db: Arc<Db>` field, AppState::default() uses in-memory SQLite for tests. M3 can then just use state.db.

Q: Should `tau resume` be added as a stub in M2 (like the existing tui/gui/config stubs), or deferred entirely to M3+?

A: "Stub in M2 (Recommended)" — Add `tau resume` as a `bail!("not implemented")` stub alongside tui/gui/config. The actual implementation needs session JSON-RPC methods from M3+.

## Q&A Session: 2026-07-16 13:15 (M3 tightening — round 1)

Q: rig 0.40's provider clients are each different types. How should tau-core abstract over providers for runtime dispatch?

A: "Enum dispatch — all providers" — A `Provider` enum with a variant for every rig completion provider (~20 variants). Each method matches on the variant. Macro-generated match arms to reduce boilerplate. Matches architecture contract "all providers from day one."

Q: The current Config has `model: Option<String>` and `providers: BTreeMap<String, ProviderConfig>`. How should the user specify which provider + model to use?

A: "Provider prefix in model" — Model field is combined: "openai/gpt-4o". Parse at runtime by splitting on "/". No new config field. **This updates the Config schema**: `model` field format is now `"<provider>/<model>"`, e.g. `"openai/gpt-4o"`, `"anthropic/claude-sonnet-4"`, `"gemini/gemini-2.0-flash"`. (cite: 2026-07-16 13:15)

Q: Does M3 add JSON-RPC methods for completion/streaming, or is it purely the internal tau-core integration layer?

A: "Add completion.stream method" — M3 adds a `completion.stream` JSON-RPC method so clients can immediately stream completions. More end-to-end testable.

Q: How should M3 tests work without real API keys?

A: "Use rig's test-utils" — Enable `rig-core/test-utils` feature in dev-deps. Use `MockCompletionModel` with scripted `MockTurn` responses + streaming. Purpose-built, no custom mock needed.

## Q&A Session: 2026-07-16 13:20 (M3 tightening — round 2)

Q: How should the `completion.stream` JSON-RPC protocol work over the WebSocket?

A: "Delta notifications + final response" — Client sends `completion.stream` request. Server sends `completion.delta` notifications (with request_id linking) as tokens arrive. Server sends the final `completion.stream` response (matching request id) with full text + usage when done. Client knows streaming is over when the response arrives.

Q: Should `completion.stream` create sessions and persist messages/usage to the DB, or just stream completions without persistence?

A: "M3 persists to DB" — `completion.stream` accepts optional `session_id`. If absent, creates a new session. Always persists user message + assistant response + usage to DB. End-to-end testable.

Q: The config already has `ProviderConfig.api_base`. Should M3 support custom base URLs for self-hosted OpenAI-compatible providers?

A: "Support api_base" — Pass `ProviderConfig.api_base` through to rig's `ClientBuilder::base_url()`. Enables self-hosted OpenAI-compatible endpoints (Ollama, vLLM, etc.).

## Q&A Session: 2026-07-16 23:17

Q: For M4, should the tool registry be an internal tau-core API only, with no new JSON-RPC tool methods or agent-loop integration until M6?
A: "Internal only (Recommended)"

Q: What path policy should M4 file/search tools use?
A: "Do what OpenCode does and have a policy system that gives access to different paths, prompt the user for allowing/denying the first time a path outside their cwd tree is accessed. The prompting part doesn't have to be implemented yet, just a policy system with a way to register available paths (defaulting to just the cwd being added) in a way that supports later addition to the ws api"

Q: What should hashline `read` return for text files?
A: "Annotated text + REV (Recommended)"

Q: What output contract should `grep`, `glob`, and `list` use in M4?
A: "Whatever OpenCode uses (explore my fork's codebase at https://github.com/quinntyx/opencode)."

Q: Should M4 define bounded output and resource limits for all tools?
A: "Yes, explicit defaults (Recommended)"

## Q&A Session: 2026-07-16 23:18

Q: What internal tau-core tool API should M4 expose?
A: "Typed registry (Recommended)"

Q: The fork separates typed core outputs from model-facing rendered text. Which should tau implement in M4?
A: "Both layers (Recommended)"

Q: When a tool targets a path outside the registered access roots, what should M4 do before interactive prompting exists?
A: "Typed approval-needed error (Recommended)"

Q: Should M4 `read` support directories as well as files?
A: "Files and directories. Directories should also use hashline references and numeric IDs, and if you edit a directory, it should do something similar to `oil.nvim` and apply the filesystem changes. This should be made very clear to the model in the tool description, since it's a departure from what most harnesses do. Deleting folders that contain contents through this method should throw an error, so it's primarily for management and creation of files as well as an improvement on `ls`."

Q: For M4 `grep`/`glob`/`list`, which fork semantics should be binding?
A: "Fork semantics (Recommended)"

## Q&A Session: 2026-07-16 23:19

Q: For directory `read`, should each sorted entry be rendered as a virtual hashline such as `#HL 1#<hash>#<anchor>|entry-name/`, with the numeric line number serving as the entry ID for future directory edits?
A: "Do what oil.nvim does. Oil has an invisible number id prepended to each line; in this case, it wouldn't be invisible since the model has to see it too. The ideal is `<hashlineref>|01|entry-name/` in my opinion, with stable numeric IDs assigned in alphabetical order so that they're predictably stable for the model, and HL refs just used for when the model calls the edit tool pointed at a directory. The IDs are in alphabetical order, so adding a new file can shift them, but this is predictable to the model and it should easily be able to understand what happened rather than getting confused; this is distinct from line numbers. Using line numbering would make the numerical ID for each folder unstable across different directory edits, especially if more invasive changes (like making multiple files) was done."

Q: Should registered external access roots live in a per-tool-registry/per-session policy object, with the initial cwd canonicalized and automatically registered, and later roots added explicitly by the future permission flow?
A: "Per session registry (Recommended)"

Q: Should these fork defaults be binding M4 defaults: read 2,000 lines, 50 KiB output, 2,000 characters per line; grep 100 matches; glob 100 results; directory reads 2,000 entries?
A: "Use these exact defaults (Recommended)"

Q: Should directory editing and the oil.nvim-like filesystem mutation behavior be explicitly deferred to M5, with M4 implementing only directory read/list references and tests?
A: "Move mutation into M4"

## Q&A Session: 2026-07-16 23:20

Q: Since directory mutation requires an edit operation, should M4 also move the general hashline `edit` tool from M5 into M4, so file and directory edits share one implementation?
A: "oh wait, edit tool is in M5? My bad i thought all the tools were in M4, I didn't read carefully enough. If the edit tool for files is in M5, then edit tool for dirs should be in M5 too."

Q: Should standalone `write` remain in M5, or move into M4 because directory editing must support creating files?
A: "Keep write in M5 (Recommended)"

Q: Which directory edit operations must M4 support?
A: "N/A"

Q: Should directory edits be submitted as hashline-addressed text edits against the rendered virtual directory, with the directory revision checked before applying changes?
A: "Hashline virtual-directory edit (Recommended)"

## Q&A Session: 2026-07-16 23:21

Q: How should we handle the mismatch between the pushed M3 implementation and the earlier all-rig-provider contract?
A: "Reopen M3 first (Recommended)"

## Q&A Session: 2026-07-16 23:22

Q: How should M4 `read` handle binary files, given the fork supports image/PDF attachments but tau's binary channel is not yet wired?
A: "Typed binary result (Recommended)"

## Q&A Session: 2026-07-17 00:46

Q: Should M5’s file `edit` follow the fork’s exact hashline edit contract: single `ref`, `startRef`/`endRef`, or ordered `operations[]`, with file REV validation, line/anchor validation, stale-reference rejection, and stripping `#HL` prefixes from replacement content?
A: "Exact fork contract (Recommended)"

Q: For M5 directory edits against the virtual directory output, which operations should be supported?
A: "Create/rename/delete-empty (Recommended)"

Q: What should standalone `write` guarantee?
A: "Atomic overwrite (Recommended)"

Q: What should M5’s bash tool do before the M8 permission/airtight gate exists?
A: "Execute plus classify (Recommended)"

Q: What snapshot behavior belongs in M5?
A: "Capture-only primitive (Recommended)"

## Q&A Session: 2026-07-17 04:11

Q: Confirm the recommended M5 defaults: content-addressed temp snapshots; platform shell with 120-second timeout, 2,000 lines, and 50 KiB head/tail output; ID-addressed directory operations with parent/name/kind/content creation fields.
A: "Confirm recommended defaults"

## Q&A Session: 2026-07-17 10:57

Q: How should M6-M13 airtightening and implementation orchestration proceed?
A: "Okay, well, let's do airtighting for M6-M13. Make sure to ask at least 30 questions, since we're airtightening multiple milestones at once but I don't want that to mean that any individual milestone gets less attention than if we were doing them one at a time. After we airtight all those, then divide up the work to be done into gui, server, etc. and orchestrate multiple worker agents on separate worktrees to tackle those tasks. Since ROADMAP.md is untracked, you will need to copy the relevant section of the roadmap into those branches, including the Q&A sessions and arch contract, before forking the agents"

## Q&A Session: 2026-07-17 10:57 r1

Q: Should tau replace its custom stream wrapper with Rig’s real AgentRunner and AgentHook, keeping tau-server as a thin transport?
A: "Use Rig runner (Recommended)"

Q: What structured lifecycle should every tool call emit?
A: "Full lifecycle (Recommended)"

Q: Should tool-call IDs remain stable across provider calls, persistence, retries, events, and compaction?
A: "Whatever rig's dynamic routing uses"

Q: When the question tool is called, how should client ownership and suspension work?
A: "Any client answers"

Q: How should a batched question-tool reply be persisted?
A: "Record per item (Recommended)"

Q: Which default limits should task subagents use?
A: "Similar to claude code, we want separate \"tiers\" you can enable. Exposed on the gui as a dropdown on the bar. For now, we just want tiers Unlimited (as it sounds, unlimited), Max (depth 3, eight tasks), xhigh (depth 2, eight tasks), high (depth 2, four tasks), medium (depth 1, four tasks), and low (depth 1, 1 task)."

Q: May independent read-only tools run concurrently while mutations remain serialized?
A: "Parallel reads (Recommended)"

Q: How should unfinished turns appear after daemon restart?
A: "Whatever OpenCode does"

## Q&A Session: 2026-07-17 10:57 r2

Q: Should permission matching use a canonical subject containing tool name plus deterministically serialized arguments?
A: "Canonical subject (Recommended)"

Q: What should happen when no permission rule matches?
A: "Risk-based default, except network access should be allowed by default"

Q: Which rule scopes should exist at MVP?
A: "Global and session (Recommended)"

Q: Where should saved permission decisions live?
A: "SQLite + global rules in config file(s), but I want the config files to use KDL because that's my favorite config language"

Q: Which approval choices should clients expose?
A: "Five choices, but rename deny to reject since that's what opencode calls it"

Q: When multiple clients are attached, who may answer a permission request?
A: "Initiator owns (Recommended)"

Q: What should happen if nobody answers a permission request?
A: "Wait indefinitely. More broadly, anything that blocks on requiring user input should not ever time out, it should just wait for you to click it. Many times, when using an agent, you get up to get coffee, and if the agent session exited because of a timeout by the time you get back, that would suck"

Q: How should a client that cannot render prompts handle an ask decision?
A: "Up to the client. After all, the client can just say that the user said yes even if they didn't. Therefore, we don't need to have any special support for clients that can't render ask prompts, it's their job to either render ask prompts or say \"yes\" or \"no\" automatically from their end"

Q: Should any operations be non-saveable hard denials regardless of user permission rules?
A: "Minimal hard denies (Recommended)"

## Q&A Session: 2026-07-17 10:57 r3

Q: Should each persisted plan contain Architecture Contract, Behavior Invariants, Architecture Overview, ordered steps/items, Q&A citations, current step, and revision?
A: "Full shape (Recommended)"

Q: Should plan mutations require optimistic revision matching?
A: "Revision checks (Recommended)"

Q: Who may mark a plan step airtight?
A: "Human only unless in autonomous mode, in autonomous mode you should be able to set a capable \"steering model\" that can be given a short described creative vision or guidelines and will respond to question and user interaction prompts on behalf of the user"

Q: When should editing an airtight step revoke its airtight status?
A: "Material edits only (Recommended)"

Q: How should plan Q&A citations be represented and rendered?
A: "IDs resolved server-side (Recommended)"

Q: Which operations should the airtight gate block?
A: "All classified mutations (Recommended)"

Q: What may happen when no plan or current step exists?
A: "Planning reads only (Recommended)"

Q: What should per-session autonomous mode bypass?
A: "As mentioned earlier, it shouldn't disable airtighting or any features, it simply lets you pick a capable model to stand in for a human and steer the implementation and planning agents for you until a target goal is achieved"

Q: Should autonomous mode persist and be announced to every attached client?
A: "Persist and announce (Recommended)"

## Q&A Session: 2026-07-17 10:57 r4

Q: What should trigger automatic context compaction?
A: "Provider-aware, but base it on the compaction model's context window, not on the main model's context window. If the compaction agent uses a model with a longer context window than the main agent, we should totally use 100% of the main agent's context window and only autocompact when we get an error from the remote that our prompt contains too many tokens (or a local if statement check prior to making the request if we have reliable context window size numbers for that provider, which removes the network reliance and makes it snappier-feeling)."

Q: Should clients and agents be able to request manual compaction?
A: "Clients and agents (Recommended)"

Q: Which model should summarize an epoch?
A: "When declaring a primary agent, it should be mandatory (rejected by config validator) to declare a compaction agent for it. This is a very short one-line declaration so it does not add meaningful boilerplate to the agent definition, and it makes the user consider compaction as a first-class consideration, which is good as longer runs will rely on it heavily. The built in `build` and `plan` agents should each be configured to have the same compaction model as the user's choice of primary model. Also, compaction should be implemented as a non-primary agent, similar to how OpenCode does it, so you can override the compaction agent's behavior and prompt however you like just like any other agent"

Q: What must survive verbatim in the new epoch baseline?
A: "Full active state (Recommended)"

Q: How should large tool results survive compaction?
A: "Have the model return a list of line ranges of important lines (noted via hash ref ranges, see hashline edit tool spec), and a summary of the rest, and include that. It should be prompted so that if there is an artifact/snapshot ID that is important, that line needs to be preserved, and the hashline refs of course need to be injected as prefixes when a tool result is being sent to the compaction model"

Q: Should every context epoch and summary be persisted in SQLite?
A: "Append-only epochs (Recommended)"

Q: What happens if automatic compaction fails?
A: "Keep epoch and report (Recommended)"

Q: Should clients see estimated context tokens separately from provider-reported usage?
A: "Expose both (Recommended)"

## Q&A Session: 2026-07-17 10:57 r5

Q: What fields should configurable agent definitions support?
A: "Full agent schema (Recommended)"

Q: Should built-in planning and implementation agents be ordinary overridable config entries?
A: "Overridable defaults (Recommended)"

Q: When may the active primary agent change?
A: "Next turn only (Recommended)"

Q: Should Tab cycling call the server and persist the selected primary agent?
A: "Server-backed (Recommended)"

Q: Where should skills be discovered at MVP?
A: "Global plus cwd (Recommended)"

Q: How should skill instructions enter context?
A: "Metadata then tool (Recommended)"

Q: What may a skill contribute at MVP?
A: "Full parity with Claude Code (see the Claude Platform Docs Agent Skills Page: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)"

Q: Should skill loading be persisted and rendered as a structured event/card?
A: "Persist event (Recommended)"

## Q&A Session: 2026-07-17 10:57 r6

Q: Should MCP use the rmcp crate rather than a custom protocol implementation?
A: "Use rmcp (Recommended)"

Q: What should each configured MCP server entry contain?
A: "Full process config (Recommended)"

Q: Who should own MCP server processes?
A: "Daemon manager (Recommended)"

Q: Should MCP tools use the same permission, airtight, event, persistence, and snapshot pipeline as builtins?
A: "Same pipeline (Recommended)"

Q: How should MCP prompts be exposed?
A: "Typed commands (Recommended)"

Q: What should committing a snapshot boundary mean?
A: "Create Git commit"

Q: Which rollback operations should users have?
A: "Boundary and through (Recommended)"

Q: What should rollback do if files changed externally after capture?
A: "Refuse unless forced (Recommended)"

Q: How should language servers be selected and started?
A: "Configured lazy manager (Recommended)"

Q: Which LSP capabilities belong in MVP?
A: "Full editing set (Recommended)"

Q: How should diagnostics be retained and exposed?
A: "Live plus contextual persistence (Recommended)"

Q: How should crashed MCP and LSP processes recover?
A: "Bounded restart (Recommended)"

## Q&A Session: 2026-07-17 10:57 r7

Q: Should the TUI provide full agent-execution capabilities while remaining visually minimal?
A: "Functional parity (Recommended)"

Q: What terminal input behavior is required?
A: "Multiline editor (Recommended)"

Q: How should permission requests appear in the TUI?
A: "Inspect the OpenCode TUI and copy their implementation verbatim, I like it a lot."

Q: What diff review should the TUI support?
A: "left/right split view with hunk-by-hunk accept or reject (similar to Windsurf and Cursor's UI, but in a TUI instead). Should support mouse mode, also arrow keys or tab navigation to go from one hunk through the next, and backspace to delete the change or enter to accept it with `u` for undo and `U` for redo in a helix/vim esque way so you can correct accidental acceptance or rejection of a hunk. Also have a way to accept a whole file."

Q: How should Tab behave outside modal dialogs?
A: "Cycle agents (Recommended)"

Q: What should expandable tool cards show?
A: "Inspect OpenCode's implementation. I like it a lot. (my fork specifically, at https://github.com/quinntyx/opencode). The tl;dr is that it only shows name and result, but you can right click it to inspect and get all the details, including the raw JSON for the call"

Q: How should the TUI choose its model?
A: "Config plus selector (Recommended)"

Q: What TUI test evidence is required?
A: "Snapshots and scripted flow (Recommended)"

## Q&A Session: 2026-07-17 10:57 r8

Q: Should tau-gui be a separate double-clickable binary while `tau gui` launches it?
A: "Separate binary (Recommended)"

Q: What text input implementation should the GUI use?
A: "Proven component first (Recommended)"

Q: Which structured transcript card types are required?
A: "Complete event set (Recommended)"

Q: Where should permission and diff decisions appear?
A: "Do what OpenCode does (modal every time, it appears over the chat bar). Inspect their exact impl for the TUI version and copy it, and for the GUI version, try to faithfully reproduce the UX in GUI form"

Q: What should the model picker support?
A: "Do what OpenCode does (dialog modal in the middle of the screen, you have model name on left with provider on the right, a recently used section, a favorites system, and a search bar). As with the previous question, for the TUI, faithfully copy OpenCode's implementation and for the GUI, do a UX-preserving faithful reproduction"

Q: What should the agent picker support?
A: "Search plus Tab plus `/agent` command that lets you do eg. `/agent build` (with autocomplete like all the other commands, see OpenCode's UX) to set a particular agent name to use. Also agent config should be able to set whether it is in the main `tab` cycle, that way agents that are rarely used don't have to be tabbed through every time but can still be loaded either via search UI or via `/agent`"

Q: How broad should the MVP stats sidebar be?
A: "Full operational sidebar (Recommended)"

Q: Should sidebar layout and visual preferences persist as GUI-only settings?
A: "Persist client settings (Recommended)"

Q: How should “Don’t show again” for daemon auto-start be persisted?
A: "GUI preferences file (Recommended)"

Q: What happens to a daemon auto-started by the GUI when the GUI exits?
A: "Make the popup that tells the user to change behavior let the user choose. Add options \"disown\" and \"always disown\" which make it so that you can make the daemon independent, or always automatically make the daemon independent. If the user doesn't select this option, then the daemon should die with the GUI when it closes, since GUI apps that leave services running when they close without telling the user is bad design"

Q: Which startup states should the GUI represent?
A: "Detailed states (Recommended)"

Q: What GUI test evidence is mandatory?
A: "Components and E2E (Recommended)"

## Q&A Session: 2026-07-17 10:57 r9

Q: Should all M6-M13 behavior use typed JSON-RPC methods/events rather than parsing display strings?
A: "Typed only (Recommended)"

Q: Should events carry monotonically increasing per-session sequence numbers?
A: "Sequenced events (Recommended)"

Q: Should reconnecting clients request events after their last sequence number?
A: "Replay after sequence (Recommended)"

Q: Should clients and daemon negotiate protocol version and capabilities at connection setup?
A: "Version and capabilities (Recommended)"

Q: What should model-turn cancellation do?
A: "Propagate and persist (Recommended)"

Q: Should existing completion.stream remain temporarily compatible while richer session-turn APIs land?
A: "Immediate replacement"

Q: How should M6-M13 SQLite changes be delivered?
A: "Forward migrations with upgrade tests (Recommended)"

Q: Should final acceptance include one scripted flow covering plan, airtightening, permission, mutation, snapshot, diff, persistence replay, and compaction?
A: "Require full E2E (Recommended)"

## Q&A Session: 2026-07-17 10:57 r10

Q: Confirm tau should preserve Rig’s full identity hierarchy: RunId, turn number, internal_call_id, provider tool ID, and optional provider call_id?
A: "Preserve full hierarchy (Recommended)"

Q: OpenCode keeps session history but does not resume live turns, questions, permissions, or tool settlement after restart. Should tau match this?
A: "Match OpenCode (Recommended)"

Q: Does “wait indefinitely” apply only while the daemon remains alive, with restart turning pending prompts into interrupted state?
A: "Yes, process lifetime (Recommended)"

Q: Which subagent tier should be the default for a new session?
A: "Medium (Recommended)"

Q: Where should the selected subagent tier live?
A: "Persist per session (Recommended)"

Q: Should Unlimited literally remove both depth and concurrency limits, while retaining cancellation, permissions, and resource accounting?
A: "Literal unlimited (Recommended)"

Q: Should KDL replace TOML for all tau configuration files, while the credentials fallback remains its existing protected credential format?
A: "All config to KDL (Recommended)"

Q: How should existing config.toml files be handled?
A: "Immediate break"

Q: Confirm unmatched network access is allowed by default, but explicit reject rules still apply and network-capable MCP tools remain classified for display?
A: "Allow but classify (Recommended)"

## Q&A Session: 2026-07-17 10:57 r11

Q: Which human-interaction prompts may the autonomous steering agent answer?
A: "Two-tier. Questions and airtight only by default, with a super mode that lets the model hook up to everything, but when you enable super mode gives you a popup asking you to accept the risks of a fully autonomous run (and super mode enablement is not persisted, you have to turn it on every time before you start a super mode run, but you can hide the danger popup forever with a \"don't show this\" button if you use super mode often)"

Q: How should a session select its steering agent?
A: "Named agent plus vision (Recommended)"

Q: How should an autonomous run decide that its target goal is achieved?
A: "Steering decision plus final report (Recommended)"

Q: Should the steering agent review every completed plan step before the primary agent proceeds?
A: "Review every step (Recommended)"

Q: Must config validation ensure a compaction model can ingest at least the primary model’s maximum context?
A: "Allow smaller, and just trigger compaction when you reach 80% of the configured compaction model's maximum context. However, warn the user that this means compaction will occur occasionally even when the primary model is not at capacity yet."

Q: When the primary provider rejects an oversized prompt, should tau compact and automatically retry that model turn once?
A: "Compact and retry once (Recommended)"

Q: Should compaction-preserved hashline ranges reference an immutable stored copy of the original tool result?
A: "Immutable artifact (Recommended)"

Q: For Claude Agent Skills parity, should tau implement standard SKILL.md YAML frontmatter plus scripts, references/resources, and assets directories?
A: "Standard parity (Recommended)"

Q: How should scripts shipped inside a skill execute?
A: "Normal tool pipeline (Recommended)"

Q: Should skills be invokable both automatically and through slash commands?
A: "Automatic and slash (Recommended)"

## Q&A Session: 2026-07-17 10:57 r12

Q: Is there any additional Claude Skills behavior that must be included in the parity requirement?
A: "I also don't know if you know about it, but claude skills have an additional thing where you can have commands that are in the skill markdown adn when the model reads the skill, those commands are run and the section is replaced by the command output, so you can for example have a `git diff` command and instead of relying on the model to call git diff, it will just call it when the skill is loaded and put the output into the spot where you put `git diff`, look at Claude's exact implementation in more detail and make sure stuff like this is also supported"

## Q&A Session: 2026-07-17 10:57 r13

Q: Confirm tau should support Claude Code dynamic context syntax `!`command`` inside SKILL.md, executed before the model receives the skill?
A: "Exact syntax and timing (Recommended)"

Q: Should dynamic skill commands pass through normal permissions and wait indefinitely for approval before skill loading continues?
A: "Trust installed skills"

Q: What should happen when a dynamic skill command fails or is rejected?
A: "Insert structured failure (Recommended)"

Q: What limits should dynamic skill commands use?
A: "Bash tool limits (Recommended)"

Q: Should a snapshot “commit” create a real Git commit on the session’s currently checked-out branch?
A: "User's choice (in config files). Default to dedicated tau branch."

Q: What may an automatic snapshot commit stage?
A: "Tau-touched paths only (Recommended)"

Q: How should automatic Git commits be authored and messaged?
A: "User Git identity"

Q: What should snapshot commit do when the session cwd is not inside a Git repository?
A: "session cwd is never a git repository, because it's a plain folder with worktrees inside of it (cwd/main would be the git repository). We should automatically turn the session cwd into a tau git repo when tau starts and the user points it there, and use that git repo for managing everything tau-related."

Q: Should diff acceptance occur after a tool mutates the workspace but before its result is returned to the model?
A: "Pause after mutation (Recommended)"

Q: When a user rejects individual hunks, should tau restore only those hunks and return the accepted partial result to the model?
A: "Partial restore (Recommended)"

## Q&A Session: 2026-07-17 10:57 r14

Q: Does this instruction intentionally replace the prior MVP rule that the launch cwd itself is the one source root?
A: "No, support both (Recommended)"

Q: If cwd is a grouping directory, how should tau determine source roots?
A: "Immediate child directories (Recommended)"

Q: What should the Git repository initialized at the grouping cwd track?
A: "Tau metadata only (Recommended)"

Q: Where should accepted source-code snapshot commits be created?
A: "Each child repository (Recommended)"

Q: If one tool call changes multiple child repositories, how should commit boundaries work?
A: "Commit each plus manifest (Recommended)"

Q: For the default dedicated tau branch, should each child worktree switch to a tau-managed branch before mutations?
A: "Use separate tau worktrees (Recommended)"

Q: May tau automatically run `git init` in the grouping cwd without a prompt?
A: "Automatic and visible (Recommended)"

Q: Does “Trust installed skills” apply equally to global and cwd-local skills?
A: "Global trusted, Per-skill trust for local skills"

Q: May a trusted skill’s dynamic `!` command mutate files during context expansion?
A: "Read-only commands only (Recommended)"

Q: Which Git snapshot mode should ship as the default KDL value?
A: "Managed worktrees (Recommended)"

## Q&A Session: 2026-07-17 10:57 r15

Q: OpenCode uses a staged modal with approval, always-allow confirmation, and reject-message entry. Should tau preserve that UX while expanding it to the five configured decisions?
A: "Faithful expanded modal (Recommended)"

Q: The fork supports click expansion and an Inspect action, but no confirmed right-click binding. Should tau still add your requested right-click raw-JSON inspector?
A: "It does have a right-click binding, it's on right-clicking tool call cards. I'm using it right now. 

Nonetheless, I want the right-click in tau of course"

Q: Should tau deliberately use singular `/agent <name>` even though the inspected fork currently registers `/agents`?
A: "`/agents` opens a picker modal for search. `/agent <agentname>` allows a specific agent to switch to. I don't see a conflict here. The reason why `/agents` was used in OpenCode is because their command system is too simplistic and doesn't have rigorous arguments with autocomplete support, but we are going to have that, so we can have a good experience with `/agent <agent_name>` to switch directly while still preserving `/agents` (and probably a GUI button) to search the installed agents and pick one."

Q: Which actions should the auto-spawn warning show?
A: "Okay Quit Disown Always-disown Never-show-again"

Q: If the warning is hidden forever but ownership remains child-owned, should future GUI exits still stop auto-spawned daemons silently?
A: "Yes, stop silently (Recommended)"

Q: How should a child-owned daemon be stopped when the GUI exits?
A: "Graceful then kill (Recommended)"

Q: Confirm the GUI must never stop a daemon that was already running before it connected?
A: "Never own existing (Recommended)"

Q: How should `tau gui` launch the separate tau-gui binary?
A: "Replace process (Recommended)"

Q: When a mutation is awaiting hunk review, who may decide it?
A: "Initiating client owns (Recommended)"

Q: How should TUI/GUI hunk-review undo/redo work?
A: "Per review transaction (Recommended)"

## Q&A Session: 2026-07-17 10:57 r16

Q: What should the two built-in primary agents be named?
A: "plan and build (Recommended)"

Q: Should each primary-agent KDL entry reference a named non-primary compaction agent?
A: "Named agent reference (Recommended)"

Q: In autonomous super mode, may the steering agent answer permissions, diff reviews, rollback conflicts, and Git commit confirmations?
A: "All soft prompts (Recommended)"

Q: Confirm super-mode enablement applies to one autonomous run only, while “never show warning” is a persistent GUI/TUI preference?
A: "One run plus saved warning pref (Recommended)"

Q: How many active primary-agent turns may one session have?
A: "One active, queue rest (Recommended)"

Q: If multiple clients submit while a turn is active, how should queued turns be ordered?
A: "Server sequence order (Recommended)"

Q: Who may cancel an active turn?
A: "Any attached client (Recommended)"

Q: Which events belong in the durable sequenced journal?
A: "All state transitions (Recommended)"

Q: Should mutating JSON-RPC requests require client-generated idempotency keys?
A: "Require keys (Recommended)"

Q: On platforms without Unix exec, how should `tau gui` launch tau-gui?
A: "Spawn detached"

Q: After tau commits accepted changes on managed branches/worktrees, how should users publish them to their normal branches?
A: "Explicit integrate action (Recommended)"

Q: When should accepted tau changes become Git commits?
A: "Explicit snapshot commit (Recommended)"

## Q&A Session: 2026-07-17 10:57 r17

Q: If tau launches directly in a single source root that is not a Git repository, should it initialize Git there for snapshot commits?
A: "Initialize visibly (Recommended)"

Q: Should tau-created source commits run repository hooks and signing configuration?
A: "Normal Git behavior (Recommended)"

Q: How should managed branches/worktrees be named and cleaned up?
A: "Model-named branches"

Q: How should diff review handle binary files, renames, and deletions that cannot be hunk-reviewed?
A: "Whole-file decisions (Recommended)"

Q: Should all immediate child directories become approved read roots automatically, while mutations still use permission and airtight gates?
A: "Approve child roots (Recommended)"

Q: Does this reintroduce only root grouping/worktree management, while project picker and per-project config cascade remain deferred?
A: "Minimal grouping only (Recommended)"

Q: Should durable tool events store bounded inline output plus immutable artifact references for full output?
A: "Bounded plus artifacts (Recommended)"

Q: Should GUI/TUI client preferences use separate KDL files under the platform config directory?
A: "Separate client KDL (Recommended)"

## Behavior Invariants

- The server is a **long-running daemon** (not per-process / not spawned-and-died per client). Long-lived client implementers (e.g. a Discord bridge) must be able to act as ordinary clients over the normal client protocol — no special bridges or side channels. The daemon must be **systemd-registerable as a first-class use case**.
- **Wire protocol**: single bidirectional **WebSocket per client (axum), JSON-RPC 2.0** for all requests/responses + streamed events (token deltas, tool events, session events, permission prompts). **Binary WS frames** are multiplexed for PTY bytes and file/attachment blobs. No REST in v1.
- **Discovery/auth**: loopback socket (Unix domain socket preferred), **no auth** (trusted local user). Remote/non-loopback access is out of scope for MVP.
- **Client auto-spawn contract**: the normal client contract is "daemon already running." The default GUI may auto-spawn it. A GUI-spawned daemon is child-owned by default and stops gracefully, then forcefully if needed, with that GUI. Warning actions: Okay / Quit / Disown / Always-disown / Never-show-again; warning visibility and ownership are independent preferences. Existing daemons are never owned. `tau gui` execs the separate `tau-gui` binary on Unix and spawns it detached where exec is unavailable. The TUI never auto-spawns and prints `help: to fix this, run \`tau serve\` before running the tui`. (cite: 2026-07-17 10:57 r8, r15-r16)
- **Project system remains deferred, but minimal root grouping is restored**: launch cwd may be one source root or a grouping-only parent whose immediate child directories are equal approved read roots. There is no project picker, registry, or per-project config cascade. Tau visibly initializes a tau-metadata Git repository at a grouping cwd; accepted source commits occur in child repositories. A directly launched non-Git source root is initialized visibly. (cite: 2026-07-17 10:57 r13-r14, r17)
- **No directory/project picker at MVP** — GUI/TUI operate on the cwd they were launched from. A picker is part of the deferred project system.
- The LLM/provider layer is **`rig`**. Do not hand-roll provider adapters; reuse rig's provider set. Adding providers means vendoring/modifying rig or using rig's external-provider API.
- Build cadence is **MVP fast, then iterate**. Cut aggressively to a working GUI agent first; defer enterprise/sharing/plugins/etc.
- **Plan tool (MVP centerpiece, replaces ROADMAP.md per-conversation)**: a revisioned, structured, server-stored plan with Architecture Contract, Behavior Invariants, Architecture Overview, ordered steps/items, Q&A IDs, current step, and per-step airtight state. Only a human—or the steering agent during an autonomous run—may grant airtight status. Material edits revoke it. Every classified mutation across builtins, MCP, LSP, and bash is blocked until the current step is airtight. (cite: 2026-07-17 10:57 r3)
- **Question↔plan auto-cache (no model pasting)**: when the agent calls the `question` tool, the harness **automatically** persists the exchange as a structured **Q&A record with a stable ID** in the session data. Plan items **cite Q&A sessions by ID** (a clean link) rather than the model copying verbatim answers into plan text — less hallucination-prone. The model is never asked to hand-cache Q&A into the plan.
- **Autonomous runs use a steering agent rather than bypassing product behavior**. Normal mode lets a named steering agent answer questions and grant airtight status at every step. Super mode may answer every soft prompt, but never hard denials; it lasts one run and requires a risk confirmation, though warning suppression may persist. Steering agent, vision, run state, and final report are server-backed. (cite: 2026-07-17 10:57 r3, r11, r16)
- **Default primary agents**: **plan** and **build**, represented as overridable KDL entries. Every primary references a named non-primary compaction agent. Tab cycles only configured cycle members; `/agent <name>` switches directly and `/agents` opens search. Switching is server-backed, persisted, and applies next turn. (cite: 2026-07-17 10:57 r4-r5, r8, r16)
- **Compaction**: append-only SQLite epochs, provider-aware accounting, manual compaction, and one compact-and-retry after an oversized prompt. Automatic threshold is 80% of the configured compaction model's known window; smaller compactors are allowed with a warning. Tool results become hashline-annotated immutable artifacts; the compactor selects important ranges and summarizes the rest. Full active state and the plan with resolved Q&A enter every new baseline. (cite: 2026-07-17 10:57 r4, r11)
- **Hashline editing**: the `edit` tool addresses lines by content-hash refs, not oldString/newString. Scheme (from `https://github.com/quinntyx/opencode` → `packages/opencode/src/tool/hashline.ts`): line hash = `sha1(line).hex()[:N].upper()`, N=3 (≤4096 lines) or 4 (>4096); anchor = `sha1(prev ‖ 0x241E ‖ line ‖ 0x241E ‖ next)[:N]`; file REV = `sha1(eol-normalized content)[:8]`; ref format `<line>#<hash>[#<anchor>]`; `read` annotates lines with `#HL <line>#<hash>#<anchor>|` + a `#HL REV:<8hex>` header; `#HL` prefixes inside replacement `content` are stripped on apply; ref/REV mismatch raises a stale/invalid error.
- **MVP feature set (authoritative)**: full permissions, tiered subagents/`task`, Claude-compatible Agent Skills including progressive disclosure and trusted read-only dynamic `!\`command\`` expansion, stdio MCP via `rmcp`, Git-backed revert/undo, full context epochs, structured plan/airtighting, and full LSP. OUT of MVP: plugin system, session sharing, enterprise, cloud identity, MCP server-exposure, web client, and non-stdio MCP. (cite: 2026-07-17 10:57 r5-r6, r12-r14)
- **MVP tool set**: `read, write, edit (hashline), bash (with sed-class edit guard), grep, glob, list, question, task, plan`.
- **GUI MVP surface**: typed chat/event cards, OpenCode-faithful permission modal above the chat bar, hunk/whole-file diff review, OpenCode-faithful model picker, server-backed agent picker/commands, full operational stats sidebar, detailed startup states, and separate KDL client preferences. No session/project/directory picker. (cite: 2026-07-17 10:57 r7-r8, r15, r17)

## Architecture Overview

Greenfield Rust workspace. Crates (names provisional, adjustable):

- **`tau-core`** — domain/runtime library. Uses `rusqlite`; wraps Rig's real `AgentRunner`/`AgentHook`; adapts builtin/MCP/skill tools as Rig `DynamicTool`s with full JSON schemas; owns permissions, plans/Q&A, steering runs, context epochs, immutable artifacts, snapshots/Git/worktree orchestration, skills, MCP, and LSP. Rig identity is preserved as `RunId → turn → internal_call_id → provider IDs`. Blocking DB/Git/process work never runs directly on async runtime workers. (cite: 2026-07-17 10:57 r1, r6, r10, r13-r17)
- **`tau-proto`** — JSON-RPC 2.0 method + event schema (request/response/notification types, binary-frame channel descriptors). Shared by server and clients. Typed in Rust (serde); a neutral IR is kept so additional client emitters are possible later.
- **`tau-server`** — thin long-running transport/orchestration host. It negotiates protocol version/capabilities, admits one active FIFO-queued turn per session, broadcasts typed sequenced events, waits indefinitely for live user input, replays durable events, owns integration processes, and delegates agent/tool policy to tau-core. Existing `completion.stream` is replaced immediately. (cite: 2026-07-17 10:57 r1-r2, r9, r16)
- **`tau-client`** — async typed JSON-RPC client with session/turn/event replay, tool, plan, question, permission, diff, snapshot/Git, model, agent, MCP, LSP, and client-state capability groups. It never parses presentation strings for semantics. Mutating requests carry idempotency keys. (cite: 2026-07-17 10:57 r9, r16)
- **`tau-cli`** — the `tau` dispatcher binary: `tau serve`, `tau tui`, `tau auth`, `tau config …`, etc. (The GUI is a separate double-clickable binary.)
- **`tau-tui`** — functional agent parity in ratatui: multiline input, OpenCode-faithful staged permissions/model picker/tool details, slash-command autocomplete, Tab cycling, and mouse/keyboard split hunk review with transaction-local undo/redo. No auto-spawn. (cite: 2026-07-17 10:57 r7, r15)
- **`tau-gui`** — separate gpui binary using a proven editor component when possible, complete typed cards/modals/pickers, diff review, operational sidebar, startup-state UI, and explicit child-daemon ownership. `gui.kdl` stores client-only preferences. (cite: 2026-07-17 10:57 r8, r15-r17)
- **`tau-discord`** (reference long-lived client, can land post-MVP) — an ordinary `tau-client` consumer forwarding to/from Discord, demonstrating the daemon model. No special server support.

Data flow: client → typed versioned WS JSON-RPC → server turn queue → tau-core Rig runner/hooks → permission + airtight + snapshot checks → tool mutation → initiating-client diff review → accepted partial result → model continuation. Every durable transition receives a per-session sequence; bounded output is journaled with immutable artifact references. Reconnect replays after the last sequence. (cite: 2026-07-17 10:57 r1-r3, r9-r10, r13, r16-r17)

Config/credentials: server and client configuration use KDL with an immediate break from TOML; credentials retain their protected keyring/file fallback and env precedence. Server config is global-only at MVP; GUI/TUI preferences use separate KDL files; local skill trust is per-skill. (cite: 2026-07-17 10:57 r2, r10, r14, r17)

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

- [ ] **M2 — Persistence layer (SQLite via rusqlite) + sessions (cwd-anchored) + messages/usage/Q&A schema**
  - [x] Airtight? **Yes** (tightened 2026-07-16 12:55)
  - **Library**: `rusqlite` (with `"bundled"` feature for libsqlite3-sys) + `rusqlite_migration` for schema versioning. Sync API; all DB calls in the daemon wrapped in `tokio::task::spawn_blocking`. Single connection in `Arc<Mutex<Connection>>`. (cite: 2026-07-16 12:45 r1)
  - **DB location**: `dirs::data_dir().join("tau").join("tau.db")` (e.g. `~/.local/share/tau/tau.db`). WAL journal mode (`PRAGMA journal_mode=WAL`). New `tau-core` fn `default_db_path() -> Result<PathBuf>` (mirrors `default_socket_path()`).
  - **Schema (4 conceptual entities → 5 physical tables)**: (cite: 2026-07-16 12:45 r1 + 12:50 r2)
    - `sessions`: `id TEXT PRIMARY KEY` (UUID v4), `cwd TEXT NOT NULL`, `title TEXT` (nullable, auto-gen deferred), `created_at INTEGER NOT NULL` (unix-epoch ms), `updated_at INTEGER NOT NULL`.
    - `messages`: `id INTEGER PRIMARY KEY AUTOINCREMENT`, `session_id TEXT NOT NULL REFERENCES sessions(id)`, `role TEXT NOT NULL CHECK(role IN ('user','assistant','system'))`, `seq INTEGER NOT NULL`, `created_at INTEGER NOT NULL`. `UNIQUE(session_id, seq)`.
    - `content_blocks` (child of messages — structured content): `id INTEGER PRIMARY KEY AUTOINCREMENT`, `message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE`, `seq INTEGER NOT NULL`, `block_type TEXT NOT NULL CHECK(block_type IN ('text','tool_use','tool_result'))`, `text TEXT`, `tool_call_id TEXT`, `tool_name TEXT`, `tool_args_json TEXT`, `tool_result_json TEXT`, `tool_is_error INTEGER`. `UNIQUE(message_id, seq)`.
    - `usage`: `id INTEGER PRIMARY KEY AUTOINCREMENT`, `session_id TEXT NOT NULL REFERENCES sessions(id)`, `message_id INTEGER REFERENCES messages(id)`, `model TEXT NOT NULL`, `input_tokens INTEGER NOT NULL`, `output_tokens INTEGER NOT NULL`, `cached_tokens INTEGER`, `created_at INTEGER NOT NULL`.
    - `qa_records`: `id TEXT PRIMARY KEY` (UUID v4), `session_id TEXT NOT NULL REFERENCES sessions(id)`, `question TEXT NOT NULL`, `answer TEXT NOT NULL`, `created_at INTEGER NOT NULL`.
    - Deferred to their milestones: plan store → M10, snapshots/revert → M11, permissions-saved → M7. Each adds its own migration.
  - **Domain types in `tau-core/src/db.rs`**: `Session { id: String, cwd: String, title: Option<String>, created_at: i64, updated_at: i64 }`; `Message { id: i64, session_id: String, role: String, seq: i64, created_at: i64, blocks: Vec<ContentBlock> }`; `ContentBlock` enum (`Text(String)` / `ToolUse { call_id, name, args_json }` / `ToolResult { call_id, result_json, is_error }`); `Usage { ... }`; `QaRecord { ... }`. All timestamps are `i64` (unix-epoch ms via `std::time` — no chrono).
  - **Repository** (`tau-core/src/db.rs`): `Db { conn: Arc<Mutex<Connection>> }`. Methods: `Db::open(path) -> Result<Db>` (open + migrate), `Db::open_in_memory() -> Result<Db>` (tests), `create_session(cwd)`, `list_sessions()`, `get_session(id)`, `append_message(session_id, role, blocks)`, `get_messages(session_id)` (joins content_blocks), `record_usage(...)`, `record_qa(session_id, question, answer)`, `get_qa_records(session_id)`. Migrations: a single `Migrations::new(vec![M::up(SQL)])]` — future milestones append.
  - **Server wiring** (`tau-server/src/lib.rs`): `AppState` gains `db: Arc<Db>` field; `run()` calls `Db::open(default_db_path()?)` before constructing `AppState`; `AppState::default()` uses `Db::open_in_memory()` for tests. DB accessor `state.db()`. (cite: 2026-07-16 12:50 r2 — wire into server now)
  - **CLI**: `tau resume` added as `bail!("not implemented")` stub alongside tui/gui/config. Actual implementation in M3+ when session JSON-RPC methods exist. (cite: 2026-07-16 12:50 r2 — stub in M2)
  - **Crates added**: `rusqlite` (`"bundled"`), `rusqlite_migration`, `uuid` (v4 features) into `tau-core`; all three to workspace `[workspace.dependencies]`.
  - **Tests**: migration idempotency (open twice, no error); session create → list → get round-trip; session records cwd; message append with text block → get_messages returns blocks; usage record round-trip; Q&A record round-trip; all via `Db::open_in_memory()`. M0 round-trip + M1 tests still pass (`AppState::default()` now uses in-memory Db).
  - **Behavior preservation**: persistence layer is additive — no protocol methods added. Server opens DB at startup but no JSON-RPC handler touches it yet (that arrives in M3+).

- [x] **M3 — rig integration: Provider enum + `completion.stream` JSON-RPC + streaming + session persistence** — IMPLEMENTED (commits `2d96112`, `26f80f7`, `f344f9a`)
  - [x] Airtight? **Yes** (tightened 2026-07-16 13:25; provider and session-contract repairs 2026-07-16 23:21)
  - `rig-core = "0.40"` with `test-utils`; `tau-core::provider::Provider` has concrete variants for all 23 completion-capable rig providers in this release. Voyage AI is excluded because it is embeddings-only. OpenAI uses the Responses model; Azure requires `ProviderConfig.api_base`; Llamafile supports no-key construction; all other key-bearing providers resolve credentials through the expanded credentials enum.
  - Provider streaming normalizes rig `StreamedAssistantContent` into text and usage deltas. The client receives `completion.delta` notifications carrying `request_id`, then the matching final `completion.stream` response.
  - `CompletionStreamParams` uses a required `provider/model` string, prompt, optional `session_id`, and optional cwd. An omitted session id creates a new cwd-anchored session; an explicit id resumes that session. This preserves the later M2 decision that sessions are always new by default and resume is explicit (cite: 2026-07-16 12:45).
  - The server persists user message, assistant response, and usage. `tau-client` exposes a borrowed `CompletionStream` yielding typed delta and final events. No live provider is required for tests.
  - Tests cover mock streaming, construction of every provider without network access, DB persistence, and fake-daemon WebSocket streaming. Full workspace fmt, strict Clippy, and tests pass.
  - **Behavior preservation**: ping/health, config/credentials, DB, and existing WebSocket behavior remain passing.
- [x] **M4 — Internal typed tool registry + read/grep/glob/list + hashline read** — IMPLEMENTED (commits `f803adc`, `73cad0b`, `f0f0032`, `e10b591`)
   - [x] Airtight? **Yes** (tightened 2026-07-16 23:22)
   - **Boundary**: M4 is tau-core only. No new JSON-RPC methods, server routing, client capability group, or agent-loop integration. The registry is the internal foundation consumed by M6.
   - **Registry API** (`tau-core/src/tools/`): a typed `Tool` implementation contract, typed input/output/error structures, deterministic model-facing rendering, descriptors (`name`, description, input shape), and a registry that type-erases only at lookup/dispatch boundaries. Duplicate names and unknown tools are typed errors.
   - **Per-session path policy**: `AccessPolicy` canonicalizes and registers the client cwd as its initial root. Additional roots can be registered later by the permission flow. Existing and non-existing descendants are checked without allowing symlink/path escapes. A path outside registered roots returns a typed `ApprovalNeeded` error containing operation, requested path, and candidate root; M4 does not prompt or access the path. (cite: 2026-07-16 23:17 and 23:18)
   - **Hashline module** (`tau-core/src/tools/hashline/`): SHA-1 uppercase hashes; 3-character line/anchor hashes through 4,096 lines and 4-character hashes above; anchor input is `prev + U+241E + line + U+241E + next`; file REV is the first 8 characters of the SHA-1 of CRLF-normalized content. Parse and render refs using `#HL <line>#<hash>#<anchor>|...` plus `#HL REV:<rev>`. This matches the inspected fork at `/home/henry/.cache/repo_cache/github.com-quinntyx-opencode/packages/opencode/src/tool/hashline.ts`.
   - **File read**: accepts relative or absolute paths after policy resolution; supports 1-indexed offset and limit; defaults to 2,000 lines, 50 KiB rendered output, and 2,000 characters per displayed line. It returns typed path/type/content/truncation metadata plus deterministic model text matching the fork's `<path>`, `<type>`, `<content>`, and continuation-marker shape. Binary files return a typed binary result with bytes, MIME, and metadata; client/model attachment transport remains deferred to the binary-channel milestone. (cite: 2026-07-16 23:22)
   - **Directory read/list**: entries are sorted alphabetically and bounded to 2,000 entries. Directory rendering uses a virtual hashline document whose visible entry form is `<hashlineref>|<zero-padded-id>|<entry-name>/`; IDs are separate from line numbers, start at 1, and are assigned alphabetically with a minimum two-digit width. The virtual directory REV/hashlines are read-only in M4; directory mutation uses the M5 hashline edit contract. (cite: 2026-07-16 23:19 and 23:20)
   - **Grep**: regex pattern, optional relative/absolute path, optional include glob; 100-match default; hidden files included, `.git` excluded, deterministic path/line ordering, typed invalid-pattern errors, and typed matches with path, 1-indexed line, byte offset, text, and submatches. Observable behavior follows the inspected fork's `packages/opencode/src/tool/grep.ts` and `packages/core/src/ripgrep.ts`.
   - **Glob**: pattern plus optional path; 100-file default; hidden files excluded by default, `.git` excluded, normalized relative paths, deterministic sorting, and typed truncation metadata. Observable behavior follows the inspected fork's `packages/opencode/src/tool/glob.ts`.
   - **Rendering**: typed results are the internal source of truth; each tool also exposes deterministic model-facing text. The fork has no standalone model-facing `list` tool, so tau's `list` is the typed direct-directory primitive shared by directory read and rendering.
   - **Dependencies**: use Rust-native SHA-1, regex/glob matching, and filesystem traversal/ignore handling; do not require a system-installed `rg` binary. Match the fork's observable `--no-config`, hidden, `.git`, sorting, and limit semantics.
   - **Tests** (`tau-core/src/tools/tests.rs` and focused module tests): hashline vectors and CRLF/EOF behavior; file render/parse round trips; directory IDs, virtual REV, sorting, pagination, and truncation; path policy descendants, symlink escapes, approval-needed errors, and root registration; read binary/line/byte limits; grep matches/invalid regex/include/hidden behavior; glob/list sorting and limits; registry duplicate/unknown dispatch; typed output equals rendered output expectations. Use `tempfile` fixtures and existing `cargo test --workspace --all-targets --all-features`.
   - **Behavior preservation**: no existing JSON-RPC method changes, no provider/session behavior changes, no writes or mutations, and all existing M0–M3 tests remain passing. `edit`, `write`, bash, snapshots, and directory mutation remain M5 responsibilities. (cite: 2026-07-16 23:20)

- [ ] **M5 — Hashline edit + write + bash (with sed-class guard)**
   - [x] Airtight? **Yes** (tightened 2026-07-17 04:11; implementation not started)
   - **Boundary**: M5 remains tau-core-only. No JSON-RPC methods, server/client tool exposure, interactive prompting, saved permission rules, or M8 airtight-plan enforcement. M5 executes bash and reports classification because that behavior is explicitly authorized for this milestone (cite: 2026-07-17 00:46).
   - **Edit API** (`tau-core/src/tools/edit.rs`): `EditInput` accepts `path`, optional `file_rev`, single `ref`, `start_ref`/`end_ref`, or ordered `operations[]`; each operation supports replacement content and an explicit operation kind. Validate current REV when supplied, line hashes, anchors, bounds, and non-overlap. Reject stale/mismatched references with typed errors that require rereading. Strip embedded `#HL REV:` and line-reference prefixes from replacement content. Preserve LF/CRLF, final-newline state, and BOM. Use a per-path mutation lock and compare source bytes again before atomic replacement. Exact hashline behavior is based on the inspected fork at `/home/henry/.cache/repo_cache/github.com-quinntyx-opencode/packages/opencode/src/tool/edit.ts` and the existing tau hashline primitives.
   - **Directory edit**: when `path` resolves to a directory, reread and recompute its virtual directory REV before every operation. Use numeric IDs/hashline refs for rename/delete; creation receives parent path, name, kind, and optional file content. Create files/directories, rename entries, and delete only empty directories or files. Reject non-empty directory deletion, path escapes, missing IDs, stale REV, and collisions. Directory mutation is ID-addressed rather than text-diff-derived (cite: 2026-07-17 00:46).
   - **Atomic write** (`tau-core/src/tools/write.rs`): `WriteInput { path, content }`; resolve through `AccessPolicy`, create parents only inside an approved root, preserve existing permissions where possible, write to a sibling temporary file, flush/sync, then rename into place. Existing content is replaced only after the complete write succeeds; failed writes leave the original unchanged (cite: 2026-07-17 00:46).
   - **Mutation helper** (`tau-core/src/tools/mutation.rs`): shared path locks, source-byte compare-and-swap, temporary-file naming/cleanup, BOM/EOL preservation, and typed mutation errors. `edit` and `write` must share this helper rather than implementing independent atomicity.
   - **Bash** (`tau-core/src/tools/bash.rs`): `BashInput { command, workdir, timeout }`; resolve workdir through `AccessPolicy`, execute `/bin/sh -c` on Unix and the platform shell on Windows, default timeout 120 seconds, cap returned output at 2,000 lines and 50 KiB using head/tail truncation metadata. Return exit code, stdout/stderr, timeout state, truncation state, and a command classification. Classify at minimum filesystem-mutating commands and sed/perl/python-style likely mutators; unknown/dynamic commands are conservatively marked `PotentialMutation`. Execute and classify, but do not enforce permission or airtight gates yet (cite: 2026-07-17 00:46).
   - **Snapshots** (`tau-core/src/tools/snapshot.rs`): capture-only, content-addressed pre-mutation store under the tau cache/data directory. Capture target files/directories before direct edits/writes; capture the approved cwd tree before bash, excluding `.git` and skipping files above the fork-compatible 2 MiB per-file bound. Store a manifest plus content blobs keyed by digest. Return a snapshot ID and metadata; no restore, listing, persistence in DB, or protocol API until M11 (cite: 2026-07-17 00:46).
   - **Registry** (`tau-core/src/tools/registry.rs`): register `EditTool`, `WriteTool`, and `BashTool` after existing read/list/glob/grep builtins. Preserve the current typed implementation contract and JSON-erased dispatch boundary.
   - **Existing M4 files**: update `tools/mod.rs` exports, `types.rs` outputs, `error.rs` typed mutation/snapshot/command errors, `hashline/mod.rs` line/stringification helpers, `read.rs` directory descriptor to remove the “read-only until M5” wording, and `tools/tests.rs` contract coverage. Do not change M4 hashes, REV calculation, limits, IDs, sorting, or access-policy behavior.
   - **Tests**: single-line/range/batched edits; stale REV/hash/anchor; prefix stripping; overlap rejection; CRLF/final-newline/BOM preservation; atomic-write failure safety; directory create/rename/delete-empty/non-empty rejection; bash stdout/stderr/status/timeout/truncation/classification; snapshot capture-before-mutation and content-addressed manifests; registry descriptors and M4 regression suite. Use existing tempfile tests and `cargo test --workspace --all-targets --all-features`.
   - **Behavior preservation**: M0–M4 protocol, provider, session, DB, and read/search behavior remains unchanged. M5 adds internal mutations only; user-visible rollback, permission enforcement, plan gates, and tool protocol exposure remain later milestones. (cite: 2026-07-17 00:46 and 04:11)

- [ ] **M6 — Rig agent runner, typed turn/event protocol, question, and tiered task** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **Protocol replacement** (`tau-proto`, `tau-client`, `tau-server`): immediately replace `completion.stream` with version/capability negotiation and typed session-turn start/cancel/replay methods. One active turn per session; additional submissions are persisted FIFO by server sequence. Mutating requests require client idempotency keys. Any attached client may cancel. (cite: 2026-07-17 10:57 r9, r16)
   - **Durable event journal**: add forward SQLite migrations for monotonically sequenced per-session state transitions. Persist bounded output plus immutable artifact references; high-frequency token/tool deltas may be coalesced. Reconnect resumes after a client-supplied sequence. `tau-client::events()` becomes a real multiplexed stream. (cite: 2026-07-17 10:57 r9, r16-r17)
   - **Rig runtime** (`tau-core/src/agent/`): replace the custom provider-stream loop with Rig's public `AgentRunner` builder and `AgentHook`. Preserve `RunId → turn → internal_call_id → provider tool ID / optional call_id`; `internal_call_id` is authoritative. Copy borrowed hook data into owned tau events. Pin each turn's advertised tool registry. (cite: 2026-07-17 10:57 r1, r10)
   - **Dynamic tools**: adapt the typed registry to Rig `DynamicTool`/`ToolSet` with complete JSON Schema—not `{type: object}` placeholders—and route every builtin through one hook pipeline. Read-only calls may run concurrently; mutations serialize per path/session; committed result order follows model call order. (cite: 2026-07-17 10:57 r1)
   - **Tool lifecycle**: typed requested → awaiting-input/approval → running → bounded output deltas → completed/failed/cancelled events, stable IDs, persisted tool-use/result blocks, cancellation propagation, and pre-mutation snapshot interception. Never encode semantics in display strings. (cite: 2026-07-17 10:57 r1, r9-r10)
   - **Question tool**: suspend without timeout while the daemon lives; any attached client may answer; persist one stable Q&A record per item under a shared request ID. Restart marks pending questions and other live work interrupted, matching inspected OpenCode behavior. (cite: 2026-07-17 10:57 r1-r2, r10)
   - **Task tool**: isolated nested Rig runs with inherited roots/permissions, shared read-only plan view, cancellation/resource accounting, and per-session tier: Unlimited, Max(3/8), xhigh(2/8), high(2/4), medium(1/4 default), low(1/1). Unlimited removes only tau depth/concurrency caps. (cite: 2026-07-17 10:57 r1, r10)
   - **Tests**: mock Rig multi-turn hook order/IDs/concurrency, dynamic schemas, question multi-client/restart behavior, tier matrices, cancellation, FIFO/idempotency, event replay, interrupted restart, and real server/client typed streaming. Update all old `completion.stream` tests in the same change.

- [ ] **M7 — Full ordered permission rules and blocking approval lifecycle** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **KDL breaking migration**: replace all tau TOML configuration with KDL immediately; do not read legacy `config.toml`. Server config and separate GUI/TUI preference files use KDL; the protected credential fallback remains separate. Add validator diagnostics with source spans. (cite: 2026-07-17 10:57 r2, r10, r17)
   - **Rules**: deterministic canonical subject over tool + normalized args; first ordered match wins. Scopes are global KDL and session SQLite. Defaults: allow approved-root reads/search and network access, ask for mutation/process/outside-root access, while network remains classified and explicitly rejectable. (cite: 2026-07-17 10:57 r2, r10)
   - **Decisions/UI contract**: Allow once, Allow for session, Always allow matching rule, Reject once, Always reject matching rule. Global durable decisions update KDL atomically; session decisions update SQLite. (cite: 2026-07-17 10:57 r2)
   - **Lifecycle**: initiating client owns permission requests; explicit takeover is allowed. Requests never time out while the daemon lives. Any client may technically send a reply, but server validates ownership for permission decisions. Restart marks pending requests interrupted. Clients choose how to render or automate replies. (cite: 2026-07-17 10:57 r2, r10)
   - **Hard denials**: tau credential/runtime internals and unresolved root/path escapes cannot be made saveable; autonomous super mode cannot override them. All other decisions flow through the same pipeline for builtin, MCP, LSP, skill, and Git operations. (cite: 2026-07-17 10:57 r2, r6, r16)
   - **Tests**: canonicalization/property matrix, ordering/scopes/defaults, atomic KDL writes and syntax diagnostics, SQLite replay, five reply modes, indefinite waits with cancellation/restart, ownership/takeover, hard denials, network default, and upgrade migration from v1.

- [ ] **M8 — Revisioned plan tool, Q&A resolution, airtight gate, and steering-run state** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **Persistence/API**: forward migrations for one revisioned plan per session: Architecture Contract, Behavior Invariants, Architecture Overview, ordered steps/items, current step, airtight flags, and Q&A IDs. `plan` supports read, revision-checked update, set-current, mark-item, request-airtight, grant/revoke-airtight, and render. Stale updates fail with current revision. (cite: 2026-07-17 10:57 r3)
   - **Airtight authority**: only a human client or active steering agent may grant airtight status. Material edits to contract/invariants/scope/items revoke it; cosmetic edits do not. Q&A IDs resolve server-side to quoted persisted records in model rendering. (cite: 2026-07-17 10:57 r3)
   - **Gate**: without a current airtight step, only planning/read/question/skill-context operations run. Every classified builtin/MCP/LSP/bash mutation is rejected before execution. Plan/build prompts and tool availability reinforce but never replace the harness gate. (cite: 2026-07-17 10:57 r3)
   - **Autonomous state**: named steering agent + vision + goal + run status are persisted and broadcast. Normal steering answers questions and grants airtight status at every completed step; super mode may answer all soft prompts but lasts one run and requires risk acceptance. Warning suppression may persist separately. Steering declares goal completion and emits a final report. (cite: 2026-07-17 10:57 r3, r11, r16)
   - **Tests**: revision conflicts, material/cosmetic revocation, human/steering authority, resolved Q&A rendering, mutation classification matrix, no-plan behavior, normal/super authority, hard-denial preservation, per-step steering review, goal completion, and restart state.

- [ ] **M9 — Persisted context epochs, artifact-aware compaction, and plan reinjection** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **Agent contract**: every primary agent references a valid named non-primary compaction agent. Builtin plan/build defaults use the chosen primary model through default compaction agents. Unknown context limits warn; smaller compaction windows are allowed with an early-compaction warning. (cite: 2026-07-17 10:57 r4, r11, r16)
   - **Epoch store**: forward migrations persist append-only epochs, parent links, baselines, mid-epoch system messages, summaries, estimated token state, provider-reported usage, and terminal status. Full active state survives compaction verbatim. (cite: 2026-07-17 10:57 r4)
   - **Trigger/retry**: compact at 80% of the configured compaction model's known window; otherwise use the full primary window and react to provider overflow. An overflow compacts and retries once with the same run/turn identity and a recorded retry event. Clients and agents may request manual compaction. (cite: 2026-07-17 10:57 r4, r11)
   - **Artifacts**: full bounded/unbounded tool output is stored immutably with hashline rendering. Compactor returns validated important hashline ranges plus summary; selected lines, snapshot/artifact IDs, active plan with resolved Q&A, agents, permissions summary, recent messages, unresolved prompts, and active tool state enter the new baseline. Dynamic output never recursively executes. (cite: 2026-07-17 10:57 r4, r11, r13)
   - **Failure**: failed compaction leaves the old epoch untouched, emits failure, and only blocks a new model request when capacity cannot be made safe.
   - **Tests**: threshold/capacity matrix, smaller-compactor warning, provider overflow retry-once, manual compaction, epoch persistence/restart, full plan/Q&A reinjection, hashline range validation, immutable artifact retrieval, failure atomicity, and estimated/reported usage events.

- [ ] **M10 — Configurable plan/build agents, switching, grouping roots, and Claude-compatible skills** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **Agent KDL**: name, description, primary/cycle flags, model, prompt, tools, permission overrides, mode, and required compaction-agent reference. Ship overridable `plan` and `build`; custom non-cycle agents remain selectable. Validator rejects missing/invalid references. (cite: 2026-07-17 10:57 r4-r5, r16)
   - **Switching/commands**: typed server method persists selected agent and queues an in-flight change for the next turn. Tab cycles configured cycle members. `/agent <name>` provides argument autocomplete; `/agents` and GUI controls open OpenCode-faithful search. Skill slash commands share the typed autocomplete registry. (cite: 2026-07-17 10:57 r5, r8, r15)
   - **Minimal grouping**: detect direct source cwd versus grouping cwd. For grouping, immediate child directories become approved read roots while the parent remains grouping-only; no picker or project config cascade. Initialize visible tau metadata Git state as specified for M11. (cite: 2026-07-17 10:57 r14, r17)
   - **Skills**: discover global `~/.config/tau/skills` and cwd `.tau/skills`, local overrides names, local trust is persisted per skill, global skills are trusted. Implement Claude Agent Skills-compatible `SKILL.md` YAML frontmatter, scripts, references/resources, assets, progressive metadata→body→resource disclosure, auto-selection, and slash invocation. (cite: 2026-07-17 10:57 r5, r11-r14)
   - **Dynamic context**: exact Claude Code `!\`command\`` preprocessing before model consumption; one-pass stdout substitution, `$ARGUMENTS`/positional expansion, bash limits, structured failure insertion, and read-only command classification. Trusted global or trusted local skills skip permission prompts, but mutating/unknown dynamic commands are rejected. Skill load/execution is a persisted typed event. (cite: 2026-07-17 10:57 r12-r14)
   - **Tests**: KDL agent validation/default override, next-turn switching/replay/commands/autocomplete, cycle filtering, grouping/direct-root detection, skill precedence/trust, frontmatter/progressive loading/resources/scripts, dynamic expansion/arguments/failure/limits/no recursion/read-only enforcement, and skill event persistence.

- [ ] **M11 — rmcp manager, Git/worktree-backed revert, and full LSP manager** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **MCP**: use `rmcp`; daemon-owned bounded-restart manager, one process per enabled KDL entry (command, args, explicit env, startup timeout, permissions). Expose discovered tools through the identical DynamicTool/policy/event pipeline and prompts as typed searchable commands. Non-stdio transports and server exposure remain out of scope. (cite: 2026-07-17 10:57 r6)
   - **Snapshot/diff transaction**: capture before mutation, apply mutation, then suspend before returning the result to the model. Initiating client owns review with takeover. Text uses hunk decisions; binary/rename/delete use whole-file decisions. Rejected hunks restore partially; accepted partial state becomes the tool result. Review supports transaction-local undo/redo. (cite: 2026-07-17 10:57 r13, r15, r17)
   - **Git topology**: grouping cwd gets a tau-metadata repository; each child source repository receives accepted source commits. Direct non-Git roots initialize visibly. Default managed mode creates model-named, sanitized, collision-safe branches and matching separate worktree folders without switching user worktrees. Multi-repo boundaries commit each child plus a coordinating metadata manifest. (cite: 2026-07-17 10:57 r13-r14, r17)
   - **Commit/integrate**: explicit snapshot commit only; stage tau-touched paths, use normal user Git identity/hooks/signing, and record session/turn/snapshot metadata. Config may choose current-branch, managed-worktree, or metadata-only mode; managed is default. Integration into normal branches is an explicit previewed merge/cherry-pick action. External conflicts refuse unless explicitly forced. (cite: 2026-07-17 10:57 r6, r13-r14, r16-r17)
   - **LSP**: KDL language→command configuration; lazy one-server-per-language/root manager with bounded restart/status. Support diagnostics, hover, definition, references, symbols, formatting, rename, and workspace edits. Workspace edits use the same mutation/review pipeline. Diagnostics stream live; persist only those used in completed model context. (cite: 2026-07-17 10:57 r6)
   - **Tests**: rmcp stdio fixture/tool/prompt/restart/policy; snapshot list/preview/partial rollback/conflict/force/commit; direct/grouped/non-Git/multi-repo managed worktrees and hooks; explicit integration; LSP fixture for diagnostics/navigation/format/rename/workspace edit/restart/context persistence.

- [ ] **M12 — Functional-parity ratatui client** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - **Runtime/input**: never auto-spawn; exact daemon-down help. Multiline Unicode editor, Enter submit, Shift+Enter newline, cancellation, clipboard/history, reconnect/replay, real configured model selection, and full typed event reducer. (cite: 2026-07-17 10:57 r7)
   - **OpenCode-faithful UX**: staged permission modal over prompt expanded to five decisions, reject message, OpenCode-style favorites/recents/search model modal, collapsed tool name/result cards, click expansion, right-click/action raw JSON inspector, and slash autocomplete. `/agent <name>`, `/agents`, and Tab use server-backed switching. (cite: 2026-07-17 10:57 r7, r15)
   - **Diff review**: left/right split, mouse mode, arrow/Tab hunk navigation, Enter accept, Backspace reject, `u` undo, `U` redo, whole-file accept, and atomic binary/rename/delete choices. Only initiating client decides unless ownership transfers. (cite: 2026-07-17 10:57 r7, r15, r17)
   - **Autonomous UX**: tier dropdown/command, steering-agent/vision controls, normal autonomous status, one-run super confirmation, and persistent warning suppression in `tui.kdl`.
   - **Tests**: ratatui buffer snapshots for every card/modal/picker/diff state; scripted daemon flows for replay, stream, tools, indefinite permission/question, hunk review undo/redo, agent/model commands, autonomy, cancellation, and daemon-down exact text.

- [ ] **M13 — Separate primary gpui client and complete MVP acceptance** — AIRTIGHT (tightened 2026-07-17 10:57)
   - [x] Airtight? **Yes**
   - [ ] **Reopened implementation audit (2026-07-17)**: user report: "significant problems with the frontend, namely that it completely doesn't work and all features and hooking up stuff like commands, popups, etc. the gui is like 1% done if even that". M13 remains incomplete. Reducer-only tests, decorative controls, and local-only card decisions do not satisfy acceptance; tests must drive rendered GPUI listeners and typed daemon calls. The screenshot regression `rpc error -32601: unknown method: session.turn.start` requires startup protocol negotiation and a clear incompatible-daemon recovery state before any turn submission.
   - **Binary/editor**: `tau-gui` becomes a separate executable; `tau gui` execs it on Unix and spawns detached elsewhere. Prefer a maintained GPUI editor component; otherwise require full Unicode grapheme, selection, navigation, clipboard, multiline, IME, and focus tests before acceptance. (cite: 2026-07-17 10:57 r8, r15-r16)
   - **Typed UI**: reducer-driven cards for user, assistant, reasoning, complete tool lifecycle, question, permission, diff, error, compaction, and system/integration status. OpenCode-faithful permission modal appears over chat; tool card right-click opens arguments/output/metadata/raw JSON. No string-prefix semantic parsing. (cite: 2026-07-17 10:57 r8-r9, r15)
   - **Pickers/commands**: centered provider-labeled model picker with recents, favorites, fuzzy search, and per-session selection; agent search picker, GUI button, Tab cycle, `/agent <name>`, and `/agents`. Command arguments autocomplete. (cite: 2026-07-17 10:57 r8, r15)
   - **Diff/autonomy**: visual hunk and whole-file review with the same transaction/ownership/undo rules as TUI; subagent-tier dropdown; steering-agent/vision and normal/super controls with risk confirmation.
   - **Operational sidebar**: plan/current step/airtight state, LSP status/diagnostics, input/output/cached tokens, estimated context tokens/% and reported usage, model/agent, session ID, cwd/roots, MCP status, task tier, and autonomous state. Layout preferences persist in `gui.kdl`. (cite: 2026-07-17 10:57 r8, r17)
   - **Daemon startup/ownership**: detailed connecting/spawning/migrating/ready/degraded/failed/retry states. Auto-spawn warning actions are Okay, Quit, Disown, Always-disown, Never-show-again. Child-owned daemon stops gracefully then forcefully; existing/disowned daemon remains. Ownership and warning preferences are independent. (cite: 2026-07-17 10:57 r8, r15)
   - **Tests/acceptance**: event reducer and component tests, editor tests, client-preference and daemon-lifecycle subprocess tests, headless GPUI smoke, and scripted full E2E: negotiate → plan → Q&A citation → human/steering airtight → permission → mutation → snapshot → hunk decision → model result → explicit Git commit/integrate preview → replay → compaction with plan reinjection. M13 is incomplete until this passes. (cite: 2026-07-17 10:57 r8-r9)

## M6-M13 Implementation Workstreams

The airtight milestones are implemented in dependency-aware waves. Every worker receives the relevant verbatim Q&A/architecture contract in its untracked worktree-local `ROADMAP.md` before implementation begins. (cite: 2026-07-17 10:57)

1. **Protocol/storage** — `tau-proto`, `tau-client`, event journal, idempotency, replay, and forward migrations.
2. **Server runtime** — negotiated sockets, connection registry, FIFO turn queue, cancellation, indefinite input waits, and event broadcast/replay.
3. **Core Rig runner** — real Rig runner/hooks, DynamicTool schemas, lifecycle events, question, and tiered task.
4. **Policy/planning** — KDL migration, permission rules/lifecycle, revisioned plan/Q&A, airtight gate, and steering-run state.
5. **Context/agents/skills** — append-only epochs, compaction, plan/build agents, switching, grouping roots, Claude-compatible skills, and dynamic context.
6. **Integrations/Git** — rmcp, LSP, diff/snapshot transactions, Git commits, managed worktrees, and explicit integration.
7. **TUI** — typed reducer, multiline input, OpenCode-faithful modals/cards/pickers, commands, and hunk review.
8. **GUI** — separate binary, typed reducer/components, editor, modals/pickers/sidebar, diff/autonomy UX, and daemon ownership.

Wave 1 produces compile-tested candidate branches for these boundaries. After review and foundation integration, Wave 2 uses another parallel worker set for cross-crate wiring, client parity, fixtures, and the mandatory M13 end-to-end acceptance flow.

- [ ] **M14 — Hardening: systemd unit, docs, packaging, end-to-end**
   - [ ] Airtight? No
   - First-class systemd unit + install docs; `tau serve` single-instance + socket activation; packaging (.deb/.rpm/.dmg/.msi as feasible); end-to-end happy-path test across daemon+GUI.

(Out of scope for MVP, tracked separately: **project system (multi-root projects, project registry, `.tau/` config cascade, directory/project pickers)**, plugin system, session sharing, enterprise/SSO, cloud identity, MCP server-exposure, web client, non-stdio MCP transports, `tau-discord` reference bridge.)

## Q&A Session: 2026-07-17 UI redesign round 1

Q: How much project functionality should the new left sidebar include now?
A: "Full projects (Recommended)"

Q: How should the left sidebar organize projects and chats?
A: "Nested chats (Recommended)"

Q: I recommend compact composer chips for agent, model/provider, and effort, plus a separate Run Mode menu for task tier and autonomy. Should we use that split?
A: "Split controls (Recommended)"

Q: What should the effort selector control?
A: "Provider-aware reasoning (Recommended)"

Q: Where should the Antigravity-style Review Changes button open the OpenCode-style left/right diff?
A: "Center replacement (Recommended)"

Q: When should the operational sidebar appear?
A: "Always visible"

Q: How interactive should the custom phase/step todo be?
A: "Inspect and navigate (Recommended)"

Q: What should be the visual foundation?
A: "Light first"

## Q&A Session: 2026-07-17 UI redesign round 2

Q: What should appear below the Claude-style landing composer now that projects/chats live in the left sidebar?
A: "Whitespace + hints (Recommended)"

Q: What is a project in the new full project system?
A: "Workspace group"

Q: Which project-sidebar actions should be in the first design?
A: "New/open/edit/archive (Recommended)"

Q: How should a new conversation get its title?
A: "Generated then editable (Recommended)"

Q: Which attachments/tools should the landing and conversation composer expose?
A: "Context plus files (Recommended)"

Q: How should the compact agent/model/provider/effort cluster open?
A: "Separate chips"

Q: What should the Run Mode menu contain besides task tier and autonomy?
A: "Tier + autonomy + steering (Recommended)"

Q: What should the center diff review layout include?
A: "File rail + split editor (Recommended)"

Q: Which sections should be visible by default in the always-visible right sidebar?
A: "Contextual stack (Recommended)"

Q: Approve this top bar action set: sidebar toggle, `project / conversation` breadcrumb, connection indicator, New conversation, command palette/search, terminal, Review Changes, right-sidebar toggle, and settings.
A: "Approve action set (Recommended)"

Q: How should the shell behave when the native window becomes narrow?
A: "Progressive collapse (Recommended)"

Q: Should the comprehensive redesign be implemented as staged vertical slices?
A: "Vertical slices (Recommended)"
