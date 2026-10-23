# Pithmail

A private, local-first desktop email reader. Read and search locally stored mail without AI or a cloud backend.

**Current status: Milestone A — synthetic fixture mailbox.** Gmail connection and local AI are not available yet. Do not import real mail into this version: its disposable fixture database is not encrypted.

## Run

Prerequisites: [Tauri 2 platform requirements](https://v2.tauri.app/start/prerequisites/), Rust stable, and Node.js 24+ (a current LTS release is recommended). On Linux, install your distribution's WebKitGTK 4.1 and other Tauri prerequisites first.

```sh
npm ci
npm run tauri -- dev
```

If Cargo is not on your PATH after installing Rust, first run `. "$HOME/.cargo/env"`.

The app opens six synthetic English/Korean emails, including a conversation, an attachment, archived mail, Spam, and Trash. No credentials or models are required.

## Use

- **Today:** important and recent mail, excluding Spam and Trash. This is a deterministic overview, not an AI briefing.
- **Inbox:** browse all six locally stored messages and read conversations.
- **Search:** offline keyword search across subjects, senders, and bodies. Multiple words must all match; word prefixes are supported. Korean text is supported.
- **Settings:** choose a persisted light, dark, or device theme and inspect the current privacy limitations.

Messages display as plaintext. Active HTML, external images, and clickable email links are not rendered. Attachment bytes are stored, but opening/exporting them is not yet available.

The fixture database lives under the OS application-local data directory in `app.pithmail.desktop/synthetic-fixtures-v1/mail.sqlite3` (macOS: `~/Library/Application Support/app.pithmail.desktop/synthetic-fixtures-v1/mail.sqlite3`). It contains synthetic data only. Fixture messages are restored idempotently at startup; theme preferences persist.

## Privacy and availability

This milestone makes no application-level network requests and has no telemetry, analytics, AI service, or backend server. Development uses a loopback-only Vite server; a built application needs no development server or network connection. Dependency installation is a separate development-time network operation.

Native launch and UI integration have been tested on macOS ARM64. Linux, including Gentoo/Hyprland/Wayland, and Windows are **not yet validated**. Installable application packages, code signing, and automatic updates are not provided.

The project will grow incrementally into encrypted Gmail synchronization and local-only llama.cpp analysis. See [architecture and implementation status](docs/architecture.md).
