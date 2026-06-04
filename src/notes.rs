use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::common::{parse_records, run_applescript, FS, RS};
use crate::{
    FoldersCreateArgs, FoldersDeleteArgs, FoldersListArgs, NotesAttachmentsDeleteArgs,
    NotesAttachmentsListArgs, NotesAttachmentsSaveArgs, NotesCreateArgs, NotesDeleteArgs,
    NotesGetArgs, NotesListArgs, NotesMoveArgs, NotesSearchArgs, NotesShareArgs,
    NotesSharedAcceptArgs, NotesSharedGetArgs, NotesSharedListArgs, NotesShowArgs, NotesUpdateArgs,
};

const INJECTED_SHARE_HELPER_SOURCE: &str =
    include_str!("../helpers/notes-share-injected/AppleNotesShareInjected.m");
const INJECTED_ACCEPT_HELPER_SOURCE: &str =
    include_str!("../helpers/notes-accept-injected/AppleNotesAcceptInjected.m");

pub fn accounts_list() -> Result<()> {
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
    let items: Vec<_> = output
        .split(RS)
        .filter(|s| !s.is_empty())
        .map(|name| json!({ "name": name }))
        .collect();
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

pub fn folders_list(args: FoldersListArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
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
            set rec to (id of f as string) & fs & (name of f as string)
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
    let records = parse_records(&output);
    let items: Vec<_> = records
        .into_iter()
        .map(|r| {
            let id = r.get(0).cloned().unwrap_or_default();
            let name = r.get(1).cloned().unwrap_or_default();
            json!({ "id": id, "name": name })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

pub fn folders_create(args: FoldersCreateArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let name = args.name;
    let parent = args.parent.unwrap_or_default();
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
                make new folder at targetAccount with properties {name:folderName}
            end if
        else
            if not (exists folder parentName of targetAccount) then error "Parent folder not found: " & parentName
            tell folder parentName of targetAccount
                if not (exists folder folderName) then
                    make new folder with properties {name:folderName}
                end if
            end tell
        end if
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[account, name, parent])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "status": "OK" }))?
    );
    Ok(())
}

pub fn folders_delete(args: FoldersDeleteArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let name = args.name;
    let parent = args.parent.unwrap_or_default();
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
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "status": "OK" }))?
    );
    Ok(())
}

