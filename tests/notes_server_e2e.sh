#!/usr/bin/env bash
set -euo pipefail

if [[ "${APPLE_NOTES_E2E:-}" != "1" ]]; then
  echo "skipping live Notes server e2e; set APPLE_NOTES_E2E=1 to run"
  exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required for the live Notes server e2e" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ "${APPLE_NOTES_E2E_SKIP_BUILD:-}" != "1" ]]; then
  cargo build --bins
fi

APPLE_BIN="${APPLE_NOTES_E2E_APPLE_BIN:-$ROOT/target/debug/apple}"
HELPER_BIN="${APPLE_NOTES_E2E_HELPER_BIN:-$ROOT/target/debug/apple-notes-helper}"
LOCAL_BIND="${APPLE_NOTES_E2E_BIND:-127.0.0.1:3769}"
LOCAL_TOKEN="${APPLE_NOTES_E2E_TOKEN:-apple-notes-e2e-token}"
TMP_ROOT="${TMPDIR:-/tmp}/apple-notes-server-e2e-$$"
LOCAL_LOG="$TMP_ROOT/local-server.log"
LOCAL_PID=""
REMOTE_DIR=""

mkdir -p "$TMP_ROOT"

cleanup() {
  local status=$?
  if [[ "$status" -ne 0 && -f "$LOCAL_LOG" ]]; then
    echo "local Notes server log:" >&2
    sed -n '1,200p' "$LOCAL_LOG" >&2 || true
  fi
  if [[ -n "$LOCAL_PID" ]]; then
    kill "$LOCAL_PID" >/dev/null 2>&1 || true
    wait "$LOCAL_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${APPLE_NOTES_E2E_REMOTE_HOST:-}" && -n "$REMOTE_DIR" ]]; then
    local key_args=()
    if [[ -n "${APPLE_NOTES_E2E_REMOTE_SSH_KEY:-}" ]]; then
      key_args=(-i "$APPLE_NOTES_E2E_REMOTE_SSH_KEY")
    fi
    ssh "${key_args[@]}" "$APPLE_NOTES_E2E_REMOTE_HOST" \
      "if [ -f '$REMOTE_DIR/server.pid' ]; then kill \$(cat '$REMOTE_DIR/server.pid') >/dev/null 2>&1 || true; fi; rm -rf '$REMOTE_DIR'" \
      >/dev/null 2>&1 || true
  fi
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

APPLE_NOTES_SERVER_TEMP_ATTACHMENT_TTL_SECS="${APPLE_NOTES_E2E_TEMP_ATTACHMENT_TTL_SECS:-120}" \
APPLE_NOTES_SERVER_TEMP_DIR="${APPLE_NOTES_E2E_TEMP_DIR:-/tmp}" \
"$APPLE_BIN" notes server \
  --bind "$LOCAL_BIND" \
  --token "$LOCAL_TOKEN" \
  --helper "$HELPER_BIN" \
  --backend "${APPLE_NOTES_E2E_BACKEND:-auto}" \
  --poll-interval "${APPLE_NOTES_E2E_POLL_INTERVAL:-2}" \
  >"$LOCAL_LOG" 2>&1 &
LOCAL_PID=$!

python3 - "$LOCAL_BIND" "$LOCAL_TOKEN" <<'PY'
import json
import os
import sys
import time
import urllib.request

base = f"http://{sys.argv[1]}"
token = sys.argv[2]

def request(method, path, body=None, timeout=5):
    data = None
    headers = {"Authorization": f"Bearer {token}"}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.loads(resp.read().decode() or "{}")

deadline = time.time() + 30
while True:
    try:
        health = request("GET", "/health")
        assert health["ok"]
        break
    except Exception:
        if time.time() > deadline:
            raise
        time.sleep(0.5)
PY

