//! Formatting contract at the helper boundary, shared by REST/MCP and CLI writes.
use anyhow::{bail, Result};
use kuchikiki::{traits::TendrilSink, NodeData, NodeRef};
use pulldown_cmark::{html, Event, Options, Parser};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub struct PreparedWrite {
    pub params: Value,
    format: Option<String>,
    expected: Option<Snapshot>,
    warnings: Vec<Value>,
}

#[derive(Debug, Default, PartialEq)]
struct Snapshot {
    lines: Vec<String>,
    urls: BTreeSet<String>,
    hrefs: BTreeSet<String>,
    tables: Vec<Vec<Vec<String>>>,
    table_spans: Vec<Vec<Vec<(u64, u64)>>>,
    headings: Vec<String>,
    lists: Vec<String>,
}

fn warning(code: &str, message: &str) -> Value {
    json!({"code": code, "message": message})
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn plain_html(text: &str) -> String {
    normalize_newlines(text)
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                "<div><br></div>".to_string()
            } else {
                format!("<div>{}</div>", escape(line))
            }
        })
        .collect()
}

fn parse(html: &str) -> NodeRef {
    kuchikiki::parse_html().one(html)
}

fn looks_like_html(text: &str) -> bool {
    let document = parse(text);
    document.select("body *").unwrap().next().is_some()
}

fn field<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        _ => bail!("invalid.params: {key} must be a string"),
    }
}

pub fn prepare(params: &Value) -> Result<PreparedWrite> {
    let content = field(params, "content")?;
    let format = field(params, "contentFormat")?;
    let html = field(params, "html")?;
    let body = field(params, "body")?;
    let mut warnings = Vec::new();
    let selected = if let Some(content) = content {
        if html.is_some() || body.is_some() {
            bail!("invalid.params: content cannot be combined with html/body; use only content and contentFormat");
        }
        let format = format.ok_or_else(|| {
            anyhow::anyhow!(
                "invalid.params: content requires contentFormat (plain_text, html, markdown)"
            )
        })?;
        if !["plain_text", "html", "markdown"].contains(&format) {
            bail!("invalid.params: contentFormat must be plain_text, html or markdown");
        }
        Some((format, content))
    } else {
        if format.is_some() {
            bail!("invalid.params: contentFormat requires content (empty string is allowed)");
        }
        if html.is_some() && body.is_some() {
            warnings.push(warning("formatting.ambiguous_legacy_fields", "Both html and body were supplied; html took precedence. Use content with contentFormat for an unambiguous write."));
        }
        html.map(|value| ("html", value)).or_else(|| body.map(|value| {
            warnings.push(warning("formatting.legacy_body", "body was interpreted as HTML when it contained elements, otherwise as plain text. Use content with contentFormat to specify the format explicitly."));
            (if looks_like_html(value) { "html" } else { "plain_text" }, value)
        }))
    };

    let mut prepared = params.clone();
    let object = prepared
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("invalid.params: params must be an object"))?;
    for key in ["body", "html", "content", "contentFormat"] {
        object.remove(key);
    }
    let (format, expected) = if let Some((format, content)) = selected {
        let rendered = match format {
            "plain_text" => plain_html(content),
            "markdown" => {
                let events = Parser::new_ext(
                    content,
                    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
                )
                .map(|event| {
                    if matches!(event, Event::SoftBreak) {
                        Event::HardBreak
                    } else {
                        event
                    }
                });
                let mut rendered = String::new();
                html::push_html(&mut rendered, events);
                rendered
            }
            _ => content.to_string(),
        };
        let document = parse(&rendered);
        let anchors: Vec<_> = document.select("a[href]").unwrap().collect();
        let mut fallback_added = false;
        for anchor in anchors {
            let href = anchor
                .attributes
                .borrow()
                .get("href")
                .unwrap_or("")
                .to_string();
            if !href.is_empty() && !anchor.text_contents().contains(&href) {
                // Notes can turn anchors into underlined labels. Keep their destinations visible.
                anchor
                    .as_node()
                    .append(NodeRef::new_text(format!(" ({href})")));
                fallback_added = true;
            }
        }
        let rendered = if fallback_added {
            warnings.push(warning("formatting.visible_url_fallback", "Link destinations were added to visible link labels because Notes may discard href attributes. Native clickability is best-effort; check the returned link validation."));
            document.to_string()
        } else {
            rendered
        };
        if format == "html" && content.contains('\n') {
            warnings.push(warning("formatting.html_whitespace", "HTML source newlines follow HTML whitespace rules. Use br/block elements, or plain_text, for intended line breaks."));
        }
        let expected = snapshot(&rendered);
        object.insert("html".into(), Value::String(rendered));
        (Some(format.to_string()), Some(expected))
    } else {
        (None, None)
    };
    Ok(PreparedWrite {
        params: prepared,
        format,
        expected,
        warnings,
    })
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_content_outside_tables(node: &NodeRef) -> bool {
    if let Some(element) = node.as_element() {
        if element.name.local.as_ref() == "table" {
            return false;
        }
    }
    if let NodeData::Text(text) = node.data() {
        return !text.borrow().trim().is_empty();
    }
    node.children()
        .any(|child| has_content_outside_tables(&child))
}