pub fn notes_list(args: NotesListArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let folder = args.folder.unwrap_or_default();
    let items = notes_list_items(account, folder, false)?;
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

pub fn notes_shared_list(args: NotesSharedListArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let folder = args.folder.unwrap_or_default();
    let items = notes_list_items(account, folder, true)?;
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

pub fn notes_shared_get(args: NotesSharedGetArgs) -> Result<()> {
    notes_get(NotesGetArgs { id: args.id })
}

pub fn notes_shared_accept(args: NotesSharedAcceptArgs) -> Result<()> {
    preflight_private_notes_helper()?;

    let work_dir =
        std::env::temp_dir().join(format!("apple-cli-notes-accept-{}", std::process::id()));
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    let source_path = work_dir.join("AppleNotesAcceptInjected.m");
    let dylib_path = work_dir.join("libAppleNotesAcceptInjected.dylib");
    let log_path = work_dir.join("notes-accept.log");
    fs::write(&source_path, INJECTED_ACCEPT_HELPER_SOURCE)
        .with_context(|| format!("failed to write {}", source_path.display()))?;

    compile_injected_helper(&source_path, &dylib_path)?;

    let result_path = notes_helper_result_path("notes-accept-result.json")?;
    let _ = fs::remove_file(&result_path);
    let _ = fs::remove_file(&log_path);

    let _ = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "/System/Applications/Notes.app" to quit"#)
        .status();
    thread::sleep(Duration::from_secs(2));

    let log_file = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;
    Command::new("/System/Applications/Notes.app/Contents/MacOS/Notes")
        .env("DYLD_INSERT_LIBRARIES", &dylib_path)
        .env("APPLE_CLI_NOTES_ACCEPT_URL", &args.url)
        .env("APPLE_CLI_NOTES_ACCEPT_RESULT", &result_path)
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .with_context(|| {
            "failed to relaunch Notes with the injected private accept helper".to_string()
        })?;

    let deadline = Instant::now() + Duration::from_secs(args.timeout);
    while Instant::now() < deadline {
        if result_path.exists() {
            let result_text = fs::read_to_string(&result_path)
                .with_context(|| format!("failed to read {}", result_path.display()))?;
            let mut result: Value = serde_json::from_str(&result_text)
                .with_context(|| format!("failed to parse {}", result_path.display()))?;
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
            println!("{}", serde_json::to_string_pretty(&result)?);
            if result.get("status").and_then(Value::as_str) == Some("ok") {
                return Ok(());
            }
            return Err(anyhow!(
                "private Notes accept helper returned an error at {}",
                result_path.display()
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }

    let log_tail = read_tail(&log_path, 6000).unwrap_or_default();
    Err(anyhow!(
        "timed out waiting for private Notes accept helper result at {}. Log: {}",
        result_path.display(),
        log_tail
    ))
}

fn notes_list_items(account: String, folder: String, shared_only: bool) -> Result<Vec<Value>> {
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
            set rec to (id of n as string) & fs & (name of n as string) & fs & folderName & fs & createdText & fs & modifiedText & fs & protectedText & fs & sharedText
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
    let records = parse_records(&output);
    let items = records
        .into_iter()
        .map(|r| {
            let id = r.get(0).cloned().unwrap_or_default();
            let name = r.get(1).cloned().unwrap_or_default();
            let folder = r.get(2).cloned().unwrap_or_default();
            let created_at = r.get(3).cloned().unwrap_or_default();
            let modified_at = r.get(4).cloned().unwrap_or_default();
            let password_protected = r.get(5).cloned().unwrap_or_default();
            let shared = r.get(6).cloned().unwrap_or_default();
            json!({
                "id": id,
                "name": name,
                "folder": folder,
                "created_at": created_at,
                "modified_at": modified_at,
                "password_protected": password_protected,
                "shared": shared
            })
        })
        .collect();
    Ok(items)
}

pub fn notes_get(args: NotesGetArgs) -> Result<()> {
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
    let output = run_applescript(script, &[args.id])?;
    let fields: Vec<String> = output.split(FS).map(|f| f.to_string()).collect();
    let id = fields.get(0).cloned().unwrap_or_default();
    let name = fields.get(1).cloned().unwrap_or_default();
    let folder = fields.get(2).cloned().unwrap_or_default();
    let body = fields.get(3).cloned().unwrap_or_default();
    let plaintext = fields.get(4).cloned().unwrap_or_default();
    let created_at = fields.get(5).cloned().unwrap_or_default();
    let modified_at = fields.get(6).cloned().unwrap_or_default();
    let password_protected = fields.get(7).cloned().unwrap_or_default();
    let shared = fields.get(8).cloned().unwrap_or_default();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "id": id,
            "name": name,
            "folder": folder,
            "body": body,
            "plaintext": plaintext,
            "created_at": created_at,
            "modified_at": modified_at,
            "password_protected": password_protected,
            "shared": shared
        }))?
    );
    Ok(())
}

pub fn notes_create(args: NotesCreateArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let folder = args.folder.unwrap_or_else(|| "Notes".to_string());
    let name = args.name.unwrap_or_else(|| "Untitled".to_string());
    let body = args.body;
    let attach = args.attach;
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
                    make new attachment at end of attachments of newNote with data (POSIX file fp)
                end if
            end repeat
        end if
        return (id of newNote as string)
    end tell
end run
"#;
    let attach_blob = if attach.is_empty() {
        "".to_string()
    } else {
        attach.join("\n")
    };
    let output = run_applescript(script, &[account, folder, name, body, attach_blob])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "id": output }))?
    );
    Ok(())
}

pub fn notes_update(args: NotesUpdateArgs) -> Result<()> {
    let name = args.name.unwrap_or_default();
    let body = args.body.unwrap_or_default();
    let attach = args.attach;
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
                    make new attachment at end of attachments of n with data (POSIX file fp)
                end if
            end repeat
        end if
        return (id of n as string)
    end tell
end run
"#;
    let attach_blob = if attach.is_empty() {
        "".to_string()
    } else {
        attach.join("\n")
    };
    let output = run_applescript(script, &[args.id, name, body, attach_blob])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "id": output }))?
    );
    Ok(())
}

