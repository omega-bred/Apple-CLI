use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "../common.rs"]
mod common;

use common::{parse_records, run_applescript, FS, RS};

const INJECTED_PRIVATE_HELPER_SOURCE: &str =
    include_str!("../../helpers/notes-private-injected/AppleNotesPrivateInjected.m");

#[derive(Parser)]
#[command(
    name = "apple-notes-helper",
    version,
    about = "JSON-lines helper protocol for Apple Notes"
)]
struct Cli {
    /// Read JSON requests from stdin and write JSON responses to stdout
    #[arg(long)]
    stdio: bool,
    /// Backend preference: auto, applescript, or private
    #[arg(long, default_value = "auto")]
    backend: String,
}

#[derive(Debug, Deserialize)]
struct Request {
    id: Value,
    version: u64,
    op: String,
    #[serde(default)]
    params: Value,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if !cli.stdio {
        return Err(anyhow!("apple-notes-helper currently requires --stdio"));
    }
    run_stdio(&cli.backend)
}

fn run_stdio(backend: &str) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let parsed = serde_json::from_str::<Request>(&line);
        let (response, should_shutdown) = match parsed {
            Ok(request) => {
                let should_shutdown = request.op == "helper.shutdown";
                (handle_request(request, backend), should_shutdown)
            }
            Err(error) => (
                json!({
                    "id": Value::Null,
                    "ok": false,
                    "error": {
                        "code": "invalid.request",
                        "message": error.to_string(),
                        "retryable": false
                    }
                }),
                false,
            ),
        };

        writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
        stdout.flush()?;
        if should_shutdown {
            break;
        }
    }

    Ok(())
}

fn handle_request(request: Request, backend: &str) -> Value {
    let id = request.id.clone();
    if request.version != 1 {
        return error_response(
            id,
            "invalid.request",
            format!("unsupported protocol version: {}", request.version),
            false,
            json!({ "supportedVersion": 1 }),
        );
    }

    match dispatch(&request.op, &request.params, backend) {
        Ok(result) => json!({
            "id": id,
            "ok": true,
            "result": result,
            "warnings": []
        }),
        Err(error) => error_response(
            id,
            classify_error(&error),
            error.to_string(),
            false,
            Value::Null,
        ),
    }
}

fn dispatch(op: &str, params: &Value, backend: &str) -> Result<Value> {
    if should_try_private_operation(op, backend) {
        match run_private_operation(op, params) {
            Ok(result) => return Ok(result),
            Err(error) if backend == "auto" => {
                eprintln!("apple-notes-helper private backend fallback for {op}: {error}");
            }
            Err(error) => return Err(error),
        }
    }

    match op {
        "helper.ping" => Ok(json!({ "pong": true })),
        "helper.shutdown" => Ok(json!({ "shutdown": true })),
        "helper.capabilities" => helper_capabilities(backend),
        "accounts.list" => accounts_list(),
        "folders.list" => folders_list(params),
        "folders.create" => folders_create(params),
        "folders.rename" => folders_rename(params),
        "folders.delete" => folders_delete(params),
        "notes.list" => notes_list(params),
        "notes.get" => notes_get(params),
        "notes.create" => notes_create(params),
        "notes.update" => notes_update(params),
        "notes.delete" => notes_delete(params),
        "notes.move" => notes_move(params),
        "notes.search" => notes_search(params),
        "notes.show" => notes_show(params),
        "attachments.list" => attachments_list(params),
        "attachments.save" => attachments_save(params),
        "attachments.delete" => attachments_delete(params),
        "shares.create" => shares_create(params, backend),
        "shares.accept" => shares_accept(params),
        other => Err(anyhow!("unsupported operation: {other}")),
    }
}

fn should_try_private_operation(op: &str, backend: &str) -> bool {
    if !matches!(backend, "auto" | "private") {
        return false;
    }
    let supported = matches!(
        op,
        "accounts.list"
            | "folders.list"
            | "folders.create"
            | "folders.rename"
            | "folders.delete"
            | "notes.list"
            | "notes.get"
            | "notes.create"
            | "notes.update"
            | "notes.delete"
            | "notes.move"
            | "notes.search"
            | "attachments.list"
            | "attachments.save"
            | "attachments.delete"
    );
    supported && (backend == "private" || private_notes_injection_available())
}

