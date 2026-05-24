---
description: OpenWolf protocol enforcement — active on all files
globs: **/*
---

- Check .wolf/anatomy.md before reading any project file
- Check .wolf/cerebrum.md Do-Not-Repeat list before generating code
- NEVER hand-edit .wolf/anatomy.md. OpenWolf's post-write hook maintains it automatically; the hook's parseAnatomy regex demands a strict `- \`file\` — description (~N tok)` format (em-dash with spaces, parenthesised token estimate). Any non-conforming entry is silently dropped the next time the hook fires, which collapses the whole file. If anatomy.md looks stale or wrong, run `openwolf scan` (or `openwolf scan --check` to inspect without writing) rather than editing it.
- After writing or editing files, append a one-line entry to .wolf/memory.md
- After receiving a user correction, update .wolf/cerebrum.md immediately (Preferences, Learnings, or Do-Not-Repeat)
- LEARN from every interaction: if you discover a convention, user preference, or project pattern, add it to .wolf/cerebrum.md. Low threshold — when in doubt, log it.
- BEFORE fixing any bug or error: read .wolf/buglog.json for known fixes
- AFTER fixing any bug, error, failed test, failed build, or user-reported problem: ALWAYS log to .wolf/buglog.json with error_message, root_cause, fix, and tags
- If you edit a file more than twice in a session, that likely indicates a bug — log it to .wolf/buglog.json
- When the user asks to check/evaluate UI design: run `openwolf designqc` to capture screenshots, then read them from .wolf/designqc-captures/
- When the user asks to change/pick/migrate UI framework: read .wolf/reframe-frameworks.md, ask decision questions, recommend a framework, then execute with the framework's prompt
