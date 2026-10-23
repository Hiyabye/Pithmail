# Pithmail development

## Scope and invariants

Current implementation covers A and B, with live Google acceptance pending. `docs/architecture.md` is the source of truth for decisions, limitations, schema, and validation status. Do not expand into AI until B acceptance is explicitly settled.

- Svelte/strict TypeScript owns presentation; Rust owns mail/storage/auth/sync logic. No frontend HTTP, database, tokens or direct providers.
- Keep disposable fixtures separate from encrypted Gmail. Never relax the fixture provider CHECK to import real mail. Real data, original provider JSON, parts, search and sync checkpoints all belong inside SQLCipher.
- No native credential fallback or automatic lost-key replacement. Acquire/hold the cross-process mailbox lock before first key provisioning.
- Never use `@html` for received/model content. No remote images, telemetry, cloud inference, or mail/prompts/tokens/summaries in logs.
- OAuth transitions and sync steps must share the transition gate. Pause before reconnect, wait for the in-flight response/commit, and reverify account identity on explicit resume.
- Completion requires every queued message and required part accounted for. Incomplete listing/attachment 404 cannot justify deleting a message. Authoritative remote deletion must remove local dependent content/search.

## Layout

- `src/`: views/components, `lib/mailbox.svelte.ts` ViewModel, `lib/api.ts` typed IPC, `lib/SyncStatus.svelte` progress.
- `src-tauri/`: one crate. `provider` contracts; `mail` fixture parser/provider; `gmail_mime` FULL normalization; `storage`/`storage_gmail`; `security`, `auth`, `network`, `gmail`, `sync`, `commands`.
- `src-tauri/migrations/`: fixture version 1; encrypted initial version 2 and idempotent sync schema. Add versioned migrations for changes to deployed data; do not edit schemas in place after release.
- `fixtures/`: compiled-in synthetic MIME only.
- `*_tests.rs` and inline Rust tests: synthetic/local mock boundaries, no real Google/native vault by default.
- `tests/native-{smoke,gmail-smoke}.js`: actual native WebView drivers for fixtures/encrypted synthetic Gmail.
- `assets/icon.svg`: source of `src-tauri/icons/icon.png`.

## Setup and commands

Use npm plus committed `package-lock.json`, Rust stable/rustfmt/Clippy plus `src-tauri/Cargo.lock`. `File::try_lock` and Linux credential store require a sufficiently recent Rust; use current stable. Node 24+ is required. Platform prerequisites: official Tauri link in README; SQLCipher/vendored OpenSSL requires a C build toolchain and Perl; Linux needs D-Bus development files, pkg-config, and a real unlocked Secret Service for manual credential validation. No Linux setup has been validated on this Mac.

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
PITHMAIL_SMOKE_TEST=gmail src-tauri/target/debug/pithmail
```

Both smoke modes need a graphical session, use temporary synthetic databases, exit pass/fail, and exist only in debug builds. Encrypted mode bypasses OS credentials with a temporary random synthetic key; it is not evidence of live authentication. Neither mode contacts Google or touches normal mailbox files. Ordinary Rust tests use local TCP mocks and in-memory credential mocks. The ignored `native_credential_roundtrip` test writes/reads/deletes one uniquely named native diagnostic entry. The ignored `commands::tests::live_gmail_one_message` test uses existing native tokens and the real encrypted mailbox to retrieve profile, one ID page, and at most one FULL message; no sync worker or external attachment requests. It requires `PITHMAIL_LIVE_CHECK=one-message`, an existing absolute `PITHMAIL_LIVE_MAILBOX_PATH`, and the OAuth config path. Run either ignored test only with explicit authorization and an exact test filter, never as part of a generic `--include-ignored` suite. Close the app before the live check so its cross-process lock can be acquired.

Tauri CLI build is required to validate embedded production assets/custom protocol. `cargo build` alone is insufficient for that check; `npm run dev` alone is a browser preview that intentionally cannot access mail. Release bundling/signing is disabled.

## Development contracts

Rust objects serialize camelCase; synchronize `api.ts` with domain/command/status contracts. Keep errors content-free and actionable. Use parameterized SQL and literal prefix-word search, not raw FTS input. All DB/MIME work remains on the blocking pool; never hold the database mutex across HTTP.

`Network` is the sole application HTTP boundary. Keep release origins fixed, redirects/proxies disabled, and bounds/timeouts enforced. Test endpoint overrides must stay `cfg(test)` and loopback-only. Preserve typed auth/provider errors so token-endpoint outages participate in durable retry rather than forcing consent.

Preserve original Gmail FULL response bytes separately from normalized text; reconstructed MIME is only parser input. Do not claim exact `.eml` preservation. Attachment metadata may precede content; the UI must not describe pending attachments as downloaded.

Config setup and usage belong in README. OAuth JSON must be an owner-only regular file outside Git; pass only its path using `PITHMAIL_OAUTH_CONFIG`. Do not automatically connect a real account, open consent, run native vault tests, or start a real full download during automated development validation. Obtain specific approval first.

Use tests that exercise real boundaries: encrypted reopen/constraints, provider-shaped MIME, local HTTP errors, replay/checkpoints, deletion/part accounting, pause/retry timing and auth-transition concurrency, plus actual UI/IPC. Keep Linux, live Google and performance/energy claims explicitly unverified until measured there. Generated `src-tauri/gen`, targets, local config, mail databases and model weights do not belong in Git.
