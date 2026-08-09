use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct OpenApi {
    paths: BTreeMap<String, BTreeMap<String, Operation>>,
}

#[derive(Debug, Deserialize)]
struct Operation {
    #[serde(rename = "operationId")]
    operation_id: String,
}

fn main() {
    println!("cargo:rerun-if-changed=openapi/notes-server.yaml");

    let spec = fs::read_to_string("openapi/notes-server.yaml")
        .expect("failed to read openapi/notes-server.yaml");
    let openapi: OpenApi =
        serde_yaml::from_str(&spec).expect("failed to parse openapi/notes-server.yaml");

    let mut output = String::from(
        "pub fn generated_notes_routes() -> axum::Router<std::sync::Arc<crate::notes_server::ServerState>> {\n",
    );
    output.push_str("    use axum::routing::{delete, get, post};\n");
    output.push_str("    axum::Router::new()\n");

    for (path, methods) in openapi.paths {
        let mut method_calls = Vec::new();
        for (method, operation) in methods {
            if !matches!(method.as_str(), "get" | "post" | "patch" | "delete") {
                continue;
            }
            let handler = operation_id_to_handler(&operation.operation_id);
            method_calls.push(format!(
                "{}(crate::notes_server::{})",
                method.as_str(),
                handler
            ));
        }
        if method_calls.is_empty() {
            continue;
        }
        let axum_path = openapi_path_to_axum_path(&path);
        output.push_str(&format!(
            "        .route({:?}, {})\n",
            axum_path,
            method_calls.join(".")
        ));
    }

    output.push_str("}\n");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR missing"));
    fs::write(out_dir.join("notes_server_routes.rs"), output)
        .expect("failed to write generated notes server routes");
}

fn openapi_path_to_axum_path(path: &str) -> String {
    let mut output = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            output.push(':');
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                output.push(inner);
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn operation_id_to_handler(operation_id: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in operation_id.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if idx != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
