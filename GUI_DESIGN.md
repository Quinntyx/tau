# Tau GUI Design

## Direction

Tau combines Claude Desktop's calm landing composer, Arena's project rail, OpenCode's information density and review workflow, Z.ai's structured coding transcript, and Antigravity's project/conversation chrome. The result is a light-first, keyboard-friendly coding workspace rather than a dashboard or a decorative mockup.

The GUI is a typed client. A visible control is not complete until it dispatches a typed client/backend action, renders pending and failure states, and has a component or scripted acceptance test. No semantic state may be inferred by parsing display text.

## Non-goals

- No TPS meter. Token usage and context accounting are useful; generation speed is not a product signal for Tau.
- No Recent Chats landing cards. Projects and nested conversations own navigation.
- No Arena battle/comparison mode. The reusable idea is a concise mode selector, adapted to Tau's run controls.
- No decorative buttons, dead picker helpers, or local-only approval decisions.

## Shell

The window has three regions plus a persistent top bar:

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│  [rail]  project / conversation                         [connection] [actions] │
├───────────────┬──────────────────────────────────────────┬─────────────────────┤
│ Projects      │ Transcript / landing / diff review       │ Operational context │
│               │                                           │                     │
│               │                                           │                     │
└───────────────┴──────────────────────────────────────────┴─────────────────────┘
```

### Top bar

Left to right:

- Sidebar toggle.
- Breadcrumb: `project / conversation_name`; each segment opens its own typed picker/action.
- Connection and daemon health indicator; clicking opens the detailed startup/compatibility state.
- New conversation.
- Command palette/search.
- Terminal action.
- Review Changes.
- Right-sidebar toggle.
- Settings.

Actions that are unavailable in the current state are hidden or disabled with an explanation. They are never rendered as inert icons.

### Left project rail

Projects are workspace groups: a named grouping directory with equal child source roots/worktrees and nested conversations. The rail supports:

- New project.
- Open project.
- Edit project name and roots.
- Archive project.
- New conversation within a project.
- Expand/collapse project conversations.
- Select, rename, archive, and search conversations.

New conversations receive a generated title after the first prompt and can be renamed inline from the rail or breadcrumb. Project and conversation mutations are server-backed and persisted; optimistic rows remain visibly pending until acknowledgement.

### Landing canvas

The landing state is intentionally spacious. It contains a centered greeting and the Claude-style message bubble, with no recent-chat card grid. Optional keyboard hints may sit below it. The composer supports multiline text, context attachments, files, slash commands, and the controls specified below.

### Conversation canvas

The conversation state uses a scrollable transcript with structured cards:

- User and assistant messages.
- Reasoning and progress, collapsed by default.
- Tool name/result summary cards with expansion and right-click inspection.
- Permission, question, plan, airtight, diff, compaction, error, and integration cards.
- Streaming and cancellation states.

The composer stays anchored at the bottom. Modals for permissions, questions, plan/airtight actions, and selectors appear above the composer without changing the underlying transcript scroll position.

### Right operational rail

The rail is visible by default in both landing and conversation states, but landing values must be explicit unavailable/loading rather than fake live data. It is a contextual stack, not an always-expanded wall of statistics:

- Plan and custom todo: current step, subitems, airtight state, phase/status, and navigation links to related transcript events.
- Session: title, session ID, cwd/source roots, active agent, model/provider, and connection state.
- Usage: input/output/cached tokens, estimated context tokens and percentage, provider-reported usage, and compaction status. No TPS.
- Integrations: LSP diagnostics/status, MCP servers/tools, Git/snapshot/change status.
- Run: task tier, autonomy mode, steering agent/vision, and risk status.

Sections collapse automatically when their data is unavailable or low priority. Every value has a typed source and an unavailable/loading/error representation.

## Composer Controls

The composer has a message area, a lower control row, and a submit/cancel affordance.

### Separate selector chips

Agent, model, provider, and effort are separate chips so each menu remains legible and individually keyboard accessible. The cluster replaces the single model label from Claude Desktop.

- Agent chip: `plan` or `build`, plus configured custom agents.
- Model chip: model name and provider identity.
- Provider chip: provider label and connection/configuration status.
- Effort chip: provider-aware `Auto`, `Low`, `Medium`, `High`, `Max`; unsupported values are disabled with an explanation.

Each chip opens a focused popover with search where useful, current selection, recent/favorite choices, and an explicit confirmation/acknowledgement state. Selection is server-backed when it affects the next turn and persisted at the correct session/config scope.

### Run Mode menu

A separate mode control contains:

- Task tier: Unlimited, Max, xhigh, high, medium, low.
- Normal autonomy state.
- Super-mode one-run confirmation and risk explanation.
- Steering agent.
- Steering vision/guidelines.
- Current steering/risk status.

The menu does not hide permission, plan, or diff behavior. It configures who may answer soft prompts; hard denials remain enforced.

### Other composer actions

- Attach files.
- Add workspace context.
- Slash command and argument autocomplete.
- Submit.
- Cancel while a turn is active.

## Review Changes

Review Changes replaces the center canvas while preserving both sidebars and the top bar. It contains:

- Changed-file rail with status, path, repository/root, and selection.
- Side-by-side old/new editor with synchronized scrolling.
- Hunk focus and keyboard navigation.
- Hunk accept/reject, whole-file accept/reject, undo, redo, and review summary.
- Binary, rename, and deletion whole-file decisions.
- Pending, ownership, conflict, and acknowledgement states.

The initiating client owns the transaction by default. Every decision uses the typed diff API, stable transaction/hunk identifiers, and idempotency keys. The UI changes only after the daemon acknowledges the decision.

## Visual System

Light mode is canonical for the first implementation: warm off-white canvas, white/elevated surfaces, graphite text, restrained gray borders, and one Tau accent for active/focus states. Dark mode uses the same semantic tokens, not a second ad-hoc palette.

- Typography: one highly legible UI family; larger, quieter display greeting; compact technical metadata.
- Shape: medium-radius composer and cards; small-radius controls; no excessive pills except status/chips.
- Density: generous landing whitespace, compact transcript metadata, readable code and diffs.
- Color: semantic tokens for accent, success, warning, danger, pending, muted, and surface elevation. Never use color as the only status signal.
- Motion: short opacity/position transitions for menus and cards; no continuous decorative animation; respect reduced-motion preferences.
- Accessibility: visible focus rings, keyboard navigation, minimum hit targets, tooltips for icon-only actions, accessible names, and contrast-compliant semantic colors.

## Vertical Slices

Each slice is independently usable and includes state, rendering, typed actions, persistence where required, and tests.

1. **Shell and projects**: top bar, light-first tokens, progressive layout, project rail, workspace-group CRUD, nested conversations, generated/editable titles.
2. **Landing and composer**: Claude-style landing, multiline composer, context/files, separate agent/model/provider/effort chips, run-mode menu, command palette wiring.
3. **Live conversation**: typed transcript cards, streaming/cancel/replay, tool inspection, permission/question/plan/airtight controls, daemon negotiation and lifecycle UX.
4. **Operational rail**: contextual plan/todo navigation, session/usage/context, LSP/MCP/Git status, autonomy/tier state, unavailable/loading/error states.
5. **Review workspace**: center-replacing file rail and side-by-side diff, hunk/whole-file actions, ownership, undo/redo, binary/rename/delete handling.
6. **Responsive and acceptance**: progressive collapse, focus/keyboard behavior, headless GPUI smoke, scripted daemon E2E, and full acceptance flow.

## Control Wiring Contract

Every button, chip, menu item, keyboard command, and context action must document:

| Control | Typed action | Scope | Success state | Failure state | Evidence |
| --- | --- | --- | --- | --- | --- |
| Project create/open/edit/archive | project RPC | global/project | row/state acknowledged | inline error + retry | component + E2E |
| Conversation new/select/rename/archive | session RPC | project/session | breadcrumb/rail updated | error without losing selection | component + E2E |
| Agent/model/provider/effort | config/session/turn RPC | session/next turn | chip updates after ack | pending/error/revert | component + wire test |
| Run Mode | tier/autonomy/steering RPC | session/run | menu status updates | risk/error state | component + E2E |
| Submit/cancel/replay | turn RPC | session/turn | typed lifecycle events | retry/interrupted state | scripted E2E |
| Permission/question/plan/airtight | policy/plan RPC | request/step | card resolves after ack | pending/retry/error | wire + E2E |
| Review Changes actions | diff/snapshot/Git RPC | transaction/hunk | review state acknowledged | conflict/ownership error | component + E2E |
| Sidebar and layout toggles | GUI preference action | client | persisted layout | local error fallback | preference test |

No control may mutate only a local card or label and claim completion.

## Acceptance Flow

The release gate runs a real scripted daemon/client flow:

`launch → negotiate → project create/open → conversation create → select agent/model/provider/effort → submit → stream → tool card inspect → permission/question → plan step and airtight → mutation → snapshot/diff review → hunk decision/undo → result → sidebar updates → replay → compaction/plan reinjection → cancel/reconnect → rename/archive`

The flow must verify typed RPC payloads, durable state, acknowledgement ordering, error recovery, keyboard paths, and rendered action seams. Visual snapshots supplement but never replace behavior assertions.