fn is_native_heading(node: &NodeRef) -> bool {
    // Observed Notes 26.3 export for imported h1/h2: div > b > span, 24/18px.
    // Require the entire paragraph to be that styled heading, not arbitrary large text.
    node.select("b > span[style], strong > span[style]")
        .unwrap()
        .any(|span| {
            let direct = span
                .as_node()
                .parent()
                .and_then(|bold| bold.parent())
                .as_ref()
                == Some(node);
            let attributes = span.attributes.borrow();
            let size = attributes
                .get("style")
                .unwrap_or("")
                .split(';')
                .find_map(|declaration| {
                    let (name, value) = declaration.split_once(':')?;
                    if !name.trim().eq_ignore_ascii_case("font-size") {
                        return None;
                    }
                    value.trim().strip_suffix("px")?.trim().parse::<u32>().ok()
                });
            direct
                && matches!(size, Some(24 | 18))
                && compact(&span.text_contents()) == compact(&node.text_contents())
        })
}

fn visible(node: &NodeRef, output: &mut String, skip_tables: bool, preformatted: bool) {
    if let NodeData::Text(text) = node.data() {
        let text = text.borrow();
        if preformatted {
            output.push_str(&normalize_newlines(&text));
            return;
        }
        if text.trim().is_empty() && (output.is_empty() || output.ends_with('\n')) {
            return;
        }
        // Source indentation between elements is not a display line break.
        if text.starts_with(char::is_whitespace)
            && !output.is_empty()
            && !output.ends_with(char::is_whitespace)
        {
            output.push(' ');
        }
        output.push_str(&text.split_whitespace().collect::<Vec<_>>().join(" "));
        if text.ends_with(char::is_whitespace) && !output.ends_with(' ') {
            output.push(' ');
        }
        return;
    }
    let name = node
        .as_element()
        .map(|element| element.name.local.as_ref())
        .unwrap_or("");
    if ["head", "script", "style"].contains(&name) {
        return;
    }
    if name == "br" {
        output.push('\n');
        return;
    }
    let table_wrapper = skip_tables
        && ["div", "object"].contains(&name)
        && node.select("table").unwrap().next().is_some()
        && !has_content_outside_tables(node);
    if skip_tables && (name == "table" || table_wrapper) {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        return; // Tables are compared separately by rows, cells and text.
    }
    let block = [
        "div",
        "p",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "pre",
        "blockquote",
        "section",
        "td",
        "th",
        "tr",
    ]
    .contains(&name);
    if block && !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    let start = output.len();
    for child in node.children() {
        visible(&child, output, skip_tables, preformatted || name == "pre");
    }
    if block && (output.len() == start || !output.ends_with('\n')) {
        output.push('\n');
    }
}

