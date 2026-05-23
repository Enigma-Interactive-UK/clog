# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

**Clog** (Core Log) - a Windows desktop application for viewing, tailing, searching and filtering log4j2-formatted log files produced by Play 1.x Java applications.

The project is in its earliest stage: no source code, no build system, and no tech stack chosen yet. The first substantive task is to agree on the stack before scaffolding. Do not invent build, test, or run commands - none exist yet. Update this file once they do.

## Log format Clog must parse

Sample logs and the originating log4j2 configs live in [research/](research/). The pattern in production use is:

```
[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n
```

Concretely each line looks like:

```
[INFO ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play/sites/solopress
```

Notes that matter for the parser:
- Level is left-padded to 5 chars inside brackets (`INFO `, `WARN `, `ERROR`, `DEBUG`, `TRACE`).
- Thread name is bracketed and may contain spaces or punctuation.
- Logger name (`%c{1}`) is the short class name, typically `play` for framework lines but anything for app lines.
- `%msg` may contain newlines (stack traces) - lines that do not match the leading `[LEVEL]` pattern belong to the previous record.
- Play 1.x rolls files via log4j2's `RollingFile` with an `OnStartupTriggeringPolicy`, so tailing must survive file rotation/truncation. See [research/log4j2.wsl-oink.xml](research/log4j2.wsl-oink.xml) for a representative appender setup (main + per-level info/warn/error files).
- A real ~8.7MB sample log lives at [research/solopress.out](research/solopress.out) - use it for parser and tailing tests rather than synthetic fixtures.

## OpenWolf

This repo is managed by OpenWolf. The protocol in [.wolf/OPENWOLF.md](.wolf/OPENWOLF.md) is binding for every session:

- Consult [.wolf/anatomy.md](.wolf/anatomy.md) before reading files; update it when files are added/renamed/removed.
- Consult [.wolf/cerebrum.md](.wolf/cerebrum.md) before generating code; record preferences, learnings, do-not-repeats and decisions there as they emerge.
- Log bugs to `.wolf/buglog.json` per the protocol (low threshold - when in doubt, log).
- Append a one-line memory entry to `.wolf/memory.md` after significant actions.
