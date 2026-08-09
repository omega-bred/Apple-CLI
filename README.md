# apple-cli

<p align="center"><img src="assets/apple-cli-banner.png" width="420" alt="Apple CLI" /></p>

> **Disclaimer:** This is not an official Apple project. Not affiliated with or endorsed by Apple Inc. Apple, macOS, iMessage, Notes, Reminders, and Calendar are trademarks of Apple Inc.

Apple CLI for macOS. Local-first automation for **Notes**, **Reminders**, **Calendar**, and **Messages** behind a stable CLI surface. Notes uses the private `apple-notes-helper` backend; the other app integrations use AppleScript. Runs entirely on device.

---

## Table of Contents

- [Installation](#installation)
- [Permissions](#permissions)
- [Repository Structure](#repository-structure)
- [Private Notes Backend](#private-notes-backend)
- [Commands Reference](#commands-reference)
  - **Commands:** [notes](#notes) | [reminders](#reminders) | [calendar](#calendar) | [messages](#messages)
- [Testing Status](#testing-status)
- [Requirements](#requirements)
- [License](#license)

---

## Installation

### From source

```bash
git clone https://github.com/Sankalpcreat/Apple-CLI.git
cd Apple-CLI
cargo build --release
sudo cp target/release/apple /usr/local/bin/
```

### Cargo install

```bash
cargo install apple-cli
```

Or install directly from GitHub:

```bash
cargo install --git https://github.com/Sankalpcreat/Apple-CLI.git --bin apple
```

### Nix

```bash
nix build
nix develop
```

**Requirements:** Rust 1.85+ ([rustup.rs](https://rustup.rs))

---

## Permissions

Reminders, Calendar, and Messages use AppleScript. macOS will prompt for **Automation** permissions the first time you call each app.

Required permissions:
- **Reminders**
- **Calendar**
- **Messages**

Notes commands use the private helper route instead of AppleScript. They require a lab Mac where DYLD library injection into Notes is allowed, usually with SIP/library-injection protections relaxed. `APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1` bypasses the local SIP preflight only when you know the target machine allows equivalent injection another way.

If a command fails with `-10827` or `AppleEvent handler failed`, enable permissions here:
**System Settings → Privacy & Security → Automation → allow your terminal/app/binary**.

---

## Repository Structure

```text
apple-cli/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── openapi/
│   └── notes-server.yaml
├── assets/
│   └── apple-cli-banner.png
├── docs/
│   └── notes-private-helper-protocol.md
├── helpers/
│   ├── notes-accept-injected/
│   │   └── AppleNotesAcceptInjected.m
│   └── notes-share-injected/
│       └── AppleNotesShareInjected.m
└── src/
    ├── bin/
    │   └── apple-notes-helper.rs
    ├── main.rs
    ├── common.rs
    ├── notes.rs
    ├── reminders.rs
    ├── calendar.rs
    └── messages.rs
```

---

## Private Notes Backend

Notes commands are implemented through a private JSON-lines helper protocol. The Rust CLI and REST server call this same `apple-notes-helper` boundary, which is also intended to be the stable integration point for a future Java library.

The repository now builds an `apple-notes-helper` binary with the first version of that protocol:

```bash
apple-notes-helper --stdio --backend private
```

See [docs/notes-private-helper-protocol.md](docs/notes-private-helper-protocol.md) for the proposed helper architecture, operation names, response envelopes, and Java client sketch.

---

## Notes REST API Server

`apple notes server` starts a local OpenAPI-backed REST server for Notes. The
server reads [openapi/notes-server.yaml](openapi/notes-server.yaml) at build
time and generates the Axum route table from operation IDs in that spec. Runtime
handlers call `apple-notes-helper` with the private backend. `--backend auto`
is accepted only as an alias for `private`; AppleScript/UI fallback is not used
for Notes operations.

```bash
apple notes server --bind 127.0.0.1:3768
apple notes server --token "$APPLE_NOTES_SERVER_TOKEN"
```

Useful endpoints:

```text
GET    /openapi.yaml
GET    /health
GET    /v1/accounts
GET    /v1/folders
POST   /v1/folders
POST   /v1/folders/delete
POST   /v1/folders/rename
GET    /v1/notes
POST   /v1/notes
GET    /v1/notes/search
GET    /v1/notes/{noteId}
PATCH  /v1/notes/{noteId}
DELETE /v1/notes/{noteId}
POST   /v1/notes/{noteId}/move
GET    /v1/notes/{noteId}/attachments
POST   /v1/notes/{noteId}/attachments
GET    /v1/notes/{noteId}/attachments/content
POST   /v1/notes/{noteId}/attachments/delete
POST   /v1/shares
POST   /v1/shares/accept
GET    /v1/webhooks
POST   /v1/webhooks
DELETE /v1/webhooks/{webhookId}
```

Notes IDs such as `x-coredata://...` are opaque and contain slashes. When using
an ID in a path parameter, encode it as `b64:<base64url-no-padding(id)>`.
JSON body fields such as share `noteId` may use the raw ID or the same `b64:`
form. Note create/update endpoints accept HTML bodies and attachments either
as local file paths or inline `dataBase64` payloads. Inline attachments are
written to temporary files before being handed to Notes, then deleted after
`APPLE_NOTES_SERVER_TEMP_ATTACHMENT_TTL_SECS` seconds (default: 600) so Notes
has time to materialize the file. Set `APPLE_NOTES_SERVER_TEMP_DIR` to choose
where those files are staged.

Webhook subscriptions are in-memory and polling-based. Notes does not expose a
native push-change feed through this helper yet, so the server polls `notes.list` and emits
`note.created`, `note.updated`, and `note.deleted` events to subscribed URLs.

Live end-to-end testing is source-controlled but opt-in because it touches a
real macOS Notes profile:

```bash
APPLE_NOTES_E2E=1 bash tests/notes_server_e2e.sh
```

To exercise sharing and remote share acceptance, also provide an invitee and
remote machine:

```bash
APPLE_NOTES_E2E=1 \
APPLE_NOTES_E2E_INVITEE=chat@bre.land \
APPLE_NOTES_E2E_REMOTE_HOST=100.95.244.120 \
APPLE_NOTES_E2E_REMOTE_SSH_KEY="$HOME/.ssh/id_ed25519" \
bash tests/notes_server_e2e.sh
```

The script creates and renames folders, creates a note with HTML table content,
adds inline PNG images, reads note and attachment content back, moves/searches
the note, checks local webhook delivery, and deletes the test data.

---

## Commands Reference

### notes

```bash
apple notes accounts list
apple notes folders list|create|delete
apple notes list|get|create|update|delete|move|search|show
apple notes share
apple notes shared list|get|accept
apple notes attachments list|save|delete
apple notes server
```

Examples:
```bash
apple notes create --folder "Notes" --name "Test" --body "<p>Hello</p>"
apple notes share <note_id> --email "person@example.com"
apple notes share <note_id> --email "person@example.com" --backend private
apple notes shared list
apple notes shared accept --url "https://www.icloud.com/notes/..."
apple notes update <note_id> --body "<p>Updated</p>" --attach /path/file.pdf
apple notes attachments list <note_id>
```

Notes limitations:
- Notes operations are private-helper-only. `--backend auto` is accepted as an alias for `private`; `ui` and `applescript` are rejected.
- The helper compiles a temporary Objective-C dylib with `clang`, quits/relaunches Notes with `DYLD_INSERT_LIBRARIES`, and writes the helper result from inside the Notes sandbox. This is intended for local lab machines with SIP/library-injection protections relaxed.
- Private helpers fail fast when SIP is enabled because macOS strips or blocks the required DYLD injection. Set `APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1` only when you know the target machine allows equivalent injection another way.
- Attachment **delete** is best-effort on some macOS builds. Deleting the note removes attachments reliably.

---

### reminders

```bash
apple reminders lists
apple reminders lists-create|lists-update|lists-delete
apple reminders list|get|create|update|complete|delete
```

Examples:
```bash
apple reminders create --list "Reminders" --title "Pay rent" --due "2026-03-14" --flagged true
apple reminders update <id> --remind-me "2026-03-13 18:00:00"
```

Supported fields:
- due date, all-day due date, remind-me date, priority, flagged, completed

---

### calendar

```bash
apple calendar calendars
apple calendar calendars-create|calendars-delete
apple calendar events|get|create|update|delete|show
apple calendar alarms list|add|delete
apple calendar attendees list|add
```

Examples:
```bash
apple calendar create --calendar "Work" --title "Standup" --start "2026-03-13 10:00:00" --end "2026-03-13 10:30:00"
apple calendar update <event_id> --recurrence "RRULE:FREQ=DAILY;COUNT=2"
apple calendar alarms add <event_id> --type display --minutes=-15
```

Calendar limitations:
- Alarm **delete** can fail with `AppleEvent handler failed` on some macOS builds. Workaround: delete the event.
- Status updates are best-effort; if Calendar rejects the status, the command still succeeds for other fields.

---

### messages

```bash
apple messages services
apple messages chats [--type imessage|sms|rcs]
apple messages chat-participants --id <chat_id>
apple messages buddies --type imessage
apple messages send --to <handle> --text "Hello"
apple messages send-chat --id <chat_id> --text "Hello"
```

Messages limitations:
- No transcript/history, read receipts, typing indicators, stickers, or voice notes (not in AppleScript dictionary).

---

## Testing Status (2026-03-13)

**Notes**
- Passed: create/get/update/search/move/delete; attachments create/list/save
- Known issue: attachments delete can fail with `AppleEvent handler failed`

**Reminders**
- Passed: lists CRUD, reminder CRUD, all fields (due/allday/remind/flagged/priority)

**Calendar**
- Passed: calendars list/create/delete; events CRUD; recurrence update; alarms add/list; attendees add/list
- Known issues: alarm delete can fail; status set is best-effort

**Messages**
- Passed: services, chats, chat participants, buddies
- Not tested: send (requires explicit recipient)

---

## Requirements

- macOS with Notes/Reminders/Calendar/Messages apps
- Rust (stable) for build
- Automation permissions for each target app

---

## License

MIT