fn snapshot(html: &str) -> Snapshot {
    let document = parse(html);
    let mut text = String::new();
    visible(&document, &mut text, true, false);
    if text.ends_with('\n') {
        text.pop();
    }
    let mut snapshot = Snapshot {
        lines: text.split('\n').map(compact).collect(),
        ..Default::default()
    };
    for anchor in document.select("a[href]").unwrap() {
        let href = anchor
            .attributes
            .borrow()
            .get("href")
            .unwrap_or("")
            .to_string();
        if !href.is_empty() {
            snapshot.hrefs.insert(href);
        }
    }
    snapshot.urls = snapshot.hrefs.clone();
    // Parse decoded text, not HTML source: &amp; in both URLs and hrefs becomes &.
    let mut text = String::new();
    visible(&document, &mut text, false, false);
    for part in text.split(|c: char| c.is_whitespace() || ['<', '>', '"'].contains(&c)) {
        let part = part.trim_start_matches(['(', '[']);
        if part.starts_with("https://")
            || part.starts_with("http://")
            || part.starts_with("mailto:")
        {
            let mut part = part.trim_end_matches([',', '.', ';']);
            while (part.ends_with(')') && part.matches(')').count() > part.matches('(').count())
                || (part.ends_with(']') && part.matches(']').count() > part.matches('[').count())
            {
                part = &part[..part.len() - 1];
            }
            snapshot.urls.insert(part.to_string());
        }
    }
    for table in document.select("table").unwrap() {
        snapshot.table_spans.push(
            table
                .as_node()
                .select("tr")
                .unwrap()
                .map(|row| {
                    row.as_node()
                        .select("td, th")
                        .unwrap()
                        .map(|cell| {
                            let attributes = cell.attributes.borrow();
                            let span = |name| {
                                attributes
                                    .get(name)
                                    .and_then(|value| value.parse::<u64>().ok())
                                    .unwrap_or(1)
                            };
                            (span("rowspan"), span("colspan"))
                        })
                        .collect()
                })
                .collect(),
        );
        snapshot.tables.push(
            table
                .as_node()
                .select("tr")
                .unwrap()
                .map(|row| {
                    row.as_node()
                        .select("td, th")
                        .unwrap()
                        .map(|cell| {
                            let mut text = String::new();
                            visible(cell.as_node(), &mut text, false, false);
                            if text.ends_with('\n') {
                                text.pop();
                            }
                            text.split('\n').map(compact).collect::<Vec<_>>().join("\n")
                        })
                        .collect()
                })
                .collect(),
        );
    }
    snapshot.headings = document
        .select("h1, h2, h3, h4, h5, h6, div")
        .unwrap()
        .filter(|node| node.name.local.as_ref() != "div" || is_native_heading(node.as_node()))
        .map(|node| compact(&node.text_contents()))
        .collect();
    snapshot.lists = document
        .select("li")
        .unwrap()
        .map(|n| compact(&n.text_contents()))
        .collect();
    snapshot
}

impl PreparedWrite {
    pub fn validate(self, mut result: Value) -> Value {
        let mut warnings = self.warnings;
        let expected_title = result
            .as_object_mut()
            .and_then(|object| object.remove("_formattingExpectedTitle"));
        let title_ok = expected_title
            .as_ref()
            .map(|expected| result.get("title") == Some(expected));
        if title_ok == Some(false) {
            warnings.push(warning("formatting.title_lost", "The write completed but the saved title differs. Read the note before an explicit title correction; do not retry the entire write."));
        }
        let mut checks = json!({"title": title_ok});
        let readback = result
            .get("html")
            .or_else(|| result.get("body"))
            .and_then(Value::as_str);
        let mut loss = title_ok == Some(false);
        let mut verified = self.expected.is_none();
        if let Some(expected) = self.expected {
            if let Some(readback) = readback {
                verified = true;
                let actual = snapshot(readback);
                for (key, passed, code, message) in [
                    ("urls", expected.urls.is_subset(&actual.urls), "formatting.urls_lost", "Saved content lost URL destinations. Read the note and repair with visible bare URLs; do not blindly retry the completed write."),
                    ("lineBreaks", expected.lines == actual.lines, "formatting.lines_changed", "Saved text/line boundaries differ, including possible lost blank lines. Inspect readback; prefer plain_text for literal newlines or explicit HTML br/block elements."),
                    ("tables", expected.tables == actual.tables && expected.table_spans == actual.table_spans, "formatting.tables_changed", "Saved HTML table rows, cells, spans or text differ. Inspect the note and repair using simple rectangular tables. HTML readback does not prove native table editability."),
                    ("headings", expected.headings == actual.headings, "formatting.headings_changed", "Saved HTML headings differ. Inspect the note before correcting its section headings."),
                    ("lists", expected.lists == actual.lists, "formatting.lists_changed", "Saved HTML list items differ. Inspect readback before correcting the list."),
                    ("hyperlinks", expected.hrefs.is_subset(&actual.hrefs), "formatting.hrefs_lost", "Notes removed hyperlink destinations from anchor attributes. Visible URL fallbacks may survive, but native clickability is not guaranteed."),
                ] {
                    checks[key] = json!(passed);
                    if !passed { loss = true; warnings.push(warning(code, message)); }
                }
            } else {
                warnings.push(warning("formatting.unverified", "The write completed without HTML readback. Formatting could not be validated; get the note before any correction."));
            }
        }
        result["formatting"] = json!({
            "contractVersion": 1, "inputFormat": self.format,
            "validation": if !verified { "unverified" } else if loss { "loss_detected" } else { "passed" },
            "validationScope": "post-save independent Notes HTML readback; does not certify native editability or later iCloud sync",
            "checks": checks, "warnings": warnings, "writeCompleted": true
        });
        result
    }
}

