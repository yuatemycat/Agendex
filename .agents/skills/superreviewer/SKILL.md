---
name: superreviewer
description: Structured code review checklist and auditing protocol. Activate this skill when reviewing pull requests, new features, benchmarks, or refactors across codebases, CI/CD pipelines, and documentation.
---

# Superreviewer — Code Review Skill

Use this checklist when reviewing changes to the Web Agent Proxy monorepo. Each section covers a distinct concern area. Flag any violations as Critical, High, Medium, or Low severity.

---

## 1. E2EE & Cryptographic Privacy

- Verify that AES-256-GCM encrypted payloads (`iv`, `tag`, `data`) remain opaque to the Bridge Server at all times.
- Confirm the Bridge Server never logs, inspects, or decrypts request URLs, headers, cookies, or response bodies when `encrypted: true`.
- Check that new fields added to encrypted payloads are included inside the ciphertext envelope, not sent in plaintext alongside it.
- Verify that encryption keys / passphrases are never committed, logged, or exposed in CI environment variables without necessity.

## 2. Monorepo & Build Integrity

- Run `npm run build` — all TypeScript workspace packages must compile without errors.
- Run `npm test` — all unit and integration test suites must pass (bridge-server, dashboard, mcp-server, sdk-js, simulator, scripts).
- Run `node scripts/prepare-release.js --check` — all package versions must be aligned.
- Verify that changes to shared types or interfaces in `sdk-js` do not break downstream consumers (`bridge-server`, `simulator`, `mcp-server`).

## 3. Documentation Rules (AGENTS.md Compliance)

- No ASCII box-drawing diagrams or Unicode box-drawing characters (`┌┬┐├┼┤└┴┘─│`) in `.md` files. Use plain Markdown headings, lists, and bold text instead.
- No complex character art in comments or documentation.
- All claims in documentation must be accurate and verifiable — do not overstate capabilities.

## 4. SDK Adapter Consistency

When reviewing changes to framework adapters (`langchain.py`, `llamaindex.py`, `autogen.py`, `crewai.py`):

- Import guards must follow a consistent pattern (prefer module-level `try/except ImportError` with `None` fallback).
- Fallback dict keys must be consistent across all adapters (use `"fn"` for callable, `"name"` for name, `"description"` for description).
- All adapter tool classes and factory functions must be exported in `__init__.py` and listed in `__all__`.
- `setup.py` `extras_require` must include entries for each framework adapter.
- Exception handlers must be narrowly scoped (`ImportError`, `AttributeError`) — never bare `except Exception`.

## 5. GitHub Actions & CI/CD Security

- All workflow files must include explicit `permissions: {}` at workflow level, with per-job overrides.
- Secrets must never be passed as CLI arguments — use environment variables (`env:` block).
- Avoid double-trigger scenarios (e.g., both `push: tags` and `release: published` triggering the same workflow).
- Async GitHub API calls in `actions/github-script` must use `await`.
- Publishing workflows must run tests and version checks before uploading to npm/PyPI.
- Use `python -m build` instead of deprecated `python setup.py sdist bdist_wheel`.

## 6. Examples & Demos

- Examples must be runnable end-to-end against a live Bridge Server + Runner, or clearly documented as mock/unit-test demos.
- Import paths must match actual package exports (`package.json` `exports` map for JS, `__init__.py` for Python).
- READMEs must include local editable install instructions (`pip install -e ./packages/sdk-python[extra]`) in addition to PyPI install.
- Framework-specific peer dependencies must be documented (e.g., `@langchain/core`, `zod` for the TS LangChain example).
- Fallback behavior (when a framework isn't installed) must be documented, not silently hidden.

## 7. Scripts & Utilities

- JSON parsing must be wrapped in try/catch with descriptive error messages.
- User-provided inputs (version strings, iteration counts) must be validated before use.
- Regex patterns for version extraction should handle whitespace variations.
- Environment variable fallbacks should use nullish coalescing where appropriate and guard against `NaN`.

## 8. Test Coverage

- New adapters and tools must have corresponding unit tests.
- Tests should verify both the happy path (tool returns expected output) and error paths (bridge offline, invalid input).
- Tests should mock the `WebAgentProxyClient` to verify delegation (tool calls the correct client method).
- Graceful degradation tests should verify behavior when optional framework dependencies are not installed.

---

## Review Output Format

When reporting findings, use this severity scale:

- **Critical**: Security vulnerabilities, data leaks, broken functionality that blocks release.
- **High**: Bugs that cause incorrect behavior, misleading documentation, or silent failures.
- **Medium**: Inconsistencies, missing edge case handling, incomplete test coverage.
- **Low**: Style issues, minor documentation gaps, nice-to-have improvements.
