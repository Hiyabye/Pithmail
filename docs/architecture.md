# Architecture and implementation status

## Milestone A

Implemented and validated on macOS ARM64: Tauri 2 + Svelte 5/TypeScript/Vite desktop application, six synthetic MIME emails, SQLite migrations/repository, FTS5, Today/Inbox/Search/Settings, plaintext reader, threading, stored attachments and metadata, and persisted themes. No real-mail, authentication, synchronization, inference, or background scheduler stubs exist.

The native smoke test exercises the actual embedded frontend and Rust IPC under the production CSP, without Vite. Linux/Wayland, Windows, release packaging/signing, and manual visual/accessibility review remain unverified. Search results and conversations are bounded at 200 messages; pagination must precede real large-mailbox support.

## Boundaries and data flow

```text
Svelte views/components
  → MailboxViewModel (reactive presentation state, loading/errors, stale-response guards)
    → typed api.ts invoke wrappers
      → asynchronous Tauri commands
        → blocking-pool mailbox access
          → Store / SQLite repository
```

On first access, the mailbox opens the fixture database, migrates it, and transactionally imports the fixture provider. Concurrent startup IPC calls serialize behind one connection mutex. Every database operation, including initialization and MIME parsing, runs on Tokio's blocking pool. No database lock is held across an async suspension. This simple single-connection design is appropriate for fixtures; evaluate contention with measurements before expanding it.

### Rust modules

- `domain`: serializable message summaries/details, attachment metadata, overview, settings.
- `mail`: `MailProvider` read boundary, `FixtureProvider`, and MIME normalization using `mail-parser`. Provider IDs, thread IDs, and labels are distinct from MIME content. The future Gmail adapter should feed normalization through application services; the current synchronous fixture interface is not an HTTP/sync abstraction.
- `storage`: concrete repository and current mail queries/overview policy. There is one persistence implementation, so there is no speculative generic repository trait. Keep this combined boundary small; extract application services when real sync/analysis introduces orchestration.
- `commands`: typed IPC and blocking-work coordination. The UI cannot access SQLite or a provider directly.
- `smoke`: debug-only native test driver, enabled explicitly by environment. It adds no IPC command or Tauri capability and never opens the normal fixture database.

Today is deliberately deterministic: ten most recent visible messages and ten most recent IMPORTANT-labeled messages, excluding SPAM/TRASH. It is not a current-calendar-day filter and does not claim to detect tasks/deadlines. Original normalized message content always remains accessible.

## Persistence

Version 1 stores a fixture-only account; threads; normalized messages; raw MIME in a separate table; labels and message-label relations; attachment bytes; and theme settings. Provider IDs have account-scoped uniqueness, internal IDs are stable namespaced fixture IDs, and foreign keys enforce referential integrity. The account provider CHECK and database-kind marker prevent accidental promotion into a real-mail store.

Migrations are transactional and tracked by `PRAGMA user_version`. Unknown schemas/newer versions are rejected. WAL is enabled only after database ownership is verified. Fixture import is transactional and idempotent, replacing label/attachment sets and upserting messages/raw MIME. Reimport restores compiled-in fixtures on startup; it is **not** a resumable Gmail synchronization pipeline.

FTS5 uses Unicode tokenization over subject, sender, and body. Insert/update/delete triggers maintain consistency. Queries become bound, quoted prefix tokens joined by AND, with a 512-character limit. Arbitrary FTS/SQL syntax is not exposed. Korean whole-word/prefix search is tested; morphological segmentation and arbitrary substring search are not implemented.

The database resides in a `synthetic-fixtures-v1` directory beneath Tauri's OS application-local data directory. Its content is disposable and intentionally unencrypted. Real-mail storage must use a separate encrypted database and separately designed key management; no real account/import route exists in A.

## Security and network policy

- No runtime HTTP client, shell/opener plugin, remote assets, or network adapter is present.
- Production CSP defaults to deny and permits only bundled assets and Tauri IPC. Development additionally permits the loopback Vite HTTP/WebSocket endpoint. Inline styles are permitted for Svelte styling; inline scripts are not broadly permitted.
- Tauri plugin/core capabilities are empty; only the explicit application commands are exposed to local app content. No remote IPC origins or filesystem/shell permissions are granted.
- This is the complete Milestone A network policy: **no application-originated network access**. Before B, introduce one centralized Rust policy for approved OAuth/Gmail endpoints, and in C for protected loopback inference. Do not enable frontend fetch to implement either integration.
- MIME is parsed with an established parser. HTML-only content is converted to plaintext. Svelte escapes all received text, including subjects and attachment names. No received HTML, images, scripts, iframes, or automatic links are inserted into the DOM.
- Raw MIME and attachment bytes are persisted but not executed/opened. Attachment export and explicit external-link opening are absent in A, not silently delegated to the OS.
- Errors avoid content and raw database diagnostics. No message content is logged. There is no remote AI fallback or telemetry.

