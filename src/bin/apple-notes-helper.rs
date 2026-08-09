use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::Deserialize;
use serde_json::{json, Value};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const INJECTED_PRIVATE_HELPER_SOURCE: &str =
    include_str!("../../helpers/notes-private-injected/AppleNotesPrivateInjected.m");
const INJECTED_SHARE_HELPER_SOURCE: &str =
    include_str!("../../helpers/notes-share-injected/AppleNotesShareInjected.m");
const INJECTED_ACCEPT_HELPER_SOURCE: &str =
    include_str!("../../helpers/notes-accept-injected/AppleNotesAcceptInjected.m");

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
    /// Backend preference: private, or auto as an alias for private
    #[arg(long, default_value = "private")]
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
    validate_backend(backend)?;
    match op {
        "helper.ping" => Ok(json!({ "pong": true })),
        "helper.shutdown" => Ok(json!({ "shutdown": true })),
        "helper.capabilities" => helper_capabilities(backend),
        "shared.list" => {
            let mut private_params = if params.is_object() {
                params.clone()
            } else {
                json!({})
            };
            if let Some(object) = private_params.as_object_mut() {
                object.insert("sharedOnly".to_string(), Value::Bool(true));
            }
            run_private_operation("notes.list", &private_params)
        }
        "shared.get" => run_private_operation("notes.get", params),
        "shares.create" => run_private_share(params),
        "shares.accept" => run_private_accept(params),
        private_op if should_try_private_operation(private_op, backend) => {
            run_private_operation(private_op, params)
        }
        other => Err(anyhow!("unsupported operation: {other}")),
    }
}

fn should_try_private_operation(op: &str, backend: &str) -> bool {
    if validate_backend(backend).is_err() {
        return false;
    }
    matches!(
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
            | "notes.show"
            | "shared.list"
            | "shared.get"
            | "attachments.list"
            | "attachments.save"
            | "attachments.delete"
            | "shares.create"
            | "shares.accept"
    )
}

fn validate_backend(backend: &str) -> Result<()> {
    match backend {
        "auto" | "private" => Ok(()),
        "applescript" | "ui" => Err(anyhow!(
            "Apple Notes operations are private-helper only; backend {backend:?} is no longer supported"
        )),
        other => Err(anyhow!("unsupported Notes helper backend: {other}")),
    }
}

fn preflight_private_notes_injection() -> Result<()> {
    if private_notes_injection_available() {
        return Ok(());
    }
    Err(anyhow!(
        "private Notes helper requires DYLD library injection into Notes, but SIP appears to be enabled. Run on a lab Mac with SIP/library injection relaxed, or set APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1 to bypass this preflight."
    ))
}

fn private_notes_injection_available() -> bool {
    env::var_os("APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT").is_some()
        || sip_status().as_deref() == Some("disabled")
}

