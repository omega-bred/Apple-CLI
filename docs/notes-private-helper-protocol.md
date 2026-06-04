# Notes Private Helper Protocol

This document sketches the stable interface for a private Apple Notes backend.
The intent is to let the Rust CLI and a future Java library use the same
contract while the implementation can choose the safest available macOS
mechanism per operation.

## Recommendation

Do not replace every Notes command with private APIs in one step. Port Notes to
a backend model:

- `applescript`: supported macOS automation path for ordinary CRUD.
- `private`: private Notes/CoreData/CloudKit backend for functionality that
  AppleScript cannot expose, especially sharing and share acceptance.
- `auto`: prefer `private` when the machine supports the required helper mode,
  otherwise fall back to `applescript` or UI automation.

The private backend is more capable and less UI-fragile, but it is also tied to
undocumented Notes frameworks and OS-version behavior. The stable surface should
therefore be a process protocol, not direct Objective-C symbols.

## Runtime Shape

Use a helper executable with JSON Lines over stdin/stdout:

```bash
apple-notes-helper --stdio --backend auto
```

The current `apple-notes-helper` binary implements protocol version `1`.
Ordinary Notes, folder, account, and attachment operations are adapter-backed by
AppleScript today. `shares.create` and `shares.accept` call the proven
Notes-process private helpers so Java callers can use the same stable protocol
while the internals continue moving toward private APIs.

The helper may internally choose one of two execution modes:

- `standalone`: private framework/CoreData access in the helper process.
  Good candidate for account/folder/note/attachment CRUD.
- `notes-process`: a small dylib loaded into the Notes process.
  Required for operations that need Notes' CloudKit entitlements, such as
  creating or accepting iCloud shares.

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
    "message": "Accessibility access is disabled",
    "retryable": false,
    "details": {
      "permission": "accessibility"
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

Preserve the Core Data URI as `id` because it is what Notes AppleScript and the
current private helpers can resolve reliably. Add CloudKit `recordName` when
available, but do not make Java callers depend on it as the primary key.

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

Only `shares.create` and `shares.accept` are proven private-helper operations
today. Participant management should be added once the helper can round-trip
real shares across two accounts.

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

- Start `apple-notes-helper --stdio --backend auto`.
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
        Process process = new ProcessBuilder(helperPath, "--stdio", "--backend", "auto").start();
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

## Migration Plan

1. Keep current CLI commands stable.
2. Introduce `apple-notes-helper --stdio` and implement `helper.capabilities`.
3. Move proven sharing and share acceptance into protocol handlers.
4. Port read-only Notes operations: accounts, folders, list, get, search,
   attachments list.
5. Port write operations: create, update, delete, move, attachment save/delete.
6. Add Java client package against the protocol.
7. Make the Rust CLI call the helper for Notes when `--backend private` or
   `--backend auto` selects private, with AppleScript retained as fallback.

## Compatibility Rules

- Do not remove or rename fields in protocol version `1`.
- New response fields are allowed.
- New operations are allowed.
- Breaking changes require protocol version `2`.
- All timestamps should eventually be ISO-8601 UTC. Existing AppleScript-facing
  commands may keep human-readable macOS date strings until migrated.
- Private helper results should include diagnostic `backend` and `mode`, but
  Java callers should not branch on them except for logging and support.
