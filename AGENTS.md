# AGENTS.md (apple-cli)

Read this first if you are an LLM/agent working in this repo.

## What this is

`apple-cli` is a local-first macOS CLI that drives **Notes**, **Reminders**, **Calendar**, and **Messages**. Notes is private-helper-only through `apple-notes-helper`; Reminders, Calendar, and Messages use AppleScript. It runs entirely on device.

Binary name: `apple`

## Quick start

```bash
git clone https://github.com/Sankalpcreat/Apple-CLI.git
cd Apple-CLI
cargo build --release
sudo cp target/release/apple /usr/local/bin/
```

Local install (no sudo):

```bash
cargo install --path .
```

Nix build/dev shell:

```bash
nix build
nix develop
```

First run of Reminders/Calendar/Messages commands will trigger Automation permission prompts.
If commands fail with `-10827` or `AppleEvent handler failed`, enable permissions:
System Settings → Privacy & Security → Automation.

Notes commands require a lab Mac where DYLD library injection into Notes is allowed, usually with SIP/library-injection protections relaxed.

## Repo map

```
apple-cli/
├── README.md
├── AGENTS.md
├── Cargo.toml
├── flake.nix
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
    ├── notes_server.rs
    ├── reminders.rs
    ├── calendar.rs
    └── messages.rs
```

`src/common.rs` contains AppleScript execution helpers and record parsing.

`src/bin/apple-notes-helper.rs` is the JSON-lines protocol helper described in
`docs/notes-private-helper-protocol.md`. Prefer evolving Notes toward that
backend boundary rather than binding Java directly to private Objective-C
symbols.

`openapi/notes-server.yaml` is the source of truth for the Notes REST API.
`build.rs` parses it and generates the Axum route table from operation IDs.
When changing the REST surface, update the OpenAPI spec first and then add or
rename the matching handler in `src/notes_server.rs`.
Path note IDs should support the `b64:<base64url-no-padding(id)>` form because
Apple Notes `x-coredata://...` IDs contain slashes that cannot live safely in a
single router path segment.

## Command surface (high level)

Notes:
- `apple notes accounts list`
- `apple notes folders list|create|delete`
- Notes REST API only: `POST /v1/folders/rename`
- `apple notes list|get|create|update|delete|move|search|show`
- `apple notes share [--backend auto|private]`
- `apple notes shared list|get|accept`
- `apple notes attachments list|save|delete`
- `apple notes server`

Reminders:
- `apple reminders lists`
- `apple reminders lists-create|lists-update|lists-delete`
- `apple reminders list|get|create|update|complete|delete`

Calendar:
- `apple calendar calendars`
- `apple calendar calendars-create|calendars-delete`
- `apple calendar events|get|create|update|delete|show`
- `apple calendar alarms list|add|delete`
- `apple calendar attendees list|add`

Messages:
- `apple messages services`
- `apple messages chats [--type imessage|sms|rcs]`
- `apple messages chat-participants --id <chat_id>`
- `apple messages buddies --type imessage`
- `apple messages send --to <handle> --text "Hello"`
- `apple messages send-chat --id <chat_id> --text "Hello"`

## Known limitations

- Notes: all Notes CLI and REST operations go through `apple-notes-helper --backend private`; `auto` is accepted only as an alias for `private`.
- Notes: private helpers compile and inject temporary Objective-C dylibs into Notes. They only work on lab machines where DYLD library injection into Notes is allowed, and they quit/relaunch Notes.
- Notes: private helpers preflight SIP and fail fast when injection is likely blocked. `APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1` bypasses that check for known-good lab setups.
- Notes: attachment delete can fail on some macOS builds. Deleting the note removes attachments reliably.
- Notes REST API: `apple notes server` binds to localhost by default and serves `/openapi.yaml`. Webhook subscriptions are in-memory and polling-based.
- Calendar: alarm delete can fail on some macOS builds; status updates are best-effort.
- Messages: no transcript/history, read receipts, typing indicators, stickers, or voice notes (not exposed in AppleScript dictionary).

## Testing

There is no always-on automated test suite; live Notes verification is done against a private-helper-capable macOS profile. See README for the latest manual test status and dates.

The Notes REST API has an opt-in live e2e script:

```bash
APPLE_NOTES_E2E=1 bash tests/notes_server_e2e.sh
```

Set `APPLE_NOTES_E2E_INVITEE`, `APPLE_NOTES_E2E_REMOTE_HOST`, and
`APPLE_NOTES_E2E_REMOTE_SSH_KEY` to include sharing and remote share acceptance.

## Safe automation guidance

These commands can delete user data (notes, reminders, events). When adding or running destructive actions:
- Confirm the target IDs and names.
- Prefer creating test folders/lists/calendars first.
- Avoid bulk delete without explicit confirmation.

## When making changes

- Do not add AppleScript fallback paths for Notes. Keep Notes behavior behind `apple-notes-helper`; AppleScript remains appropriate for Reminders/Calendar/Messages.
- Update README and AGENTS.md when command surface changes.
- If you add a new command group, add it to `src/main.rs` and to the README command list.