fn private_notes_injection_available() -> bool {
    env::var_os("APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT").is_some()
        || sip_status().as_deref() == Some("disabled")
}

fn run_private_operation(op: &str, params: &Value) -> Result<Value> {
    let work_dir = PathBuf::from("/tmp").join(format!(
        "apple-cli-notes-private-{}-{}",
        std::process::id(),
        uuid_like_timestamp()
    ));
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    let source_path = work_dir.join("AppleNotesPrivateInjected.m");
    let dylib_path = work_dir.join("libAppleNotesPrivateInjected.dylib");
    let request_path = work_dir.join("request.json");
    let log_path = work_dir.join("notes-private.log");
    fs::write(&source_path, INJECTED_PRIVATE_HELPER_SOURCE)
        .with_context(|| format!("failed to write {}", source_path.display()))?;
    compile_injected_helper(&source_path, &dylib_path)?;

    let prepared_params = prepare_private_params(&work_dir, op, params)?;
    fs::write(
        &request_path,
        serde_json::to_vec(&json!({ "op": op, "params": prepared_params }))?,
    )
    .with_context(|| format!("failed to write {}", request_path.display()))?;

    let result_path = notes_private_result_path(&format!(
        "notes-private-result-{}-{}.json",
        std::process::id(),
        uuid_like_timestamp()
    ))?;
    if let Some(parent) = result_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let _ = fs::remove_file(&result_path);
    let _ = fs::remove_file(&log_path);

    let _ = Command::new("/usr/bin/killall").arg("Notes").status();
    thread::sleep(Duration::from_secs(1));

    let log_file = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;
    Command::new("/System/Applications/Notes.app/Contents/MacOS/Notes")
        .env("DYLD_INSERT_LIBRARIES", &dylib_path)
        .env("APPLE_CLI_NOTES_PRIVATE_REQUEST", &request_path)
        .env("APPLE_CLI_NOTES_PRIVATE_RESULT", &result_path)
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("failed to relaunch Notes with the injected private helper")?;

    let timeout = params
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(180)
        .max(1);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    while Instant::now() < deadline {
        if result_path.exists() {
            let result_text = fs::read_to_string(&result_path)
                .with_context(|| format!("failed to read {}", result_path.display()))?;
            let result: Value = serde_json::from_str(&result_text)
                .with_context(|| format!("failed to parse {}", result_path.display()))?;
            if result.get("status").and_then(Value::as_str) == Some("ok") {
                let payload = result.get("result").cloned().unwrap_or(Value::Null);
                return postprocess_private_result(op, params, payload);
            }
            return Err(anyhow!(
                "{}",
                result
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("private Notes helper returned an error")
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }

    let log_tail = fs::read_to_string(&log_path)
        .map(|text| text.chars().rev().take(6000).collect::<String>())
        .unwrap_or_default()
        .chars()
        .rev()
        .collect::<String>();
    Err(anyhow!(
        "timed out waiting for private Notes helper result at {}. Log: {}",
        result_path.display(),
        log_tail
    ))
}

fn postprocess_private_result(op: &str, params: &Value, payload: Value) -> Result<Value> {
    if op != "attachments.save" {
        return Ok(payload);
    }
    let Some(output) = param_string_any(params, &["output", "outputDir", "output_dir"]) else {
        return Ok(payload);
    };
    let Some(source) = payload
        .get("sourcePath")
        .or_else(|| payload.get("path"))
        .and_then(Value::as_str)
    else {
        return Ok(payload);
    };
    let source_path = PathBuf::from(source);
    if !source_path.exists() {
        return Ok(payload);
    }
    let file_name = source_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "attachment".to_string());
    let output_dir = PathBuf::from(output);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let destination = output_dir.join(file_name);
    fs::copy(&source_path, &destination).with_context(|| {
        format!(
            "failed to copy private attachment {} to {}",
            source_path.display(),
            destination.display()
        )
    })?;
    Ok(json!({ "path": destination }))
}

fn prepare_private_params(work_dir: &Path, op: &str, params: &Value) -> Result<Value> {
    if op != "notes.create" && op != "notes.update" {
        return Ok(params.clone());
    }
    let mut prepared = params.clone();
    stage_attachment_array(work_dir, &mut prepared, "attachments")?;
    stage_attachment_array(work_dir, &mut prepared, "attach")?;
    Ok(prepared)
}

fn stage_attachment_array(work_dir: &Path, params: &mut Value, key: &str) -> Result<()> {
    let Some(paths) = params.get(key).and_then(Value::as_array) else {
        return Ok(());
    };
    let staged_dir = work_dir.join("attachments");
    fs::create_dir_all(&staged_dir)
        .with_context(|| format!("failed to create {}", staged_dir.display()))?;
    let mut staged = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().filter_map(Value::as_str).enumerate() {
        let source = PathBuf::from(path);
        let file_name = source
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "attachment".to_string());
        let mut destination = staged_dir.join(&file_name);
        if destination.exists() {
            destination = staged_dir.join(format!("{index}-{file_name}"));
        }
        fs::copy(&source, &destination).with_context(|| {
            format!(
                "failed to stage private Notes attachment {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        staged.push(Value::String(destination.to_string_lossy().to_string()));
    }
    if let Some(object) = params.as_object_mut() {
        object.insert(key.to_string(), Value::Array(staged));
    }
    Ok(())
}

fn compile_injected_helper(source_path: &Path, dylib_path: &Path) -> Result<()> {
    let clang = if Path::new("/usr/bin/clang").exists() {
        PathBuf::from("/usr/bin/clang")
    } else {
        PathBuf::from("clang")
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64e"
    } else {
        "x86_64"
    };
    let output = Command::new(&clang)
        .arg("-arch")
        .arg(arch)
        .arg("-dynamiclib")
        .arg("-fobjc-arc")
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("CoreData")
        .arg("-framework")
        .arg("CloudKit")
        .arg(source_path)
        .arg("-o")
        .arg(dylib_path)
        .output()
        .with_context(|| format!("failed to execute {}", clang.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "failed to compile private Notes helper with {}: {}{}",
            clang.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn notes_private_result_path(file_name: &str) -> Result<PathBuf> {
    let home = env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join("Library/Containers/com.apple.Notes/Data/Library/Application Support/apple-cli")
        .join(file_name))
}

fn uuid_like_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}

fn error_response(
    id: Value,
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
    details: Value,
) -> Value {
    let mut error = json!({
        "code": code.into(),
        "message": message.into(),
        "retryable": retryable
    });
    if !details.is_null() {
        error["details"] = details;
    }
    json!({
        "id": id,
        "ok": false,
        "error": error
    })
}

fn classify_error(error: &anyhow::Error) -> &'static str {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("not found") {
        "not.found"
    } else if message.contains("permission")
        || message.contains("accessibility")
        || message.contains("automation")
        || message.contains("-10827")
    {
        "permission.denied"
    } else if message.contains("timed out") || message.contains("-1712") {
        "notes.timeout"
    } else if message.contains("unsupported") {
        "backend.unsupported"
    } else if message.contains("icloud") || message.contains("share") {
        "icloud.share_failed"
    } else {
        "internal"
    }
}

fn helper_capabilities(backend: &str) -> Result<Value> {
    let sip = sip_status();
    let notes_process_injection = sip.as_deref() == Some("disabled")
        || env::var_os("APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT").is_some();
    let ui_automation = accessibility_enabled();

    Ok(json!({
        "protocolVersion": 1,
        "backend": backend,
        "capabilities": {
            "notesCrud": true,
            "attachments": true,
            "shareCreate": notes_process_injection || ui_automation,
            "shareAccept": notes_process_injection,
            "uiAutomation": ui_automation,
            "notesProcessInjection": notes_process_injection
        },
        "diagnostics": {
            "sip": sip.unwrap_or_else(|| "unknown".to_string()),
            "accessibility": ui_automation
        }
    }))
}

fn sip_status() -> Option<String> {
    let output = Command::new("/usr/bin/csrutil")
        .arg("status")
        .output()
        .ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    if text.contains("system integrity protection status: disabled") {
        Some("disabled".to_string())
    } else if text.contains("system integrity protection status: enabled") {
        Some("enabled".to_string())
    } else {
        Some("unknown".to_string())
    }
}

fn accessibility_enabled() -> bool {
    let mut command = Command::new("/usr/bin/osascript");
    command
        .arg("-e")
        .arg(r#"tell application "System Events" to UI elements enabled"#);
    let output = command_output_with_timeout(command, Duration::from_secs(2));
    output
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim() == "true")
        .unwrap_or(false)
}

fn command_output_with_timeout(mut command: Command, timeout: Duration) -> Option<Output> {
    let start = Instant::now();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    loop {
        if child.try_wait().ok()?.is_some() {
            return child.wait_with_output().ok();
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            return child.wait_with_output().ok();
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn accounts_list() -> Result<Value> {
    let script = r#"
on run argv
    tell application "/System/Applications/Notes.app"
        set rs to character id 30
        set outList to {}
        repeat with a in accounts
            set end of outList to (name of a as string)
        end repeat
        set AppleScript's text item delimiters to rs
        set outText to outList as text
        set AppleScript's text item delimiters to ""
        return outText
    end tell
end run
"#;
    let output = run_applescript(script, &[])?;
    Ok(Value::Array(
        output
            .split(RS)
            .filter(|name| !name.is_empty())
            .map(|name| json!({ "name": name }))
            .collect(),
    ))
}

fn folders_list(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let script = r#"
on run argv
    set accountName to item 1 of argv
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        set fs to character id 31
        set rs to character id 30
        set outList to {}
        repeat with f in folders of targetAccount
            set parentName to ""
            try
                set parentName to (name of container of f as string)
            end try
            set rec to (id of f as string) & fs & (name of f as string) & fs & parentName
            set end of outList to rec
        end repeat
        set AppleScript's text item delimiters to rs
        set outText to outList as text
        set AppleScript's text item delimiters to ""
        return outText
    end tell
end run
"#;
    let output = run_applescript(script, &[account])?;
    Ok(Value::Array(
        parse_records(&output)
            .into_iter()
            .map(|r| {
                json!({
                    "id": field(&r, 0),
                    "name": field(&r, 1),
                    "parent": null_if_empty(field(&r, 2)),
                    "shared": false
                })
            })
            .collect(),
    ))
}

fn folders_create(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let name = required_string(params, "name")?;
    let parent = param_string(params, "parent").unwrap_or_default();
    let script = r#"
on run argv
    set accountName to item 1 of argv
    set folderName to item 2 of argv
    set parentName to item 3 of argv
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        if parentName is "" then
            if not (exists folder folderName of targetAccount) then
                set newFolder to make new folder at targetAccount with properties {name:folderName}
            else
                set newFolder to folder folderName of targetAccount
            end if
        else
            if not (exists folder parentName of targetAccount) then error "Parent folder not found: " & parentName
            tell folder parentName of targetAccount
                if not (exists folder folderName) then
                    set newFolder to make new folder with properties {name:folderName}
                else
                    set newFolder to folder folderName
                end if
            end tell
        end if
        return (id of newFolder as string)
    end tell
end run
"#;
    let id = run_applescript(script, &[account, name.clone(), parent])?;
    Ok(json!({ "id": id, "name": name }))
}

fn folders_rename(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let name = required_string_any(params, &["name", "oldName"])?;
    let new_name = required_string_any(params, &["newName", "new_name"])?;
    let parent = param_string(params, "parent").unwrap_or_default();
    let script = r#"
on run argv
    set accountName to item 1 of argv
    set folderName to item 2 of argv
    set newFolderName to item 3 of argv
    set parentName to item 4 of argv
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        if parentName is "" then
            if not (exists folder folderName of targetAccount) then error "Folder not found: " & folderName
            set targetFolder to folder folderName of targetAccount
        else
            if not (exists folder parentName of targetAccount) then error "Parent folder not found: " & parentName
            if not (exists folder folderName of folder parentName of targetAccount) then error "Folder not found: " & folderName
            set targetFolder to folder folderName of folder parentName of targetAccount
        end if
        set name of targetFolder to newFolderName
        return (id of targetFolder as string)
    end tell
end run
"#;
    let id = run_applescript(script, &[account, name, new_name.clone(), parent])?;
    Ok(json!({ "id": id, "name": new_name }))
}

fn folders_delete(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let name = required_string(params, "name")?;
    let parent = param_string(params, "parent").unwrap_or_default();
    let script = r#"
on run argv
    set accountName to item 1 of argv
    set folderName to item 2 of argv
    set parentName to item 3 of argv
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        if parentName is "" then
            if not (exists folder folderName of targetAccount) then error "Folder not found: " & folderName
            delete folder folderName of targetAccount
        else
            if not (exists folder parentName of targetAccount) then error "Parent folder not found: " & parentName
            if not (exists folder folderName of folder parentName of targetAccount) then error "Folder not found: " & folderName
            delete folder folderName of folder parentName of targetAccount
        end if
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[account, name, parent])?;
    Ok(json!({ "status": "ok" }))
}

fn notes_list(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let folder = param_string(params, "folder").unwrap_or_default();
    let shared_only = param_bool(params, "sharedOnly").unwrap_or(false)
        || param_bool(params, "shared_only").unwrap_or(false);
    let script = r#"
on run argv
    set accountName to item 1 of argv
    set folderName to item 2 of argv
    set sharedOnlyText to item 3 of argv
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        if folderName is "" then
            set targetNotes to every note of targetAccount
        else
            if not (exists folder folderName of targetAccount) then error "Folder not found: " & folderName
            set targetNotes to every note of folder folderName of targetAccount
        end if
        set fs to character id 31
        set rs to character id 30
        set outList to {}
        repeat with n in targetNotes
            set includeNote to true
            if sharedOnlyText is "true" then
                set includeNote to false
                try
                    if (shared of n as boolean) is true then set includeNote to true
                end try
            end if
            if includeNote is true then
                set folderNameOut to ""
                try
                    set folderNameOut to (name of container of n as string)
                end try
                set createdText to ""
                try
                    set createdText to (creation date of n as string)
                end try
                set modifiedText to ""
                try
                    set modifiedText to (modification date of n as string)
                end try
                set protectedText to ""
                try
                    set protectedText to (password protected of n as string)
                end try
                set sharedText to ""
                try
                    set sharedText to (shared of n as string)
                end try
                set rec to (id of n as string) & fs & (name of n as string) & fs & folderNameOut & fs & createdText & fs & modifiedText & fs & protectedText & fs & sharedText
                set end of outList to rec
            end if
        end repeat
        set AppleScript's text item delimiters to rs
        set outText to outList as text
        set AppleScript's text item delimiters to ""
        return outText
    end tell
end run
"#;
    let output = run_applescript(script, &[account, folder, shared_only.to_string()])?;
    Ok(Value::Array(
        parse_records(&output)
            .into_iter()
            .map(note_summary_from_record)
            .collect(),
    ))
}

fn notes_get(params: &Value) -> Result<Value> {
    let id = required_string(params, "id")?;
    let script = r#"
on run argv
    set noteId to item 1 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        set n to note id noteId
        set fs to character id 31
        set folderName to ""
        try
            set folderName to (name of container of n as string)
        end try
        set createdText to ""
        try
            set createdText to (creation date of n as string)
        end try
        set modifiedText to ""
        try
            set modifiedText to (modification date of n as string)
        end try
        set protectedText to ""
        try
            set protectedText to (password protected of n as string)
        end try
        set sharedText to ""
        try
            set sharedText to (shared of n as string)
        end try
        set notePlainText to ""
        try
            set notePlainText to (plaintext of n as string)
        end try
        return (id of n as string) & fs & (name of n as string) & fs & folderName & fs & (body of n as string) & fs & notePlainText & fs & createdText & fs & modifiedText & fs & protectedText & fs & sharedText
    end tell
end run
"#;
    let output = run_applescript(script, &[id])?;
    let fields: Vec<String> = output.split(FS).map(|f| f.to_string()).collect();
    Ok(note_detail_from_fields(&fields))
}

fn notes_create(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let folder = param_string(params, "folder").unwrap_or_else(|| "Notes".to_string());
    let title =
        param_string_any(params, &["title", "name"]).unwrap_or_else(|| "Untitled".to_string());
    let html = required_string_any(params, &["html", "body"])?;
    let attach_blob = param_string_array(params, "attachments")
        .or_else(|| param_string_array(params, "attach"))
        .unwrap_or_default()
        .join("\n");
    let script = r#"
on run argv
    set accountName to item 1 of argv
    set folderName to item 2 of argv
    set noteName to item 3 of argv
    set noteBody to item 4 of argv
    set attachText to item 5 of argv
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        if not (exists folder folderName of targetAccount) then error "Folder not found: " & folderName
        set newNote to make new note at folder folderName of targetAccount with properties {name:noteName, body:noteBody}
        if attachText is not "" then
            set AppleScript's text item delimiters to linefeed
            set fileList to text items of attachText
            set AppleScript's text item delimiters to ""
            repeat with fp in fileList
                if fp is not "" then
                    set attachmentFile to (fp as string) as POSIX file
                    make new attachment at end of attachments of newNote with data attachmentFile
                end if
            end repeat
        end if
        return (id of newNote as string)
    end tell
end run
"#;
    let id = run_applescript(script, &[account, folder, title.clone(), html, attach_blob])?;
    Ok(json!({ "id": id, "title": title }))
}

fn notes_update(params: &Value) -> Result<Value> {
    let id = required_string(params, "id")?;
    let title = param_string_any(params, &["title", "name"]).unwrap_or_default();
    let html = param_string_any(params, &["html", "body"]).unwrap_or_default();
    let attach_blob = param_string_array(params, "attachments")
        .or_else(|| param_string_array(params, "attach"))
        .unwrap_or_default()
        .join("\n");
    let script = r#"
on run argv
    set noteId to item 1 of argv
    set noteName to item 2 of argv
    set noteBody to item 3 of argv
    set attachText to item 4 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        set n to note id noteId
        if noteName is not "" then set name of n to noteName
        if noteBody is not "" then set body of n to noteBody
        if attachText is not "" then
            set AppleScript's text item delimiters to linefeed
            set fileList to text items of attachText
            set AppleScript's text item delimiters to ""
            repeat with fp in fileList
                if fp is not "" then
                    set attachmentFile to (fp as string) as POSIX file
                    make new attachment at end of attachments of n with data attachmentFile
                end if
            end repeat
        end if
        return (id of n as string)
    end tell
end run
"#;
    let id = run_applescript(script, &[id, title, html, attach_blob])?;
    Ok(json!({ "id": id }))
}

fn notes_delete(params: &Value) -> Result<Value> {
    let id = required_string(params, "id")?;
    let script = r#"
on run argv
    set noteId to item 1 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        delete note id noteId
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[id])?;
    Ok(json!({ "status": "ok" }))
}

fn notes_move(params: &Value) -> Result<Value> {
    let id = required_string(params, "id")?;
    let account = param_string(params, "account").unwrap_or_default();
    let folder = required_string(params, "folder")?;
    let script = r#"
on run argv
    set noteId to item 1 of argv
    set accountName to item 2 of argv
    set folderName to item 3 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        if not (exists folder folderName of targetAccount) then error "Folder not found: " & folderName
        move note id noteId to folder folderName of targetAccount
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[id, account, folder.clone()])?;
    Ok(json!({ "status": "ok", "folder": folder }))
}

fn notes_search(params: &Value) -> Result<Value> {
    let account = param_string(params, "account").unwrap_or_default();
    let query = required_string(params, "query")?;
    let limit = param_usize(params, "limit").unwrap_or(0);
    let script = r#"
on run argv
    set accountName to item 1 of argv
    set queryText to item 2 of argv
    set limitText to item 3 of argv
    if limitText is "" then
        set maxCount to 0
    else
        set maxCount to limitText as integer
    end if
    tell application "/System/Applications/Notes.app"
        if accountName is "" then
            set targetAccount to account 1
        else
            if not (exists account accountName) then error "Account not found: " & accountName
            set targetAccount to account accountName
        end if
        set matches to (every note of targetAccount whose name contains queryText or body contains queryText)
        set fs to character id 31
        set rs to character id 30
        set outList to {}
        repeat with n in matches
            set folderName to ""
            try
                set folderName to (name of container of n as string)
            end try
            set sharedText to ""
            try
                set sharedText to (shared of n as string)
            end try
            set rec to (id of n as string) & fs & (name of n as string) & fs & folderName & fs & sharedText
            set end of outList to rec
            if maxCount is not 0 then
                if (count of outList) >= maxCount then exit repeat
            end if
        end repeat
        set AppleScript's text item delimiters to rs
        set outText to outList as text
        set AppleScript's text item delimiters to ""
        return outText
    end tell
end run
"#;
    let output = run_applescript(script, &[account, query, limit.to_string()])?;
    Ok(Value::Array(
        parse_records(&output)
            .into_iter()
            .map(|r| {
                json!({
                    "id": field(&r, 0),
                    "title": field(&r, 1),
                    "folder": field(&r, 2),
                    "shared": parse_bool_field(&field(&r, 3))
                })
            })
            .collect(),
    ))
}

fn notes_show(params: &Value) -> Result<Value> {
    let id = required_string(params, "id")?;
    let script = r#"
on run argv
    set noteId to item 1 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        show note id noteId
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[id])?;
    Ok(json!({ "status": "ok" }))
}

fn attachments_list(params: &Value) -> Result<Value> {
    let id = required_string_any(params, &["noteId", "id"])?;
    let script = r#"
on run argv
    set noteId to item 1 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        set n to note id noteId
        set fs to character id 31
        set rs to character id 30
        set outList to {}
        repeat with a in attachments of n
            set cidText to ""
            try
                set cidText to (content identifier of a as string)
            end try
            set createdText to ""
            try
                set createdText to (creation date of a as string)
            end try
            set modifiedText to ""
            try
                set modifiedText to (modification date of a as string)
            end try
            set urlText to ""
            try
                set urlText to (URL of a as string)
            end try
            set sharedText to ""
            try
                set sharedText to (shared of a as string)
            end try
            set rec to (id of a as string) & fs & (name of a as string) & fs & cidText & fs & createdText & fs & modifiedText & fs & urlText & fs & sharedText
            set end of outList to rec
        end repeat
        set AppleScript's text item delimiters to rs
        set outText to outList as text
        set AppleScript's text item delimiters to ""
        return outText
    end tell
end run
"#;
    let output = run_applescript(script, &[id])?;
    Ok(Value::Array(
        parse_records(&output)
            .into_iter()
            .map(|r| {
                json!({
                    "id": field(&r, 0),
                    "name": field(&r, 1),
                    "contentIdentifier": field(&r, 2),
                    "createdAt": field(&r, 3),
                    "modifiedAt": field(&r, 4),
                    "url": null_if_empty(field(&r, 5)),
                    "shared": parse_bool_field(&field(&r, 6))
                })
            })
            .collect(),
    ))
}

fn attachments_save(params: &Value) -> Result<Value> {
    let note_id = required_string_any(params, &["noteId", "id"])?;
    let attachment_id =
        param_string_any(params, &["attachmentId", "attachment_id"]).unwrap_or_default();
    let name = param_string(params, "name").unwrap_or_default();
    let output = required_string_any(params, &["output", "outputDir", "output_dir"])?;
    if attachment_id.is_empty() && name.is_empty() {
        return Err(anyhow!("provide attachmentId or name"));
    }
    let script = r#"
on run argv
    set noteId to item 1 of argv
    set attId to item 2 of argv
    set attName to item 3 of argv
    set outDir to item 4 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        set n to note id noteId
        set target to missing value
        if attId is not "" then
            try
                set target to first attachment of n whose id is attId
            end try
        end if
        if target is missing value and attName is not "" then
            try
                set target to first attachment of n whose name is attName
            end try
        end if
        if target is missing value then error "Attachment not found"
        set outDirAlias to POSIX file outDir as alias
        set outDirPosix to POSIX path of outDirAlias
        set outFilePosix to outDirPosix & (name of target as string)
        set outFileHfs to (outDirAlias as text) & (name of target as string)
        save target in file outFileHfs
        return outFilePosix
    end tell
end run
"#;
    let path = run_applescript(script, &[note_id, attachment_id, name, output])?;
    Ok(json!({ "path": path }))
}

fn attachments_delete(params: &Value) -> Result<Value> {
    let note_id = required_string_any(params, &["noteId", "id"])?;
    let attachment_id =
        param_string_any(params, &["attachmentId", "attachment_id"]).unwrap_or_default();
    let name = param_string(params, "name").unwrap_or_default();
    if attachment_id.is_empty() && name.is_empty() {
        return Err(anyhow!("provide attachmentId or name"));
    }
    let script = r#"
on run argv
    set noteId to item 1 of argv
    set attId to item 2 of argv
    set attName to item 3 of argv
    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        set n to note id noteId
        set target to missing value
        if attId is not "" then
            try
                set target to first attachment of n whose id is attId
            end try
        end if
        if target is missing value and attName is not "" then
            try
                set target to first attachment of n whose name is attName
            end try
        end if
        if target is missing value then error "Attachment not found"
        delete target
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[note_id, attachment_id, name])?;
    Ok(json!({ "status": "ok" }))
}

fn shares_create(params: &Value, backend: &str) -> Result<Value> {
    let note_id = required_string_any(params, &["noteId", "id"])?;
    let invitee = required_string_any(params, &["invitee", "email"])?;
    let timeout = param_usize(params, "timeout").unwrap_or(60).to_string();
    let share_backend = param_string(params, "backend").unwrap_or_else(|| backend.to_string());
    let effective_backend = if share_backend == "applescript" {
        "ui".to_string()
    } else {
        share_backend
    };
    run_apple_json(&[
        "notes",
        "share",
        &note_id,
        "--email",
        &invitee,
        "--backend",
        &effective_backend,
        "--timeout",
        &timeout,
    ])
}

fn shares_accept(params: &Value) -> Result<Value> {
    let url = required_string(params, "url")?;
    let timeout = param_usize(params, "timeout").unwrap_or(90).to_string();
    run_apple_json(&[
        "notes",
        "shared",
        "accept",
        "--url",
        &url,
        "--timeout",
        &timeout,
    ])
}

fn run_apple_json(args: &[&str]) -> Result<Value> {
    let apple = sibling_binary("apple");
    let output = Command::new(&apple)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {}", apple.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() { stdout } else { stderr };
        return Err(anyhow!(message));
    }
    serde_json::from_str(&stdout)
        .with_context(|| format!("apple command did not return JSON: {stdout}"))
}