pub fn capabilities() -> Value {
    json!({
        "contractVersion": 1,
        "inputFormats": ["plain_text", "html", "markdown"],
        "preferredFields": ["content", "contentFormat"],
        "plainTextNewlines": "escaped into explicit HTML line blocks, including blank lines",
        "headingsSectionsLists": "HTML import, best-effort with post-save readback checks",
        "links": {"visibleUrlFallback": true, "nativeHyperlinks": "best-effort; missing hrefs produce warnings"},
        "tables": {"input": "simple HTML tables or Markdown pipe tables", "nativeTables": "best-effort, not guaranteed", "validation": "HTML rows, cells and text; not native editability"},
        "titleOnContentUpdate": "preserved unless title/name is supplied",
        "validation": "result.formatting; a loss warning follows a completed write, never blindly retry",
        "readbackFormat": "html"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(params: Value, html: &str, title: &str) -> Value {
        prepare(&params).unwrap().validate(
            json!({"html": html, "title": title, "_formattingExpectedTitle": "Stable title"}),
        )
    }

    #[test]
    fn plain_text_preserves_newlines_blank_lines_and_literal_markup() {
        let prepared =
            prepare(&json!({"contentFormat":"plain_text", "content":"one\r\n\r\ntwo & <three>\n"}))
                .unwrap();
        let html = prepared.params["html"].as_str().unwrap().to_string();
        assert_eq!(
            html,
            "<div>one</div><div><br></div><div>two &amp; &lt;three&gt;</div><div><br></div>"
        );
        assert_eq!(snapshot(&html).lines, ["one", "", "two & <three>", ""]);
        let result = prepared.validate(json!({"html": html}));
        assert_eq!(result["formatting"]["validation"], "passed");
    }

    #[test]
    fn legacy_fields_remain_compatible_and_ambiguity_is_reported() {
        assert_eq!(
            prepare(&json!({"body":"one\ntwo"}))
                .unwrap()
                .format
                .as_deref(),
            Some("plain_text")
        );
        let prepared = prepare(&json!({"html":"<h1>HTML</h1>", "body":"ignored"})).unwrap();
        assert_eq!(prepared.params["html"], "<h1>HTML</h1>");
        assert_eq!(
            prepared.warnings[0]["code"],
            "formatting.ambiguous_legacy_fields"
        );
        assert!(prepare(&json!({"content":"x", "contentFormat":"html", "body":"y"})).is_err());
        assert!(prepare(&json!({"content":"x"})).is_err());
        assert!(prepare(&json!({"contentFormat":"html"})).is_err());
        assert!(prepare(&json!({"content":"x", "contentFormat":"rtf"})).is_err());
    }

    #[test]
    fn links_decode_amp_entities_and_keep_destinations_visible() {
        let prepared = prepare(
            &json!({"html":"<p><a href='https://example.com/?a=1&amp;b=2'>Source</a></p>"}),
        )
        .unwrap();
        assert!(prepared.params["html"]
            .as_str()
            .unwrap()
            .contains("Source (https://example.com/?a=1&amp;b=2)"));
        let result = prepared
            .validate(json!({"html":"<div>Source (https://example.com/?a=1&amp;b=2)</div>"}));
        assert_eq!(result["formatting"]["checks"]["urls"], true);
        assert_eq!(result["formatting"]["checks"]["hyperlinks"], false);
        let lost = roundtrip(
            json!({"html":"<a href='https://example.com/?a=1&amp;b=2'>Source</a>"}),
            "<div>Source</div>",
            "Stable title",
        );
        assert_eq!(lost["formatting"]["checks"]["urls"], false);
    }

    #[test]
    fn table_structure_is_checked_not_just_cell_text_presence() {
        let params = json!({"html":"<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table><table><tr><td>other</td></tr></table>"});
        let good = roundtrip(
            params.clone(),
            params["html"].as_str().unwrap(),
            "Stable title",
        );
        assert_eq!(good["formatting"]["checks"]["tables"], true);
        for bad in [
            "<div>A B 1 2 other</div>",
            "<table><tr><td>A B</td></tr><tr><td>1 2</td></tr></table>",
            "<table><tr><td>B</td><td>A</td></tr><tr><td>2</td><td>1</td></tr></table>",
        ] {
            assert_eq!(
                roundtrip(params.clone(), bad, "Stable title")["formatting"]["checks"]["tables"],
                false
            );
        }
    }

    #[test]
    fn markdown_renders_sections_lists_links_tables_and_soft_breaks() {
        let prepared = prepare(&json!({"contentFormat":"markdown", "content":"# Section\n\nLine one\nLine two\n\n- item\n\n[Source](https://example.com/?a=1&b=2)\n\n| A | B |\n|---|---|\n| 1 | 2 |"})).unwrap();
        let html = prepared.params["html"].as_str().unwrap();
        let snapshot = snapshot(html);
        assert_eq!(snapshot.headings, ["Section"]);
        assert_eq!(snapshot.lists, ["item"]);
        assert_eq!(snapshot.tables[0], [["A", "B"], ["1", "2"]]);
        assert!(html.contains("<br"));
        assert!(snapshot.urls.contains("https://example.com/?a=1&b=2"));
    }

    #[test]
    fn collapsed_lines_title_loss_and_missing_readback_cannot_pass() {
        let params = json!({"contentFormat":"plain_text", "content":"one\n\ntwo"});
        let bad = roundtrip(params.clone(), "<div>one two</div>", "one two");
        assert_eq!(bad["formatting"]["validation"], "loss_detected");
        assert_eq!(bad["formatting"]["checks"]["lineBreaks"], false);
        assert_eq!(bad["formatting"]["checks"]["title"], false);
        assert!(bad.get("_formattingExpectedTitle").is_none());
        let missing = prepare(&params).unwrap().validate(json!({"id":"test"}));
        assert_eq!(missing["formatting"]["validation"], "unverified");
        assert_eq!(
            prepare(&json!({"content":"", "contentFormat":"plain_text"}))
                .unwrap()
                .params["html"],
            "<div><br></div>"
        );
    }

    #[test]
    fn validation_handles_block_urls_preformatted_lines_and_cell_breaks() {
        let urls = snapshot("<div>https://example.com/a</div><div>https://example.com/b_(c)</div>");
        assert!(urls.urls.contains("https://example.com/a"));
        assert!(urls.urls.contains("https://example.com/b_(c)"));
        let pre = roundtrip(
            json!({"html":"<pre>one\n\ntwo</pre>"}),
            "<pre>one two</pre>",
            "Stable title",
        );
        assert_eq!(pre["formatting"]["checks"]["lineBreaks"], false);
        let cell = roundtrip(
            json!({"html":"<table><tr><td>a<br>b</td></tr></table>"}),
            "<table><tr><td>ab</td></tr></table>",
            "Stable title",
        );
        assert_eq!(cell["formatting"]["checks"]["tables"], false);
        let spans = roundtrip(
            json!({"html":"<table><tr><td colspan='2'>a</td></tr></table>"}),
            "<table><tr><td>a</td></tr></table>",
            "Stable title",
        );
        assert_eq!(spans["formatting"]["checks"]["tables"], false);
    }

    #[test]
    fn observed_macos_export_preserves_styled_headings_and_table_object_lines() {
        let params = json!({"html": concat!(
            "<h1>Fixture Section</h1><p>Intro.</p><div><br></div><h2>Links</h2>",
            "<p><a href='https://example.com/?a=1&amp;b=2'>Source</a></p>",
            "<ul><li>First bullet</li><li>Second bullet</li></ul>",
            "<table><tr><th>Kind</th><th>Value</th></tr><tr><td>Alpha</td><td>10</td></tr></table>",
            "<table><tr><td>Second table</td><td>20</td></tr></table>")});
        let actual = include_str!("../tests/fixtures/notes-macos26_3-formatted.html");
        let result = roundtrip(params, actual, "Stable title");
        for feature in ["headings", "lineBreaks", "tables", "lists", "urls", "title"] {
            assert_eq!(result["formatting"]["checks"][feature], true, "{feature}");
        }
        assert_eq!(result["formatting"]["checks"]["hyperlinks"], false);
        let unstyled = roundtrip(
            json!({"html":"<h1>Heading</h1>"}),
            "<div><b>Heading</b></div>",
            "Stable title",
        );
        assert_eq!(unstyled["formatting"]["checks"]["headings"], false);
    }
}
