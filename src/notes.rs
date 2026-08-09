use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    FoldersCreateArgs, FoldersDeleteArgs, FoldersListArgs, NotesAttachmentsDeleteArgs,
    NotesAttachmentsListArgs, NotesAttachmentsSaveArgs, NotesCreateArgs, NotesDeleteArgs,
    NotesGetArgs, NotesListArgs, NotesMoveArgs, NotesSearchArgs, NotesShareArgs,
    NotesSharedAcceptArgs, NotesSharedGetArgs, NotesSharedListArgs, NotesShowArgs, NotesUpdateArgs,
};

pub fn accounts_list() -> Result<()> {
    print_helper_result("accounts.list", json!({}))
}

pub fn folders_list(args: FoldersListArgs) -> Result<()> {
    print_helper_result(
        "folders.list",
        compact_json(json!({
            "account": args.account
        })),
    )
}

pub fn folders_create(args: FoldersCreateArgs) -> Result<()> {
    print_helper_result(
        "folders.create",
        compact_json(json!({
            "account": args.account,
            "name": args.name,
            "parent": args.parent
        })),
    )
}

pub fn folders_delete(args: FoldersDeleteArgs) -> Result<()> {
    print_helper_result(
        "folders.delete",
        compact_json(json!({
            "account": args.account,
            "name": args.name,
            "parent": args.parent
        })),
    )
}

pub fn notes_list(args: NotesListArgs) -> Result<()> {
    print_helper_result(
        "notes.list",
        compact_json(json!({
            "account": args.account,
            "folder": args.folder
        })),
    )
}

pub fn notes_get(args: NotesGetArgs) -> Result<()> {
    print_helper_result("notes.get", json!({ "id": args.id }))
}

pub fn notes_create(args: NotesCreateArgs) -> Result<()> {
    print_helper_result(
        "notes.create",
        compact_json(json!({
            "account": args.account,
            "folder": args.folder,
            "title": args.name,
            "html": args.body,
            "attachments": non_empty_array(args.attach)
        })),
    )
}

pub fn notes_update(args: NotesUpdateArgs) -> Result<()> {
    print_helper_result(
        "notes.update",
        compact_json(json!({
            "id": args.id,
            "title": args.name,
            "html": args.body,
            "attachments": non_empty_array(args.attach)
        })),
    )
}

pub fn notes_delete(args: NotesDeleteArgs) -> Result<()> {
    print_helper_result("notes.delete", json!({ "id": args.id }))
}

pub fn notes_move(args: NotesMoveArgs) -> Result<()> {
    print_helper_result(
        "notes.move",
        compact_json(json!({
            "id": args.id,
            "account": args.account,
            "folder": args.folder
        })),
    )
}

pub fn notes_share(args: NotesShareArgs) -> Result<()> {
    ensure_private_backend(&args.backend)?;
    if args.open_only {
        return Err(anyhow!(
            "--open-only was a UI share-sheet option and is not supported by the private Notes helper"
        ));
    }
    if args.service != "copy-link" {
        return Err(anyhow!(
            "--service was a UI share-sheet option and is not supported by the private Notes helper"
        ));
    }
    print_helper_result(
        "shares.create",
        json!({
            "noteId": args.id,
            "invitee": args.email,
            "timeout": args.timeout
        }),
    )
}

pub fn notes_shared_list(args: NotesSharedListArgs) -> Result<()> {
    print_helper_result(
        "shared.list",
        compact_json(json!({
            "account": args.account,
            "folder": args.folder
        })),
    )
}

pub fn notes_shared_get(args: NotesSharedGetArgs) -> Result<()> {
    print_helper_result("shared.get", json!({ "id": args.id }))
}

pub fn notes_shared_accept(args: NotesSharedAcceptArgs) -> Result<()> {
    print_helper_result(
        "shares.accept",
        json!({
            "url": args.url,
            "timeout": args.timeout
        }),
    )
}

pub fn notes_search(args: NotesSearchArgs) -> Result<()> {
    print_helper_result(
        "notes.search",
        compact_json(json!({
            "account": args.account,
            "query": args.query,
            "limit": args.limit
        })),
    )
}

pub fn notes_show(args: NotesShowArgs) -> Result<()> {
    print_helper_result("notes.show", json!({ "id": args.id }))
}

pub fn notes_attachments_list(args: NotesAttachmentsListArgs) -> Result<()> {
    print_helper_result("attachments.list", json!({ "noteId": args.id }))
}

pub fn notes_attachments_save(args: NotesAttachmentsSaveArgs) -> Result<()> {
    print_helper_result(
        "attachments.save",
        compact_json(json!({
            "noteId": args.id,
            "attachmentId": args.attachment_id,
            "name": args.name,
            "output": args.output
        })),
    )
}

pub fn notes_attachments_delete(args: NotesAttachmentsDeleteArgs) -> Result<()> {
    print_helper_result(
        "attachments.delete",
        compact_json(json!({
            "noteId": args.id,
            "attachmentId": args.attachment_id,
            "name": args.name
        })),
    )
}

fn print_helper_result(op: &'static str, params: Value) -> Result<()> {
    let result = helper_call(op, params)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn helper_call(op: &'static str, params: Value) -> Result<Value> {
    let helper = helper_binary();
    let request = json!({
        "id": request_id(),
        "version": 1,
        "op": op,
        "params": params
    });
    let request_line = serde_json::to_string(&request)?;

    let mut child = Command::new(&helper)
        .arg("--stdio")
        .arg("--backend")
        .arg("private")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {}", helper.display()))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("{} stdin unavailable", helper.display()))?;
        writeln!(stdin, "{request_line}")
            .with_context(|| format!("failed to write request to {}", helper.display()))?;
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {}", helper.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(anyhow!(
            "{} failed: {}",
            helper.display(),
            if stderr.is_empty() {
                stdout.trim()
            } else {
                &stderr
            }
        ));
    }

    let line = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| anyhow!("{} returned no JSON response: {stderr}", helper.display()))?;
    let response: Value = serde_json::from_str(line)
        .with_context(|| format!("failed to parse {} response: {line}", helper.display()))?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(response.get("result").cloned().unwrap_or(Value::Null));
    }

    let message = response
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("Apple Notes private helper request failed");
    Err(anyhow!(message.to_string()))
}

fn helper_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("APPLE_NOTES_HELPER") {
        return PathBuf::from(path);
    }
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.parent()
                .map(|parent| parent.join("apple-notes-helper"))
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("apple-notes-helper"))
}

fn request_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("notes-cli-{}-{}", std::process::id(), now.as_nanos())
}

fn ensure_private_backend(backend: &str) -> Result<()> {
    match backend {
        "auto" | "private" => Ok(()),
        "applescript" | "ui" => Err(anyhow!(
            "Apple Notes sharing is private-helper only; backend {backend:?} is no longer supported"
        )),
        other => Err(anyhow!("unsupported Notes sharing backend: {other}")),
    }
}

fn non_empty_array(values: Vec<String>) -> Value {
    if values.is_empty() {
        Value::Null
    } else {
        Value::Array(values.into_iter().map(Value::String).collect())
    }
}

fn compact_json(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(compact_map(map)),
        other => other,
    }
}

fn compact_map(map: Map<String, Value>) -> Map<String, Value> {
    map.into_iter()
        .filter_map(|(key, value)| match value {
            Value::Null => None,
            Value::Array(items) if items.is_empty() => None,
            other => Some((key, other)),
        })
        .collect()
}
