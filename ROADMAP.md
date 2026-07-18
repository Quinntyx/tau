# GUI Acceptance Contract
Untracked; never commit. Full contract: `/home/henry/Documents/src/tau/main/ROADMAP.md`, M13 acceptance requirement.

Scope: create production-path tests that fail on decorative/unhooked GUI. Use a scripted typed WebSocket daemon/provider seam and actual Backend/TauView actions. Cover negotiate/incompatible stale daemon, submit/stream/complete, cancel, replay/reconnect, permission/question/diff reply, model/agent commands/pickers, sidebar state, daemon warning/ownership, and listener reachability. Prefer GPUI headless test APIs available in gpui 0.2; if platform rendering cannot run in CI, expose testable view-action methods and assert emitted typed requests plus state transitions. Do not duplicate production subsystems.
