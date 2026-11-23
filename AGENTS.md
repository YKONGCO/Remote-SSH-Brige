# Remote SSH Brige Development Guidelines

## Principles
- Follow the existing Tauri 2, Rust, React, TypeScript, and Vite architecture. Prefer lightweight, maintainable, verifiable solutions over speculative abstractions.
- Keep one source of truth for configuration, permissions, task state, and persistence. Extract modules only when responsibilities are clear or reuse is demonstrated.
- Inspect relevant code and tests before editing. Fix root causes, remove obsolete code when safe, and avoid unrelated changes or compatibility layers without a real need.

## Architecture and Security
- Keep SSH, SFTP, MCP, command policy, approvals, transfers, and jobs in their owning Rust modules under `src-tauri/`. Keep React UI and styles under `src/` following existing conventions.
- Route frontend/native interactions through existing Tauri commands and typed interfaces. Do not duplicate business rules between Rust and React.
- Preserve server permissions, host-key verification, credential protection, read-only restrictions, command safeguards, and audit behavior. Never bypass authorization to make an operation succeed.
- Keep SSH connection lifecycles, execution, jobs, and file transfers clearly separated. Reuse shared connection primitives without coupling unrelated operations or adding remote resident services unnecessarily.
- Preserve safe retry semantics: do not automatically replay a command when execution may already have started; do not sacrifice transfer integrity or atomicity for speed.

## Code Quality
- Follow existing Rust, TypeScript, naming, formatting, and import conventions. Use precise types and explicit error handling; avoid `any`, unsafe assertions, unused code, and ignored errors.
- Fix build, type, lint, and formatting issues at their source. Do not weaken checks or add broad suppressions.
- Prefer self-explanatory code. Avoid unnecessary comments, TODOs, FIXMEs, new dependencies, and over-engineered abstractions.

## UI, Documentation, and Cleanup
- Preserve existing UI layout and interaction patterns; keep labels concise and styles with their owners.
- Keep lasting agent rules here and setup/run instructions in `README.md`. Do not create incidental plans, progress logs, or validation Markdown files unless requested.
- Preserve existing workspace changes, user data, test fixtures, credentials, and release artifacts. Check references before deleting legacy code or generated files.
