# Notes Private Helper Protocol

This document describes the stable interface for the private Apple Notes
backend. The intent is to let the Rust CLI, REST server, and a future Java
library use the same process contract while private macOS APIs remain isolated
behind one helper binary.

## Recommendation

Notes is private-helper-only:

- `private`: private Notes/CoreData/CloudKit backend for all Notes operations.
- `auto`: accepted as an alias for `private`.
- `applescript` and `ui`: rejected for Notes operations.

The private backend is more capable and less UI-fragile, but it is also tied to
undocumented Notes frameworks and OS-version behavior. The stable surface should
therefore be this process protocol, not direct Objective-C symbols.

## Runtime Shape

Use a helper executable with JSON Lines over stdin/stdout:

```bash
apple-notes-helper --stdio --backend private
```

The current `apple-notes-helper` binary implements protocol version `1`.
Account, folder, note, attachment, sharing, and share-acceptance operations use
injected Notes-process private helpers. `--backend auto` maps to `private`; no
AppleScript or UI automation fallback exists for Notes.

The helper may internally choose one of two execution modes:

- `notes-process`: a small dylib loaded into the Notes process.
  Required for Notes' Core Data/CloudKit state and entitlements.

The caller should not need to know which mode handled a request. It can inspect
`result.backend` and `result.mode` for diagnostics.

## Request Envelope

Every request is one JSON object on one line.

```json
{
  "id": "6f23ec3d-1a83-4dc3-a0df-0d0f2791d8e4",
  "version": 1,
  "op": "notes.get",
  "params": {
    "id": "x-coredata://..."
  }
}
```

Rules:

- `id` is caller-defined and echoed in the response.
- `version` is the protocol major version. Start at `1`.
- `op` is a stable operation name.
- `params` is always an object, even when empty.

## Response Envelope

Successful response:

```json
{
  "id": "6f23ec3d-1a83-4dc3-a0df-0d0f2791d8e4",
  "ok": true,
  "result": {
    "backend": "private",
    "mode": "notes-process"
  },
  "warnings": []
}
```

Error response:

```json
{
  "id": "6f23ec3d-1a83-4dc3-a0df-0d0f2791d8e4",
  "ok": false,
  "error": {
    "code": "permission.denied",
    "message": "private Notes helper requires DYLD library injection into Notes",
    "retryable": false,
    "details": {
      "permission": "private-notes-helper"
    }
  }
}
```

Error codes should be stable. Suggested initial set:

- `invalid.request`
- `invalid.params`
- `not.found`
- `permission.denied`
- `backend.unavailable`
- `backend.unsupported`
- `notes.timeout`
- `notes.private_api_changed`
- `icloud.unavailable`
- `icloud.share_failed`
- `internal`

## Capability Discovery

`helper.capabilities` reports what is usable on the current machine.

Request:

```json
{"id":"1","version":1,"op":"helper.capabilities","params":{}}
```

Response result:

```json
{
  "protocolVersion": 1,
  "backend": "auto",
  "capabilities": {
    "notesCrud": true,
    "attachments": true,
    "shareCreate": true,
    "shareAccept": true,
    "uiAutomation": false,
    "notesProcessInjection": true
  },
  "diagnostics": {
    "sip": "disabled",
    "accessibility": false,
    "notesAccountCount": 1
  }
}
```

## Core Types

### Note

```json
{
  "id": "x-coredata://...",
  "recordName": "84861689-2898-4729-9E91-58EBDE581CB0",
  "account": "iCloud",
  "folder": "Notes",
  "title": "Test",
  "html": "<div><h1>Test</h1></div>\n<div>Her</div>\n",
  "plaintext": "Test\nHer",
  "createdAt": "2026-06-03T18:49:00Z",
  "modifiedAt": "2026-06-03T18:57:34Z",
  "passwordProtected": false,
  "shared": true,
  "sharedReadOnly": false,
  "share": {
    "url": "https://www.icloud.com/notes/...",
    "participantCount": 2
  }
}
```

Preserve the Core Data URI as `id` because it is what the private helpers can
resolve reliably. Add CloudKit `recordName` when available, but do not make
Java callers depend on it as the primary key.

### Folder

```json
{
  "id": "x-coredata://...",
  "account": "iCloud",
  "name": "Notes",
  "parentId": null,
  "shared": false
}
```

### Attachment

```json
{
  "id": "x-coredata://...",
  "noteId": "x-coredata://...",
  "name": "file.pdf",
  "contentIdentifier": "...",
  "createdAt": "2026-06-03T18:49:00Z",
  "modifiedAt": "2026-06-03T18:49:00Z"
}
```

