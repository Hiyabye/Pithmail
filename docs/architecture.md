# Architecture and implementation status

## Status

**A implemented; B implemented and mock-validated, with live Gmail validation pending.** Native validation is macOS ARM64 only. No real Google consent or mailbox retrieval has occurred. A synthetic temporary macOS Keychain credential was created/read/deleted successfully with explicit approval. Linux Secret Service, Linux/Wayland, Windows, release packaging/signing, and manual visual/accessibility review remain unverified.

B adds encrypted real-mail storage, PKCE/OAuth, fixed-origin Gmail retrieval, resumable full/incremental synchronization, history reconciliation, durable retries and pause/progress, label metadata, and paginated offline browsing/search/thread reading. Fixtures stay isolated and usable without credentials. AI/model/power controls remain absent, not inert placeholders.

## Data flow and ownership

```text
Svelte views/components
  → MailboxViewModel (reactive presentation state, stale-response guards, paging)
    → typed api.ts IPC wrappers
      → async Tauri commands / mailbox session
        → blocking-pool Store access → SQLite/SQLCipher
        → sync engine → GmailSource → GmailClient → centralized Network
                                 → AuthService → OS SecretStore
```

One Rust crate, no server. Frontend has no HTTP client, database access, or token access. Commands return camelCase domain contracts and content-free actionable errors. The ViewModel polls local status while sync is active and refreshes downloaded mail without clearing the reader; Rust also emits `sync-progress`. Database operations and MIME parsing use Tokio's blocking pool. No database mutex is held during network awaits.

Modules:

- `domain`, `mail`: reading/settings contracts, synthetic MIME provider, original A parser.
- `provider`: Gmail wire types, normalized-part contracts, typed provider errors, `GmailSource` boundary used by the adapter and mocked tests. It is not a generic plugin framework.
- `gmail_mime`: bounded FULL-tree traversal, charset/header decoding and HTML-to-text via `mail-parser`. Parser-only synthetic MIME input prevents double transfer decoding; it is never called original MIME.
- `storage` / `storage_gmail`: concrete repository, paging, transactional message/part writes and encryption opening. One persistence implementation does not need a generic repository trait.
- `security`: explicit macOS Keychain/Linux Secret Service stores and fail-closed key provisioning.
- `auth`: private Desktop-client config, system-browser PKCE, bounded loopback callback, OS-token persistence and single-flight refresh.
- `network`: only application HTTP boundary; fixed HTTPS Google origins, no redirects/proxies, timeouts and response limits.
- `gmail`: read-only endpoint adapter, contextual HTTP errors, base64url, refresh-once on 401 and shared request pacing.
- `sync`: persistent one-provider-request-per-step state machine; no UI or credential operations.
- `commands`: session switch, background worker lifecycle, OS/database locking and auth/sync coordination.
- `smoke`: opt-in debug-only native WebView drivers with synthetic data; no added IPC permissions.

## Storage and encryption

Separate databases, never a fixture-to-real promotion:

- A: `synthetic-fixtures-v1/mail.sqlite3`, kind `synthetic-fixtures`, version 1, provider CHECK `fixture`, unencrypted and disposable.
- B: `gmail-v1/mail.sqlite3`, kind `real-gmail`, version 2, provider CHECK `gmail`, SQLCipher. Initial schema and idempotent sync tables are SQL files. Future changes require versioned migrations rather than editing deployed schemas.

Both store stable account/provider IDs, threads, normalized messages, labels and attachment metadata/content. Real storage additionally has original FULL JSON (`provider_messages`), MIME part inventory (`gmail_parts`), original label JSON (`provider_labels`), and durable `sync_state`, `sync_seen`, `sync_work`. Original FULL JSON preserves unmodeled fields and is separate from normalized text. Exact original `.eml` bytes are deliberately not fetched; FULL plus every external part avoids duplicate raw-message bandwidth/quota.

SQLCipher is built with vendored OpenSSL. Opening requires `cipher_version`, a 256-bit raw key and successful keyed schema read before migration/journal changes. Temporary SQL storage is memory-only; encrypted WAL and SQLCipher memory security are enabled. Unix database files are restricted to `0600`, containing directory to `0700`. Tests verify missing/wrong keys fail and tested content markers do not appear in database/WAL bytes; this is not a complete forensic security audit.