REMOTE_TOKEN="${APPLE_NOTES_E2E_REMOTE_TOKEN:-apple-notes-remote-e2e-token}"
REMOTE_PORT="${APPLE_NOTES_E2E_REMOTE_PORT:-3779}"
if [[ -n "${APPLE_NOTES_E2E_REMOTE_HOST:-}" ]]; then
  REMOTE_DIR="/tmp/apple-notes-server-e2e-$$"
  remote_key_args=()
  if [[ -n "${APPLE_NOTES_E2E_REMOTE_SSH_KEY:-}" ]]; then
    remote_key_args=(-i "$APPLE_NOTES_E2E_REMOTE_SSH_KEY")
  fi

  ssh "${remote_key_args[@]}" "$APPLE_NOTES_E2E_REMOTE_HOST" "mkdir -p '$REMOTE_DIR'"
  scp "${remote_key_args[@]}" "$APPLE_BIN" "$HELPER_BIN" "$APPLE_NOTES_E2E_REMOTE_HOST:$REMOTE_DIR/"
  ssh "${remote_key_args[@]}" "$APPLE_NOTES_E2E_REMOTE_HOST" \
    "chmod +x '$REMOTE_DIR/apple' '$REMOTE_DIR/apple-notes-helper'; APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1 '$REMOTE_DIR/apple' notes server --bind 127.0.0.1:$REMOTE_PORT --token '$REMOTE_TOKEN' --helper '$REMOTE_DIR/apple-notes-helper' --backend '${APPLE_NOTES_E2E_REMOTE_BACKEND:-auto}' --poll-interval 2 > '$REMOTE_DIR/server.log' 2>&1 & echo \$! > '$REMOTE_DIR/server.pid'"
fi

python3 - "$LOCAL_BIND" "$LOCAL_TOKEN" <<'PY'
import base64
import http.server
import json
import os
import shlex
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib

base = f"http://{sys.argv[1]}"
token = sys.argv[2]
created_note_id = None
folder_name = f"Apple Notes Server E2E {int(time.time())}"
move_folder_name = f"{folder_name} Moved"
rename_source_name = f"{folder_name} Rename Source"
rename_target_name = f"{folder_name} Rename Target"
title = f"Apple Notes Server E2E Note {int(time.time())}"
webhook_events = []

def png_bytes(width=160, height=96):
    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    rows = []
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            if y < height // 3:
                row.extend([235, 45, 64, 255])
            elif y < 2 * height // 3:
                row.extend([52, 199, 89, 255])
            else:
                row.extend([0, 122, 255, 255])
        rows.append(bytes(row))
    raw = b"".join(rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 1))
        + chunk(b"IEND", b"")
    )

class WebhookHandler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        webhook_events.append(json.loads(body.decode()))
        self.send_response(204)
        self.end_headers()

    def log_message(self, fmt, *args):
        return

def free_port():
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port

def request(method, path, body=None, timeout=300):
    data = None
    headers = {"Authorization": f"Bearer {token}"}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            payload = json.loads(resp.read().decode() or "{}")
    except urllib.error.HTTPError as exc:
        response_body = exc.read().decode(errors="replace")
        raise RuntimeError(f"{method} {path} failed with HTTP {exc.code}: {response_body}") from exc
    if not payload.get("ok", True):
        raise RuntimeError(payload)
    return payload

def step(message):
    print(f"[notes-server-e2e] {message}", flush=True)