fn run_private_operation(op: &str, params: &Value) -> Result<Value> {
    preflight_private_notes_injection()?;
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

fn run_private_share(params: &Value) -> Result<Value> {
    if let Some(backend) = param_string(params, "backend") {
        validate_backend(&backend)?;
    }
    let note_id = required_string_any(params, &["noteId", "id"])?;
    let invitee = required_string_any(params, &["invitee", "email"])?;
    let timeout = param_usize(params, "timeout").unwrap_or(120).max(1) as u64;
    run_injected_sidecar(
        "share",
        INJECTED_SHARE_HELPER_SOURCE,
        "AppleNotesShareInjected.m",
        "libAppleNotesShareInjected.dylib",
        "notes-share-result",
        &[
            ("APPLE_CLI_NOTES_SHARE_NOTE_ID", note_id),
            ("APPLE_CLI_NOTES_SHARE_EMAIL", invitee),
            ("APPLE_CLI_NOTES_SHARE_TIMEOUT", timeout.to_string()),
        ],
        Duration::from_secs(timeout.saturating_add(30)),
    )
}

fn run_private_accept(params: &Value) -> Result<Value> {
    let url = required_string(params, "url")?;
    let timeout = param_usize(params, "timeout").unwrap_or(120).max(1) as u64;
    run_injected_sidecar(
        "accept",
        INJECTED_ACCEPT_HELPER_SOURCE,
        "AppleNotesAcceptInjected.m",
        "libAppleNotesAcceptInjected.dylib",
        "notes-accept-result",
        &[
            ("APPLE_CLI_NOTES_ACCEPT_URL", url),
            ("APPLE_CLI_NOTES_ACCEPT_TIMEOUT", timeout.to_string()),
        ],
        Duration::from_secs(timeout.saturating_add(30)),
    )
}

fn run_injected_sidecar(
    label: &str,
    source: &str,
    source_file_name: &str,
    dylib_file_name: &str,
    result_prefix: &str,
    envs: &[(&str, String)],
    timeout: Duration,
) -> Result<Value> {
    preflight_private_notes_injection()?;
    let work_dir = PathBuf::from("/tmp").join(format!(
        "apple-cli-notes-{label}-{}-{}",
        std::process::id(),
        uuid_like_timestamp()
    ));
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    let source_path = work_dir.join(source_file_name);
    let dylib_path = work_dir.join(dylib_file_name);
    let log_path = work_dir.join(format!("notes-{label}.log"));
    fs::write(&source_path, source)
        .with_context(|| format!("failed to write {}", source_path.display()))?;
    compile_injected_helper(&source_path, &dylib_path)?;

    let result_path = notes_private_result_path(&format!(
        "{result_prefix}-{}-{}.json",
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
    let mut command = Command::new("/System/Applications/Notes.app/Contents/MacOS/Notes");
    command
        .env("DYLD_INSERT_LIBRARIES", &dylib_path)
        .env(
            match label {
                "share" => "APPLE_CLI_NOTES_SHARE_RESULT",
                "accept" => "APPLE_CLI_NOTES_ACCEPT_RESULT",
                _ => "APPLE_CLI_NOTES_PRIVATE_RESULT",
            },
            &result_path,
        )
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file));
    for (key, value) in envs {
        command.env(key, value);
    }
    command.spawn().with_context(|| {
        format!("failed to relaunch Notes with injected private {label} helper")
    })?;

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if result_path.exists() {
            let result_text = fs::read_to_string(&result_path)
                .with_context(|| format!("failed to read {}", result_path.display()))?;
            let mut result: Value = serde_json::from_str(&result_text)
                .with_context(|| format!("failed to parse {}", result_path.display()))?;
            if result.get("status").and_then(Value::as_str) == Some("ok") {
                if let Some(object) = result.as_object_mut() {
                    object.insert("backend".to_string(), json!("private"));
                    object.insert(
                        "log_path".to_string(),
                        json!(log_path.display().to_string()),
                    );
                    object.insert(
                        "helper_dylib".to_string(),
                        json!(dylib_path.display().to_string()),
                    );
                }
                return Ok(result);
            }
            return Err(anyhow!(
                "{}",
                result
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("private Notes sidecar helper returned an error")
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
        "timed out waiting for private Notes {label} helper result at {}. Log: {}",
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
        .arg("-framework")
        .arg("AppKit")
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

    Ok(json!({
        "protocolVersion": 1,
        "backend": if backend == "auto" { "private" } else { backend },
        "capabilities": {
            "notesCrud": true,
            "attachments": true,
            "shareCreate": notes_process_injection,
            "shareAccept": notes_process_injection,
            "uiAutomation": false,
            "notesProcessInjection": notes_process_injection
        },
        "diagnostics": {
            "sip": sip.unwrap_or_else(|| "unknown".to_string()),
            "accessibility": Value::Null
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

fn param_usize(params: &Value, key: &str) -> Option<usize> {
    params
        .get(key)?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_notes_protocol_operation_is_private_supported() {
        let operations = [
            "accounts.list",
            "folders.list",
            "folders.create",
            "folders.rename",
            "folders.delete",
            "notes.list",
            "notes.get",
            "notes.create",
            "notes.update",
            "notes.delete",
            "notes.move",
            "notes.search",
            "notes.show",
            "shared.list",
            "shared.get",
            "attachments.list",
            "attachments.save",
            "attachments.delete",
            "shares.create",
            "shares.accept",
        ];

        for operation in operations {
            assert!(
                should_try_private_operation(operation, "private"),
                "{operation} should be handled by the private Notes helper"
            );
        }
    }

    #[test]
    fn applescript_and_ui_backend_names_are_rejected() {
        assert!(validate_backend("private").is_ok());
        assert!(validate_backend("auto").is_ok());
        assert!(validate_backend("applescript").is_err());
        assert!(validate_backend("ui").is_err());
    }
}