The random key and OAuth tokens live in OS credentials, never frontend/config/database files. Binary secrets are base64-encoded for KWallet interoperability. macOS uses the login Keychain; Linux uses an unlocked session Secret Service with encrypted D-Bus communication. No credential-service fallback exists. Lost/damaged keys do not get replaced automatically when a database exists. No passphrase recovery, key export, portable backup, reset or sign-out/data-removal UI exists yet.

A held OS file lock prevents concurrent real-mail sessions and first-key provisioning races. A session defaults to fixtures, then explicitly unlocks Gmail and remains there until restart. Unlocking does not contact Google and durably pauses sync; an explicit Sync now resumes pending work. This deliberate B behavior favors predictable privacy over automatic startup sync; periodic scheduling belongs to D.

## OAuth and account identity

Private local Desktop-app JSON is loaded only in Rust from `PITHMAIL_OAUTH_CONFIG`. Unix group/other permission bits, regular-file/symlink status, size and opened inode are checked. No secrets need to be committed or pasted into chat. Google endpoints are fixed, not taken from config.

Bind `127.0.0.1:0` before opening system-browser consent; use PKCE S256 and independent state. Callback accepts only exact path/Host/method/unique parameters with valid state, bounded headers, per-request and overall timeouts, and cancellation. Invalid unrelated requests do not consume the valid attempt. Only `gmail.readonly` is requested/accepted.

Token refresh is single-flight and preserves the previous refresh token when Google omits it. Transient transport/429/5xx refresh errors retain retry classification/Retry-After; revoked grants require reconnect; local configuration/vault problems remain local errors. Damaged stored token JSON can be repaired by new valid consent without destroying the database key.

Before reconnecting, commands durably pause sync and acquire an auth-transition gate. The worker holds that same gate across each entire provider request and response commit. Thus account credentials cannot change underneath an identity-checked operation. Reconnect leaves sync paused. Explicit resume rechecks `getProfile`; a different email is rejected before any download/deletion against the existing mailbox. Exactly one account is supported.

Google External Testing-mode Gmail refresh tokens expire after seven days. Restricted-scope verification for public distribution is a separate unresolved operational requirement; local-only does not imply automatic exemption.

## Synchronization invariants

1. Fetch profile/verify identity and persist a pre-list history baseline; fetch/persist complete label metadata.
2. List every page with `includeSpamTrash=true`, no INBOX/query filters. Atomically persist discovered IDs, deduplicated work and next-page checkpoint.
3. Drain each page's message FULL fetches, then missing text body parts, before continuing listing. Persist/render each available body immediately; do not wait for attachments.
4. After completed enumeration, verify individually any stored message not seen. Absence alone is never deletion evidence, because Google does not promise snapshot pagination.
5. Replay all history types from the fixed baseline, atomically recording work/deletions/page checkpoints. Refetch message/label changes; apply authoritative deletions. Preserve later history changes for the same message in order.
6. Drain non-body external parts, validating expected sizes. Preserve already downloaded bytes when part ID, attachment ID and size match a label-only refetch.
7. Mark complete/advance committed cursor only with terminal history target and no pending messages, bodies or attachments. Discovered/currently stored counts may differ due to deletions; UI does not infer a completion percentage.

History 404 restarts full reconciliation while retaining readable mail. Invalid nonempty listing-page tokens restart enumeration; unrelated 400s stop visibly. Message GET 404 and explicit history deletion remove normalized content, provider JSON, parts, attachments, search entries and orphan threads. Attachment 404 does not delete the message: it stops incompletely and schedules message refetch on retry. Spam/Trash remain stored/searchable but excluded from Today.

A crash after message persistence but before work removal only repeats an idempotent fetch. Pause persists; an in-flight response may safely commit, but no subsequent request starts while paused. Retry attempts/deadlines and server cooldowns survive restart/manual retry. Exponential backoff has OS-random jitter and a five-attempt visible stop; manual retry retains cooldown. Requests share a conservative one-second minimum pacing interval, including client clones and retries; quota errors still honor server guidance. No billing or quota increase is performed.

Gmail is a convergent API view, not an atomic mailbox census. Changes arriving after the captured history target are handled by the next incremental sync. There is no promise of instantaneous global completeness while a mailbox changes continuously.

## Reading, search and limits

Today remains a deterministic important/recent overview (ten each), not current-calendar-day filtering, extracted tasks or AI. Original normalized text always remains accessible. FULL headers/parts remain preserved separately. Rich HTML rendering, external links, and attachment opening/export are not implemented.