### Share Participant

```json
{
  "displayName": "Chat",
  "email": "chat@bre.land",
  "currentUser": false,
  "role": "user",
  "permission": "readWrite",
  "acceptanceStatus": "pending"
}
```

## Operation Surface

### Helper

- `helper.capabilities`
- `helper.ping`
- `helper.shutdown`

### Accounts

- `accounts.list`

### Folders

- `folders.list`
- `folders.create`
- `folders.rename`
- `folders.delete`

### Notes

- `notes.list`
- `notes.get`
- `notes.create`
- `notes.update`
- `notes.delete`
- `notes.move`
- `notes.search`
- `notes.show`

### Attachments

- `attachments.list`
- `attachments.save`
- `attachments.delete`

### Sharing

- `shares.create`
- `shares.accept`
- `shares.listParticipants`
- `shares.updateParticipant`
- `shares.removeParticipant`
- `shares.stopSharing`

`shares.create` and `shares.accept` are implemented. Participant management
should be added once the helper can reliably round-trip real shares across two
accounts.

## Example Requests

Create note:

```json
{
  "id": "create-1",
  "version": 1,
  "op": "notes.create",
  "params": {
    "account": "iCloud",
    "folder": "Notes",
    "title": "Test",
    "html": "<p>Hello from Java</p>"
  }
}
```

Share note:

```json
{
  "id": "share-1",
  "version": 1,
  "op": "shares.create",
  "params": {
    "noteId": "x-coredata://...",
    "invitee": "chat@bre.land",
    "permission": "readWrite"
  }
}
```

Accept share:

```json
{
  "id": "accept-1",
  "version": 1,
  "op": "shares.accept",
  "params": {
    "url": "https://www.icloud.com/notes/070Tn8nRDzhRgQQ56xnsMzmFA#Test"
  }
}
```

## Java Usage Sketch

Prefer a process client first. It is easier to ship, debug, and version than JNI
or JNA, and it isolates private macOS APIs from the JVM.

```java
try (AppleNotesClient notes = AppleNotesClient.start("/usr/local/bin/apple-notes-helper")) {
    Note created = notes.createNote(new CreateNoteRequest(
        "iCloud",
        "Notes",
        "From Java",
        "<p>Hello from Java</p>"
    ));

    ShareResult share = notes.share(created.id(), "chat@bre.land", SharePermission.READ_WRITE);
    System.out.println(share.url());
}
```

The client implementation should:

- Start `apple-notes-helper --stdio --backend private`.
- Use one reader thread for stdout JSON lines.
- Keep a `ConcurrentHashMap<String, CompletableFuture<Response>>` by request id.
- Serialize requests with Jackson or another stable JSON library.
- Expose typed Java records for `Note`, `Folder`, `Attachment`, and `Share`.
- Close stdin or send `helper.shutdown` from `close()`.

Skeleton:

```java
public final class AppleNotesClient implements AutoCloseable {
    private final Process process;
    private final BufferedWriter stdin;
    private final ConcurrentHashMap<String, CompletableFuture<JsonNode>> pending = new ConcurrentHashMap<>();

    public static AppleNotesClient start(String helperPath) throws IOException {
        Process process = new ProcessBuilder(helperPath, "--stdio", "--backend", "private").start();
        return new AppleNotesClient(process);
    }

    public CompletableFuture<JsonNode> request(String op, ObjectNode params) throws IOException {
        String id = UUID.randomUUID().toString();
        ObjectNode request = JsonNodeFactory.instance.objectNode();
        request.put("id", id);
        request.put("version", 1);
        request.put("op", op);
        request.set("params", params);

        CompletableFuture<JsonNode> future = new CompletableFuture<>();
        pending.put(id, future);
        stdin.write(request.toString());
        stdin.newLine();
        stdin.flush();
        return future;
    }

    @Override
    public void close() throws IOException {
        stdin.close();
        process.destroy();
    }
}
```

## Roadmap

1. Keep current CLI commands stable while routing all Notes behavior through
   `apple-notes-helper`.
2. Keep the OpenAPI server and Java client on the helper protocol instead of
   binding directly to private Objective-C symbols.
3. Add share participant listing/update/remove once private share creation is
   reliable across two Apple accounts.

## Compatibility Rules

- Do not remove or rename fields in protocol version `1`.
- New response fields are allowed.
- New operations are allowed.
- Breaking changes require protocol version `2`.
- All timestamps should eventually be ISO-8601 UTC.
- Private helper results should include diagnostic `backend` and `mode`, but
  Java callers should not branch on them except for logging and support.
