# Pithmail

A private, local-first desktop email reader. Read and search locally stored mail without AI or a cloud backend.

**Current status: Milestone B implementation, validated with synthetic mail and mocked Gmail/OAuth.** Real Google consent and mailbox synchronization have not yet been tested. Local AI is a later milestone.

## Run

Prerequisites: [Tauri 2 platform requirements](https://v2.tauri.app/start/prerequisites/), Rust stable, and Node.js 24+ (a current LTS release is recommended). On Linux, install your distribution's WebKitGTK 4.1 prerequisites and have an unlocked Secret Service credential store available for real mail.

```sh
npm ci
npm run tauri -- dev
```

If Cargo is not on your PATH after installing Rust, first run `. "$HOME/.cargo/env"`.

The app starts with six synthetic English/Korean emails. No credentials or models are needed for this fixture mailbox. Gmail is opened explicitly in Settings; it never mixes with fixtures.

## Use

- **Today:** important and recent mail, excluding Spam and Trash. This is a deterministic overview, not an AI briefing.
- **Inbox:** all downloaded mail, including archived, sent, Spam, and Trash messages. Load more to continue browsing.
- **Search:** offline keyword/prefix search across subject, sender, and body, including Korean. Multiple words must all match.
- **Reader:** original normalized plaintext, conversations, and attachment metadata. Attachment bytes download during sync, but opening/exporting them is not available yet.
- **Settings:** Gmail connection, synchronization progress and pause/resume/retry, privacy status, and a persisted system/light/dark theme.

Active HTML, remote images, and clickable email links are not rendered. Emails become readable without waiting for all attachments. Pending bodies and attachments remain visible in sync status; they prevent full completion.

## Connect your personal Gmail account

**Live Gmail is not yet validated.** Keep this limitation in mind before downloading a large mailbox. Never paste OAuth credentials or tokens into chat or commit them to Git.

1. In your own [Google Cloud project](https://developers.google.com/workspace/gmail/api/quickstart/python), enable the Gmail API and configure the OAuth consent screen. For an External app in Testing, add your Gmail address as a test user.
2. Create an OAuth client of type **Desktop app**, then download its JSON. Pithmail requests only `gmail.readonly`; do not configure sending or modification scopes.
3. Keep that JSON in a private directory outside this repository. On macOS/Linux, restrict the directory to `0700` and the regular JSON file to `0600`. Do not use a symlink.
4. Launch with its path, replacing the example below with your actual private filename:

   ```sh
   PITHMAIL_OAUTH_CONFIG="$HOME/.config/pithmail/google-desktop.json" npm run tauri -- dev
   ```

5. In **Settings**, choose **Connect / reconnect Gmail** and approve read-only access in your system browser. This opens encrypted storage but **does not automatically download the mailbox**. Cancel sign-in if needed.
6. Choose **Sync now** when ready. This requests the accessible mailbox, including Spam and Trash. Initial download can take a long time; requests are deliberately paced. Pause and resume without discarding checkpoints.

Google's Testing-mode refresh tokens expire after seven days for Gmail scopes. Reconnect when required; offline mail remains available. Public distribution will require separate review of Google's restricted-scope verification requirements. Pithmail does not enable billing or request quota increases.

## Offline use and storage

After restarting, choose **Open encrypted Gmail mailbox** to unlock previously downloaded mail. This action needs the OS credential store, **not a network connection or new Google consent**. Synchronization stays paused until you explicitly resume it. Only one process may open the real mailbox at a time.

Real-mail content, original Gmail FULL JSON, attachment bytes, search indexes, and sync checkpoints are encrypted with SQLCipher. The random database key and OAuth tokens live in macOS Keychain or Linux Secret Service. There is no plaintext fallback. If the credential service is locked/unavailable, unlock it and retry. If the original key is lost, the local database cannot be recovered; preserve it while recovering your credential store. There is no key-export/recovery or mailbox-reset UI yet.

Storage is beneath the OS application-local data directory, normally `~/Library/Application Support/app.pithmail.desktop/` on macOS:

- `synthetic-fixtures-v1/mail.sqlite3`: disposable, **unencrypted synthetic data only**.
- `gmail-v1/mail.sqlite3`: encrypted real mail; companion WAL/SHM and lock files may exist.

Do not copy an open database as a backup, edit its files, delete credential entries, or move only the database to another computer. Portable backup/key recovery is not implemented.

Gmail FULL JSON—not exact original `.eml` bytes—is preserved. Remote label changes are reflected after synchronization. Confirmed remote deletion removes local content, attachments, and search entries; moving a message to Spam/Trash does not delete it locally. An incomplete listing never triggers deletion solely by absence. Gmail's paginated API is not an atomic mailbox snapshot; synchronization converges through history replay and reconciliation.

## Privacy and platform status

Application HTTP is restricted to Gmail retrieval and Google OAuth token exchange. Consent uses Google's system-browser page and a temporary loopback callback. No cloud AI, telemetry, analytics, or crash uploads exist. Proxies and redirects are disabled for application HTTP. Development additionally uses a loopback-only Vite server; built apps need no development server. Dependency installation is a separate development-time network operation.

Native launch, fixture/encrypted-synthetic UI tests, SQLCipher, and a temporary macOS Keychain roundtrip have been verified on macOS ARM64. **Real Google consent/retrieval, Linux/Secret Service/Wayland, Windows, release packaging/signing, and manual visual/accessibility review remain unverified.**

See [architecture and implementation status](docs/architecture.md) for boundaries, limitations, and upcoming local-only llama.cpp work.
