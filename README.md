# apple-cli

<p align="center"><img src="assets/apple-cli-banner.png" width="420" alt="Apple CLI" /></p>

> **Disclaimer:** This is not an official Apple project. Not affiliated with or endorsed by Apple Inc. Apple, macOS, iMessage, Notes, Reminders, and Calendar are trademarks of Apple Inc.

Apple CLI for macOS. Local-first automation for **Notes**, **Reminders**, **Calendar**, and **Messages** using AppleScript behind a stable CLI surface. Runs entirely on device.

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

This CLI uses AppleScript. macOS will prompt for **Automation** permissions the first time you call each app.

Required permissions:
- **Notes**
- **Reminders**
- **Calendar**
- **Messages**

If a command fails with `-10827` or `AppleEvent handler failed`, enable permissions here:
**System Settings → Privacy & Security → Automation → allow your terminal/app/binary**.

---

## Repository Structure

```text
apple-cli/
├── Cargo.toml
├── Cargo.lock
├── README.md
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

Notes is moving toward a backend model rather than a full AppleScript replacement in one jump. The current recommendation is to keep AppleScript/UI automation as a fallback and expose private Notes functionality through a stable JSON-lines helper protocol that both the Rust CLI and a future Java library can use.

The repository now builds an `apple-notes-helper` binary with the first version of that protocol:

```bash
apple-notes-helper --stdio --backend auto
```

See [docs/notes-private-helper-protocol.md](docs/notes-private-helper-protocol.md) for the proposed helper architecture, operation names, response envelopes, and Java client sketch.

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
- Notes sharing is driven through the macOS Notes share sheet because Notes exposes only a read-only `shared` flag in its AppleScript dictionary. It requires Automation access to Notes and Accessibility access for UI scripting.
- `apple notes share` defaults to `--backend auto`: it uses the private helper when the SIP preflight says Notes DYLD injection is available, otherwise it uses the UI share-sheet backend.
- `apple notes share --backend private` uses a private Notes-process helper for machines where DYLD injection into Notes is allowed. It compiles a temporary Objective-C dylib with `clang`, quits/relaunches Notes with `DYLD_INSERT_LIBRARIES`, and writes the helper result from inside the Notes sandbox. This is intended for local lab machines with SIP/library-injection protections relaxed.
- `apple notes shared accept` uses the same private injected-helper approach to accept an iCloud Notes share URL from inside Notes.
- Private helpers fail fast when SIP is enabled because macOS strips or blocks the required DYLD injection. Set `APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1` only when you know the target machine allows equivalent injection another way.
- Attachment **delete** is best-effort; some builds return `AppleEvent handler failed`. Deleting the note removes attachments reliably.
- Some Notes UI features (tables/checklists/voice notes) are not scriptable.

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