fn sibling_binary(name: &str) -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(name)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn note_summary_from_record(r: Vec<String>) -> Value {
    json!({
        "id": field(&r, 0),
        "title": field(&r, 1),
        "name": field(&r, 1),
        "folder": field(&r, 2),
        "createdAt": field(&r, 3),
        "modifiedAt": field(&r, 4),
        "passwordProtected": parse_bool_field(&field(&r, 5)),
        "shared": parse_bool_field(&field(&r, 6))
    })
}

fn note_detail_from_fields(fields: &[String]) -> Value {
    json!({
        "id": field(fields, 0),
        "title": field(fields, 1),
        "name": field(fields, 1),
        "folder": field(fields, 2),
        "html": field(fields, 3),
        "body": field(fields, 3),
        "plaintext": field(fields, 4),
        "createdAt": field(fields, 5),
        "modifiedAt": field(fields, 6),
        "passwordProtected": parse_bool_field(&field(fields, 7)),
        "shared": parse_bool_field(&field(fields, 8))
    })
}

fn field(fields: &[String], index: usize) -> String {
    fields.get(index).cloned().unwrap_or_default()
}

fn null_if_empty(value: String) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value)
    }
}

fn parse_bool_field(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

fn required_string(params: &Value, key: &str) -> Result<String> {
    param_string(params, key).ok_or_else(|| anyhow!("missing required param: {key}"))
}

fn required_string_any(params: &Value, keys: &[&str]) -> Result<String> {
    param_string_any(params, keys)
        .ok_or_else(|| anyhow!("missing required param: {}", keys.join("|")))
}

fn param_string(params: &Value, key: &str) -> Option<String> {
    params.get(key)?.as_str().map(ToString::to_string)
}

fn param_string_any(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| param_string(params, key))
}

fn param_bool(params: &Value, key: &str) -> Option<bool> {
    params.get(key)?.as_bool()
}

fn param_usize(params: &Value, key: &str) -> Option<usize> {
    params
        .get(key)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

fn param_string_array(params: &Value, key: &str) -> Option<Vec<String>> {
    params.get(key)?.as_array().map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(ToString::to_string))
            .collect()
    })
}