pub fn notes_delete(args: NotesDeleteArgs) -> Result<()> {
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
    let _ = run_applescript(script, &[args.id])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "status": "OK" }))?
    );
    Ok(())
}

pub fn notes_move(args: NotesMoveArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let folder = args.folder;
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
    let _ = run_applescript(script, &[args.id, account, folder])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "status": "OK" }))?
    );
    Ok(())
}

pub fn notes_share(args: NotesShareArgs) -> Result<()> {
    match args.backend.as_str() {
        "private" => return notes_share_private(args),
        "ui" => return notes_share_ui(args),
        "auto" => {
            if private_notes_helper_available() {
                return notes_share_private(args);
            }
            return notes_share_ui(args);
        }
        other => return Err(anyhow!("unsupported notes share backend: {other}")),
    }
}

fn notes_share_ui(args: NotesShareArgs) -> Result<()> {
    let script = r#"
on clickFirstNamed(processName, namesToTry, timeoutSeconds)
    set deadline to (current date) + timeoutSeconds
    repeat while (current date) is less than deadline
        tell application "System Events"
            tell process processName
                set allElements to entire contents
                repeat with targetName in namesToTry
                    repeat with uiElement in allElements
                        try
                            if (name of uiElement as text) is (targetName as text) then
                                click uiElement
                                return targetName as text
                            end if
                        end try
                        try
                            if (description of uiElement as text) is (targetName as text) then
                                click uiElement
                                return targetName as text
                            end if
                        end try
                    end repeat
                end repeat
            end tell
        end tell
        delay 0.5
    end repeat
    error "Timed out waiting for Notes UI element: " & (namesToTry as text)
end clickFirstNamed

on clickShareMenu(timeoutSeconds)
    tell application "System Events"
        if UI elements enabled is false then error "Accessibility access is disabled. Enable it for the terminal/Codex app in System Settings -> Privacy & Security -> Accessibility."
        tell process "Notes"
            set frontmost to true
            try
                click menu item "Share in iCloud" of menu "File" of menu bar 1
                return "Share in iCloud"
            end try
            try
                click menu item "Share Note" of menu "File" of menu bar 1
                return "Share Note"
            end try
            try
                click menu item "Manage Shared Note" of menu "File" of menu bar 1
                return "Manage Shared Note"
            end try
        end tell
    end tell
    return my clickFirstNamed("Notes", {"Share Note", "Share in iCloud", "Manage Shared Note", "Share"}, timeoutSeconds)
end clickShareMenu

on serviceNames(serviceName)
    if serviceName is "copy-link" then return {"Copy Link"}
    if serviceName is "messages" then return {"Messages"}
    if serviceName is "mail" then return {"Mail"}
    return {serviceName}
end serviceNames

on submitInvitee(invitee, timeoutSeconds)
    tell application "System Events"
        set oldClipboard to the clipboard
        set the clipboard to invitee
        keystroke "v" using command down
        key code 36
    end tell
    delay 1
    try
        set clickedButton to my clickFirstNamed("Notes", {"Share", "Copy Link", "Send", "Invite", "Continue", "Done"}, timeoutSeconds)
    on error
        set clickedButton to ""
    end try
    delay 2
    tell application "System Events"
        set newClipboard to the clipboard
        try
            if oldClipboard is not invitee then set the clipboard to newClipboard
        end try
    end tell
    return clickedButton & (character id 31) & newClipboard
end submitInvitee

on run argv
    set noteId to item 1 of argv
    set invitee to item 2 of argv
    set serviceName to item 3 of argv
    set timeoutSeconds to item 4 of argv as integer
    set openOnlyText to item 5 of argv
    set fs to character id 31

    tell application "System Events"
        if UI elements enabled is false then error "Accessibility access is disabled. Enable it for the terminal/Codex app in System Settings -> Privacy & Security -> Accessibility."
    end tell

    tell application "/System/Applications/Notes.app"
        if not (exists note id noteId) then error "Note not found: " & noteId
        show note id noteId
        activate
    end tell
    delay 1

    set openedBy to my clickShareMenu(timeoutSeconds)
    if openOnlyText is "true" then return "OPENED" & fs & openedBy & fs & ""

    set selectedService to my clickFirstNamed("Notes", my serviceNames(serviceName), timeoutSeconds)
    delay 1
    set submitResult to my submitInvitee(invitee, timeoutSeconds)
    return "OK" & fs & selectedService & fs & submitResult
end run
"#;
    let output = run_applescript(
        script,
        &[
            args.id,
            args.email,
            args.service,
            args.timeout.to_string(),
            args.open_only.to_string(),
        ],
    )?;
    let fields: Vec<String> = output.split(FS).map(|f| f.to_string()).collect();
    let status = fields.get(0).cloned().unwrap_or_default();
    let service = fields.get(1).cloned().unwrap_or_default();
    let action = fields.get(2).cloned().unwrap_or_default();
    let clipboard = fields.get(3).cloned().unwrap_or_default();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": status,
            "service": service,
            "action": action,
            "clipboard": clipboard
        }))?
    );
    Ok(())
}