def remote_api(method, path, body=None, timeout=180):
    host = os.environ.get("APPLE_NOTES_E2E_REMOTE_HOST")
    if not host:
        return None
    key = os.environ.get("APPLE_NOTES_E2E_REMOTE_SSH_KEY")
    ssh = ["ssh"]
    if key:
        ssh += ["-i", key]
    ssh.append(host)
    remote_token = os.environ.get("APPLE_NOTES_E2E_REMOTE_TOKEN", "apple-notes-remote-e2e-token")
    remote_port = os.environ.get("APPLE_NOTES_E2E_REMOTE_PORT", "3779")
    data = json.dumps(body or {})
    curl = (
        f"/usr/bin/curl -sS -X {shlex.quote(method)} "
        f"-H {shlex.quote('Authorization: Bearer ' + remote_token)} "
        f"-H {shlex.quote('Content-Type: application/json')} "
        f"--data {shlex.quote(data)} "
        f"{shlex.quote('http://127.0.0.1:' + remote_port + path)}"
    )
    result = subprocess.run(ssh + [curl], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError(result.stderr)
    payload = json.loads(result.stdout or "{}")
    if not payload.get("ok", False):
        raise RuntimeError(payload)
    return payload

def note_path(note_id, suffix=""):
    encoded = base64.urlsafe_b64encode(note_id.encode()).decode().rstrip("=")
    return "/v1/notes/" + urllib.parse.quote("b64:" + encoded, safe="") + suffix

def attachment_content_path(note_id, attachment):
    attachment_id = attachment.get("id")
    name = attachment.get("name")
    query = []
    if attachment_id:
        query.append("attachmentId=" + urllib.parse.quote(attachment_id))
    if name:
        query.append("name=" + urllib.parse.quote(name))
    if query:
        return note_path(note_id, "/attachments/content?" + "&".join(query))
    raise AssertionError(f"attachment has neither id nor name: {attachment}")

def wait_for_attachments(note_id, minimum, timeout=30):
    deadline = time.time() + timeout
    last = []
    while time.time() < deadline:
        last = request("GET", note_path(note_id, "/attachments"))["result"]
        if len(last) >= minimum:
            return last
        time.sleep(1)
    raise AssertionError(f"expected at least {minimum} attachment(s), got {last}")

def wait_for_image_attachment(note_id, timeout=30):
    deadline = time.time() + timeout
    last = []
    while time.time() < deadline:
        last = request("GET", note_path(note_id, "/attachments"))["result"]
        for attachment in last:
            uti = attachment.get("typeUTI") or ""
            name = attachment.get("name") or attachment.get("filename") or ""
            if uti.startswith("public.image") or uti in ("public.png", "public.jpeg") or name.lower().endswith((".png", ".jpg", ".jpeg")):
                return attachment
        time.sleep(1)
    raise AssertionError(f"expected an image attachment, got {last}")

def wait_for_attachment_removed(note_id, name, timeout=30):
    deadline = time.time() + timeout
    last = []
    while time.time() < deadline:
        last = request("GET", note_path(note_id, "/attachments"))["result"]
        if all((attachment.get("name") or attachment.get("filename")) != name for attachment in last):
            return last
        time.sleep(1)
    raise AssertionError(f"expected attachment {name!r} to be removed, got {last}")

webhook_port = free_port()
webhook_server = http.server.ThreadingHTTPServer(("127.0.0.1", webhook_port), WebhookHandler)
threading.Thread(target=webhook_server.serve_forever, daemon=True).start()

try:
    step("checking OpenAPI and helper capabilities")
    assert "openapi: 3.1.0" in urllib.request.urlopen(base + "/openapi.yaml", timeout=5).read().decode()
    request("GET", "/v1/capabilities")
    request("GET", "/v1/accounts")
    step("subscribing webhook")
    webhook = request("POST", "/v1/webhooks", {
        "url": f"http://127.0.0.1:{webhook_port}/notes",
        "events": ["note.created", "note.updated", "note.deleted"],
        "secret": "e2e-secret"
    })["result"]
    time.sleep(max(3, int(os.environ.get("APPLE_NOTES_E2E_POLL_INTERVAL", "2")) + 1))

    step("creating folders")
    request("POST", "/v1/folders", {"name": folder_name})
    request("POST", "/v1/folders", {"name": move_folder_name})
    request("POST", "/v1/folders", {"name": rename_source_name})
    request("POST", "/v1/folders/rename", {"name": rename_source_name, "newName": rename_target_name})
    folders = request("GET", "/v1/folders")["result"]
    assert any(folder.get("name") == folder_name for folder in folders)
    assert any(folder.get("name") == rename_target_name for folder in folders)

    step("creating note with table content and inline PNG attachment")
    image = png_bytes()
    html = (
        f"<h1>{title}</h1>"
        "<p>Plain text content from REST.</p>"
        "<table><tr><th>kind</th><th>value</th></tr>"
        "<tr><td>table-cell</td><td>alpha</td></tr></table>"
    )
    created = request("POST", "/v1/notes", {
        "folder": folder_name,
        "title": title,
        "html": html,
        "attachments": [{
            "name": "notes-server-e2e-image.png",
            "mimeType": "image/png",
            "dataBase64": base64.b64encode(image).decode()
        }]
    })["result"]
    created_note_id = created["id"]

    step("reading note text and table content")
    listed = request("GET", f"/v1/notes?folder={urllib.parse.quote(folder_name)}")["result"]
    assert any(note.get("id") == created_note_id for note in listed)
    note = request("GET", note_path(created_note_id))["result"]
    assert title in note["body"]
    assert "table-cell" in note["body"]

    step("reading attachment metadata and content")
    wait_for_attachments(created_note_id, 1)
    image_attachment = wait_for_image_attachment(created_note_id)
    content = request("GET", attachment_content_path(created_note_id, image_attachment))["result"]
    assert base64.b64decode(content["dataBase64"]).startswith(b"\x89PNG")

    step("updating note body and adding a second image")
    updated_html = html + "<p>Updated REST body with second table.</p><table><tr><td>beta</td></tr></table>"
    request("PATCH", note_path(created_note_id), {
        "html": updated_html,
        "attachments": [{
            "name": "notes-server-e2e-image-2.png",
            "mimeType": "image/png",
            "dataBase64": base64.b64encode(png_bytes(96, 64)).decode()
        }]
    })
    updated = request("GET", note_path(created_note_id))["result"]
    assert "Updated REST body" in updated["body"]

    step("adding a third image through the attachments endpoint")
    request("POST", note_path(created_note_id, "/attachments"), {
        "attachments": [{
            "name": "notes-server-e2e-image-3.png",
            "mimeType": "image/png",
            "dataBase64": base64.b64encode(png_bytes(80, 48)).decode()
        }]
    })
    wait_for_attachments(created_note_id, 3)

    step("deleting one image attachment")
    request("POST", note_path(created_note_id, "/attachments/delete"), {
        "name": "notes-server-e2e-image-3.png"
    })
    wait_for_attachment_removed(created_note_id, "notes-server-e2e-image-3.png")

    step("searching and moving the note")
    search = request("GET", f"/v1/notes/search?query={urllib.parse.quote(title)}&limit=5")["result"]
    assert any(note.get("id") == created_note_id for note in search)

    request("POST", note_path(created_note_id, "/move"), {"folder": move_folder_name})
    moved = request("GET", f"/v1/notes?folder={urllib.parse.quote(move_folder_name)}")["result"]
    assert any(note.get("id") == created_note_id for note in moved)

    invitee = os.environ.get("APPLE_NOTES_E2E_INVITEE")
    if invitee:
        step(f"sharing note with {invitee}")
        share_timeout = int(os.environ.get("APPLE_NOTES_E2E_SHARE_TIMEOUT", "90"))
        share = request("POST", "/v1/shares", {
            "noteId": created_note_id,
            "invitee": invitee,
            "backend": os.environ.get("APPLE_NOTES_E2E_SHARE_BACKEND", "auto"),
            "timeout": share_timeout,
        }, timeout=share_timeout + 60)["result"]
        share_url = share.get("share_url") or share.get("url")
        if os.environ.get("APPLE_NOTES_E2E_REMOTE_HOST") and share_url:
            step("accepting share and reading note on remote")
            remote_api("POST", "/v1/shares/accept", {
                "url": share_url,
                "timeout": int(os.environ.get("APPLE_NOTES_E2E_ACCEPT_TIMEOUT", "120")),
            })
            remote_notes = remote_api("GET", "/v1/notes", {})
            assert any(note.get("title") == title or note.get("name") == title for note in remote_notes["result"])

    deadline = time.time() + max(20, int(os.environ.get("APPLE_NOTES_E2E_POLL_INTERVAL", "2")) * 3 + 20)
    while time.time() < deadline and not webhook_events:
        time.sleep(0.5)
    assert webhook_events, "expected at least one webhook event"

    step("deleting note, folders, and webhook subscription")
    request("DELETE", note_path(created_note_id))
    created_note_id = None
    request("POST", "/v1/folders/delete", {"name": folder_name})
    request("POST", "/v1/folders/delete", {"name": move_folder_name})
    request("POST", "/v1/folders/delete", {"name": rename_target_name})
    request("DELETE", f"/v1/webhooks/{webhook['id']}")
finally:
    if created_note_id:
        try:
            request("DELETE", note_path(created_note_id))
        except Exception as exc:
            print(f"cleanup note delete failed: {exc}", file=sys.stderr)
    for name in (folder_name, move_folder_name, rename_source_name, rename_target_name):
        try:
            request("POST", "/v1/folders/delete", {"name": name})
        except Exception:
            pass
    webhook_server.shutdown()

print("notes server e2e passed")
PY