FTS5 Unicode tokenization indexes subject/sender/body; triggers maintain insert/update/delete consistency. Bound queries are literal prefix words joined by AND, at most 512 characters. Inbox, search and conversation endpoints return 100-item pages. Legacy unpaged A commands remain capped at 200 for compatibility; actual UI uses paged endpoints. Morphological Korean analysis and arbitrary substring search are not implemented. Offset pagination can shift during concurrent sync; UI deduplicates results, but does not promise a stable snapshot of a changing list.

MIME bounds: 32 nesting levels, 4,096 parts, 128 MiB/part, 256 MiB decoded total; HTTP also has endpoint-specific size bounds. Unsupported/malformed data produces visible incomplete sync, not false completion. Tests cover UTF-8, legacy charset decoding, encoded headers, plain/HTML alternatives, inline/external parts, nested structure limits and malicious markup. More unusual nested alternative/related messages and real Gmail payload transfer/charset behavior still need live-shaped validation.

## Network and WebView policy

Production CSP defaults deny and allows bundled assets plus local Tauri IPC; development additionally allows loopback Vite HTTP/WebSocket. Inline styles support Svelte; executable received HTML is never inserted. Tauri capabilities remain empty: no frontend shell, filesystem, browser-opener or HTTP permissions; only explicit application commands. Mail HTML, images, scripts, frames and automatic links are not rendered. This is application-level restriction, not an OS network sandbox.

Application HTTP permits only GET Gmail API and POST Google token endpoint over HTTPS. Browser opening is only the explicit Google consent URL; loopback accepts the OAuth callback. No cloud inference, telemetry, crash uploads, remote fallback, or mail-content logs. HTTP error details are classified internally but never echoed with provider content. Model downloads are not implemented.

## Dependencies and validation

Added established crates: SQLCipher-enabled rusqlite, reqwest/Rustls, oauth2 PKCE types, keyring-core and explicit platform stores, serde_json/base64/getrandom/zeroize, open, httpdate. Lockfiles pin exact versions. `mail-parser` enables `full_encoding`. SQLCipher bundles OpenSSL for build portability; Rustls also brings its own cryptographic provider. Linux additionally needs native D-Bus build dependencies and a functioning user Secret Service; these are not validated on this Mac. No extra UI/state/test framework was added.

Validation on macOS ARM64:

- 48 ordinary Rust tests: A behavior, encrypted reopen/FTS/wrong keys, MIME normalization and parts, 205-message pagination, real local mock HTTP responses, token refresh single-flight/errors, endpoint bounds/policy, sync/restart/history/deletion/metadata/part accounting, durable pause/backoff, and credential-transition races.
- One explicitly approved ignored OS-credential test: temporary synthetic macOS Keychain write/read/delete; ordinary tests never touch native credentials.
- Native fixture smoke: all four views, reader/thread/attachments, Korean search, themes, malicious HTML isolation, narrow navigation, injected reader/theme errors and retry.
- Native encrypted-synthetic smoke: 105 Gmail-shaped messages, label names, pending attachments/incomplete status, Inbox/search/thread pagination, offline plaintext and Korean FTS.
- Formatting, strict TypeScript/Svelte checking, ESLint, Clippy, frontend and native debug builds.

No real Google consent/retrieval or large-mailbox performance/energy measurements have run. Do not mark live Gmail or cross-platform acceptance complete based on mock tests.

Sources: [desktop OAuth](https://developers.google.com/identity/protocols/oauth2/native-app), [token expiry](https://developers.google.com/identity/protocols/oauth2#expiration), [Gmail sync](https://developers.google.com/gmail/api/guides/sync), [messages.list](https://developers.google.com/gmail/api/reference/rest/v1/users.messages/list), [formats](https://developers.google.com/gmail/api/reference/rest/v1/Format), [quotas](https://developers.google.com/gmail/api/reference/quota), [SQLCipher](https://www.zetetic.net/sqlcipher/sqlcipher-api/), [rusqlite](https://github.com/rusqlite/rusqlite), [keyring-core](https://docs.rs/keyring-core/latest/keyring_core/), [Tauri CSP](https://v2.tauri.app/security/csp/).

## Next gate

Live consent and limited real retrieval require explicit user authorization and private local OAuth configuration; a full mailbox download should follow user review of size/quota implications. Linux/Secret Service testing remains outstanding. Finish that acceptance before expanding into C: protected local `llama-server`, GGUF import, independently versioned task model assignments/cache keys, structured output validation and persistent analysis queue. D adds source-backed actions/briefing and measured power-aware scheduling; no model selection changes may be hidden inside power settings.