fn notes_share_private(args: NotesShareArgs) -> Result<()> {
    if args.open_only {
        return Err(anyhow!("--open-only is only supported by --backend ui"));
    }
    preflight_private_notes_helper()?;

    let work_dir =
        std::env::temp_dir().join(format!("apple-cli-notes-share-{}", std::process::id()));
    fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create {}", work_dir.display()))?;

    let source_path = work_dir.join("AppleNotesShareInjected.m");
    let dylib_path = work_dir.join("libAppleNotesShareInjected.dylib");
    let log_path = work_dir.join("notes-share.log");
    fs::write(&source_path, INJECTED_SHARE_HELPER_SOURCE)
        .with_context(|| format!("failed to write {}", source_path.display()))?;

    compile_injected_helper(&source_path, &dylib_path)?;

    let result_path = notes_helper_result_path("notes-share-result.json")?;
    let _ = fs::remove_file(&result_path);
    let _ = fs::remove_file(&log_path);

    let _ = Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "/System/Applications/Notes.app" to quit"#)
        .status();
    thread::sleep(Duration::from_secs(2));

    let log_file = File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;
    Command::new("/System/Applications/Notes.app/Contents/MacOS/Notes")
        .env("DYLD_INSERT_LIBRARIES", &dylib_path)
        .env("APPLE_CLI_NOTES_SHARE_NOTE_ID", &args.id)
        .env("APPLE_CLI_NOTES_SHARE_EMAIL", &args.email)
        .env("APPLE_CLI_NOTES_SHARE_RESULT", &result_path)
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .with_context(|| {
            "failed to relaunch Notes with the injected private sharing helper".to_string()
        })?;

    let deadline = Instant::now() + Duration::from_secs(args.timeout);
    while Instant::now() < deadline {
        if result_path.exists() {
            let result_text = fs::read_to_string(&result_path)
                .with_context(|| format!("failed to read {}", result_path.display()))?;
            let mut result: Value = serde_json::from_str(&result_text)
                .with_context(|| format!("failed to parse {}", result_path.display()))?;
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
            println!("{}", serde_json::to_string_pretty(&result)?);
            if result.get("status").and_then(Value::as_str) == Some("ok") {
                return Ok(());
            }
            return Err(anyhow!(
                "private Notes sharing helper returned an error at {}",
                result_path.display()
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }

    let log_tail = read_tail(&log_path, 6000).unwrap_or_default();
    Err(anyhow!(
        "timed out waiting for private Notes sharing helper result at {}. Log: {}",
        result_path.display(),
        log_tail
    ))
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!(
            "failed to compile private Notes sharing helper with {}: {}{}",
            clang.display(),
            stdout,
            stderr
        ));
    }
    Ok(())
}

fn preflight_private_notes_helper() -> Result<()> {
    if std::env::var_os("APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT").is_some() {
        return Ok(());
    }

    let csrutil = Path::new("/usr/bin/csrutil");
    if !csrutil.exists() {
        return Ok(());
    }

    let output = Command::new(csrutil).arg("status").output();
    let Ok(output) = output else {
        return Ok(());
    };
    let status_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let status_lower = status_text.to_ascii_lowercase();
    if status_lower.contains("system integrity protection status: enabled") {
        return Err(anyhow!(
            "private Notes helpers require DYLD library injection into Notes, but SIP is enabled on this Mac. Use --backend ui with Accessibility enabled, run this command on a lab Mac where SIP/library injection is relaxed, or set APPLE_CLI_SKIP_PRIVATE_NOTES_PREFLIGHT=1 to bypass this preflight."
        ));
    }
    Ok(())
}

fn private_notes_helper_available() -> bool {
    preflight_private_notes_helper().is_ok()
}

fn notes_helper_result_path(file_name: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Containers")
        .join("com.apple.Notes")
        .join("Data")
        .join("Library")
        .join("Application Support")
        .join("apple-cli")
        .join(file_name))
}