CSP and minimal permissions reduce the webview attack surface; they are not a claim of OS-level network sandboxing. Plaintext display is intentionally less rich than sanitized HTML, but eliminates remote resources and active markup from the reader. Legacy MIME encodings beyond the tested UTF-8/encoded headers need additional fixtures before B.

## Dependencies and compatibility decisions

Approved foundation: Tauri 2, Tokio, serde, rusqlite with bundled SQLite, mail-parser; Svelte 5, TypeScript, Vite, ESLint, Prettier, svelte-check, Tauri API/CLI. Lockfiles record exact versions. No UI/state framework or extra HTTP/test framework was added.

Bundled SQLite makes FTS5 independent of distribution SQLite configuration; working FTS5 is verified in tests. This bundled plain SQLite selection is **only for fixtures**, not a decision about real-mail encryption. Tauri uses the OS WebView; Linux needs WebKitGTK 4.1 prerequisites, which vary by distribution. No Gentoo package names or Wayland behavior have been validated here.

TypeScript stays on 5.9 because the checked typescript-eslint peer range excludes current TypeScript 7. ESLint 10 is supported by both selected TypeScript and Svelte lint plugins. Vite 8 supports the installed Node 26; current LTS Node is recommended for users. Native validation used Rust stable 1.99.1, macOS ARM64, and Xcode. Browser compilation targets Safari 15/Chrome 105/Firefox 102; that is a compiler target, not a claim that those WebView versions were tested.

Sources consulted before dependency selection:

- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/)
- [Calling Rust commands](https://v2.tauri.app/develop/calling-rust/)
- [Tauri CSP](https://v2.tauri.app/security/csp/)
- [Vite prerequisites](https://vite.dev/guide/)
- [rusqlite upstream](https://github.com/rusqlite/rusqlite)
- [mail-parser API](https://docs.rs/mail-parser/latest/mail_parser/)

## Validation

Eight Rust tests cover migration integrity/constraints, rejection of unrelated/newer databases without journal-mode changes, fixture idempotency, raw/attachment persistence, MIME decoding, malformed input, malicious HTML-to-text, Korean/literal search, FTS updates/deletes, thread/label policy, invalid themes, and offline reopen without a provider.

Native debug smoke launches a real WebView and checks startup IPC, Today, all six Inbox items, message/attachment reading, sent-thread navigation, HTML isolation, Korean search/reading, and theme persistence. It also verifies visible navigation at a narrow window width and retries after injected local IPC reader/theme errors. It uses an isolated temporary database. Formatting, strict Svelte/TypeScript checks, ESLint, Clippy, Rust tests, and frontend/native builds are also run. This is not a complete security audit, visual review, cross-platform test, or synchronization/AI test suite.

## Next milestones and gates

**B — Gmail:** first approve encryption and OS key management (macOS Keychain; Linux credential-service availability/failure policy), secure local OAuth configuration, dependency choices, and the sync schema. Then implement read-only PKCE/OAuth, token refresh, complete bodies-first resumable sync, attachment accounting, incremental history/reconciliation, backoff, progress, pause/resume, and crash recovery. Remote deletion must remove local content/attachments/search entries and later derived analysis after authoritative reconciliation; remote modification must replace normalized/raw content and invalidate only dependent analyses. Do not infer deletion from an interrupted or incomplete page listing. Exact tombstone/checkpoint retention needs a B design decision.

**C — local AI:** Rust-owned protected loopback `llama-server`, manual GGUF import, independently persisted task assignments/cache identities, validated structured outputs, persistent queue, and source-linked summaries/classification. No weights or automatic downloads in A.

**D — briefing/efficiency:** source-backed actions/deadlines, thread summaries, cached daily briefing, measured model-aware scheduling and power profiles. Do not expose inert profile controls or claim battery benefits before implementation and measurements.
