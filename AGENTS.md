# Pithmail development

## Scope and invariants

Current scope is Milestone A: an offline synthetic mailbox, not Gmail or AI integration. See `docs/architecture.md` for boundaries, decisions, status, and later-milestone security gates. Do not add placeholder commands/modules for later milestones.

- Keep frontend presentation/state in Svelte + strict TypeScript; all mail/storage business rules belong in Rust.
- Never use real mail in the fixture database. `accounts.provider` is deliberately constrained to `fixture`; do not relax this to implement Gmail. Real-mail storage needs separately approved encryption/key management first.
- Never render received content with `@html`; no frontend networking, external assets, telemetry, or cloud inference.
- Keep email bodies, prompts, tokens, and summaries out of logs.

## Layout

- `src/`: Svelte UI, `lib/mailbox.svelte.ts` ViewModel, `lib/api.ts` typed IPC.
- `src-tauri/`: single Rust crate; `domain`, `mail`, `storage`, `commands`; debug-only `smoke` runner.
- `src-tauri/migrations/`: ordered SQL migrations, tracked through SQLite `user_version`.
- `fixtures/`: compiled-in synthetic MIME; stable provider IDs and labels are in `mail.rs`.
- `tests/native-smoke.js`: actual native WebView UI driver; no browser/mock fallback.
- `assets/icon.svg`: source for `src-tauri/icons/icon.png`.

## Tooling

Use npm and the committed `package-lock.json`; Rust stable with rustfmt/Clippy and `src-tauri/Cargo.lock`. Install platform prerequisites from the official Tauri documentation linked in README. Do not introduce SvelteKit, an additional Rust workspace, or state/UI frameworks without a concrete approved reason.

```sh
npm ci
npm run tauri -- dev
npm run check
npm run lint
npm run format:check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri -- build --debug --no-bundle
PITHMAIL_SMOKE_TEST=1 src-tauri/target/debug/pithmail
```

The final command launches a real native WebView, exercises all four views, and exits with a pass/fail status. It requires a graphical session. Smoke mode exists only in debug builds and uses a fresh temporary fixture database, not the normal application database. Never claim cross-platform validation from the macOS smoke run.

`cargo build` alone does not replace the Tauri build command for embedded frontend validation: Tauri CLI builds with the custom protocol and production assets. `npm run dev` alone is only a browser preview and intentionally cannot access mail.

## Conventions and contracts

Rust domain objects serialize camelCase; keep `src/lib/api.ts` synchronized with `domain.rs` and Tauri command signatures. Command failures return content-free, actionable strings. Database and MIME operations must stay on the blocking pool, never on the UI thread. SQL parameters must be bound; search accepts literal prefix words, not raw FTS expressions.

Use meaningful tests at boundaries: database reopen/constraints, MIME bytes, FTS consistency, and actual UI/IPC. Include a failing reproduction for regressions where practical. Generated `src-tauri/gen`, build outputs, local config, databases, and model weights do not belong in Git.

README is usage-facing; development guidance belongs here. Keep implementation decisions and limitations accurate in `docs/architecture.md`.