fn read_tail(path: &Path, max_chars: usize) -> Result<String> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    if text.len() <= max_chars {
        return Ok(text);
    }
    let start = text
        .char_indices()
        .rev()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    Ok(text[start..].to_string())
}

pub fn notes_search(args: NotesSearchArgs) -> Result<()> {
    let account = args.account.unwrap_or_default();
    let query = args.query;
    let limit = args.limit;
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
            set rec to (id of n as string) & fs & (name of n as string) & fs & folderName
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
    let records = parse_records(&output);
    let items: Vec<_> = records
        .into_iter()
        .map(|r| {
            let id = r.get(0).cloned().unwrap_or_default();
            let name = r.get(1).cloned().unwrap_or_default();
            let folder = r.get(2).cloned().unwrap_or_default();
            json!({ "id": id, "name": name, "folder": folder })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

pub fn notes_show(args: NotesShowArgs) -> Result<()> {
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
    let _ = run_applescript(script, &[args.id])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "status": "OK" }))?
    );
    Ok(())
}

pub fn notes_attachments_list(args: NotesAttachmentsListArgs) -> Result<()> {
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
    let output = run_applescript(script, &[args.id])?;
    let records = parse_records(&output);
    let items: Vec<_> = records
        .into_iter()
        .map(|r| {
            let id = r.get(0).cloned().unwrap_or_default();
            let name = r.get(1).cloned().unwrap_or_default();
            let content_id = r.get(2).cloned().unwrap_or_default();
            let created_at = r.get(3).cloned().unwrap_or_default();
            let modified_at = r.get(4).cloned().unwrap_or_default();
            let url = r.get(5).cloned().unwrap_or_default();
            let shared = r.get(6).cloned().unwrap_or_default();
            json!({
                "id": id,
                "name": name,
                "content_identifier": content_id,
                "created_at": created_at,
                "modified_at": modified_at,
                "url": url,
                "shared": shared
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&items)?);
    Ok(())
}

pub fn notes_attachments_save(args: NotesAttachmentsSaveArgs) -> Result<()> {
    if args.attachment_id.is_none() && args.name.is_none() {
        return Err(anyhow::anyhow!("provide --attachment-id or --name"));
    }
    let att_id = args.attachment_id.unwrap_or_default();
    let name = args.name.unwrap_or_default();
    let output_dir = args.output;
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
            set target to first attachment of n whose id is attId
        else
            set target to first attachment of n whose name is attName
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
    let output = run_applescript(script, &[args.id, att_id, name, output_dir])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "path": output }))?
    );
    Ok(())
}

pub fn notes_attachments_delete(args: NotesAttachmentsDeleteArgs) -> Result<()> {
    if args.attachment_id.is_none() && args.name.is_none() {
        return Err(anyhow::anyhow!("provide --attachment-id or --name"));
    }
    let att_id = args.attachment_id.unwrap_or_default();
    let name = args.name.unwrap_or_default();
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
            set target to first attachment of n whose id is attId
        else
            set target to first attachment of n whose name is attName
        end if
        if target is missing value then error "Attachment not found"
        delete target
        return "OK"
    end tell
end run
"#;
    let _ = run_applescript(script, &[args.id, att_id, name])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "status": "OK" }))?
    );
    Ok(())
}
