# GUI Runtime/Negotiation Contract
Untracked; never commit. Full binding contract: `/home/henry/Documents/src/tau/main/ROADMAP.md`, especially M13 and Q&A r8/r9/r15-r17.

User report: "significant problems with the frontend, namely that it completely doesn't work and all features and hooking up stuff like commands, popups, etc. the gui is like 1% done if even that".

Scope: fix the real backend/client lifecycle. Negotiate protocol/version/capabilities before ready or turn submission; distinguish absent, spawning, connecting, negotiating, incompatible, degraded, ready, and failed. A reachable stale daemon must never reach `session.turn.start`; show actionable restart/retry/disown/quit UX without killing an existing daemon silently. Maintain persistent event/policy subscriptions, reconnect/replay, cancel, model/agent/autonomy/tier options, and owned-child semantics. Add live socket/subprocess tests.
