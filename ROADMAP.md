# GUI Editor/Layout Contract
Untracked; never commit. Full contract: `/home/henry/Documents/src/tau/main/ROADMAP.md`, M13 editor/binary requirements and Q&A r8.

Scope: connect the tested Unicode `EditorBuffer` to the rendered GPUI editor. Support grapheme cursor movement, selection, clipboard, multiline, Enter submit vs Shift+Enter newline, history, focus, paste, IME where GPUI exposes it, and disabled/streaming states. Build stable layout: transcript fills window, footer remains visible, modals overlay chat bar/center, sidebar toggles/resizes, tool/diff content scrolls. Ensure every rendered button has hover/click/focus semantics. Add GPUI/headless component tests where supported and pure editor tests otherwise.
