//! Bounded, transport-independent file operations inspired by earendil-works/pi.
use crate::model::{FileEdit, FileRequest, Operation, RuntimeSettings};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    fmt,
    io::{BufRead, Read},
    time::Duration,
};
use unicode_normalization::UnicodeNormalization;

pub const MAX_BYTES: usize = 50 * 1024;
pub const MAX_LINES: u64 = 2000;
pub const DIRECTORY_SCAN_LIMIT: usize = 10000;
pub const PREVIEW_BYTES: u64 = 20 * 1024 * 1024;

// Keep the existing imports available to operation implementations and callers.
pub use crate::operation_error::{
    error, error_code, error_message, error_response, error_with_details, with_stage, ErrorStage,
    OperationError as FileError,
};

pub fn require_mcp_permissions(server: &crate::model::Server, required: &[&str]) -> Result<()> {
    let missing: Vec<_> = required
        .iter()
        .copied()
        .filter(|kind| {
            // edit_file's dependency is checked separately, so don't report a
            // granted edit_file as missing when only read_file is absent.
            let explicitly_granted_edit = *kind == "edit_file"
                && server.enabled
                && server.mcp_enabled
                && server
                    .mcp_permissions
                    .as_ref()
                    .is_some_and(|permissions| permissions.iter().any(|p| p == "edit_file"));
            !server.permits_mcp(kind) && !explicitly_granted_edit
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(error_with_details(
        "PERMISSION_DENIED",
        "MCP operation is not authorized",
        json!({
            "required_permissions":required,"missing_permissions":missing,
            "granted_permissions":server.effective_mcp_permissions(),
            "mcp_enabled":server.mcp_enabled,"server_enabled":server.enabled,
        }),
    ))
}

pub fn mutates(kind: &str) -> bool {
    matches!(
        kind,
        "write_file" | "edit_file" | "upload" | "remove" | "remove_path" | "rename" | "mkdir"
    )
}

pub fn validate(op: &Operation, settings: &RuntimeSettings) -> Result<()> {
    if let Some(request) = &op.file_request {
        let valid = match request {
            FileRequest::Remove { .. } => op.kind == "remove_path",
            FileRequest::Read { offset, limit } => {
                op.kind == "read_file" && *offset >= 1 && (1..=MAX_LINES).contains(limit)
            }
            FileRequest::Preview => op.kind == "read_file",
            FileRequest::List { limit, .. } => {
                op.kind == "list_files" && (1..=1000).contains(limit)
            }
            FileRequest::Write { .. } => op.kind == "write_file",
            FileRequest::Edit { edits } => {
                if op.kind != "edit_file" || edits.is_empty() || edits.len() > 100 {
                    return Err(error(
                        "INVALID_ARGUMENT",
                        "edits must contain 1-100 replacements",
                    ));
                }
                let limit = settings.transfer_limit("upload");
                let mut bytes = 0u64;
                for edit in edits {
                    if edit.old_text.is_empty() {
                        return Err(error("INVALID_ARGUMENT", "old_text must not be empty"));
                    }
                    bytes = bytes
                        .saturating_add(edit.old_text.len() as u64)
                        .saturating_add(edit.new_text.len() as u64);
                    if bytes > limit {
                        return Err(error(
                            "PAYLOAD_TOO_LARGE",
                            "Edit payload exceeds upload limit",
                        ));
                    }
                }
                true
            }
        };
        if !valid {
            return Err(error(
                "INVALID_ARGUMENT",
                "Invalid file request or pagination range",
            ));
        }
    } else if matches!(op.kind.as_str(), "edit_file" | "remove_path") {
        return Err(error(
            "INVALID_ARGUMENT",
            "This operation requires a typed file request",
        ));
    }
    Ok(())
}

/// Read a bounded prefix with one-byte lookahead, preserving UTF-8 boundaries.
pub fn read_preview(reader: impl Read, size: u64, limit: u64) -> Result<Value> {
    if limit == 0 || limit > PREVIEW_BYTES {
        return Err(error("INVALID_ARGUMENT", "Invalid text preview size"));
    }
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > limit;
    let observed_size = bytes.len() as u64;
    bytes.truncate(limit as usize);
    if bytes.contains(&0) {
        return Err(error(
            "UNSUPPORTED_TEXT",
            "Not UTF-8 text; download the file to view it",
        ));
    }
    let content = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(e) if truncated && e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap()
        }
        Err(_) => {
            return Err(error(
                "UNSUPPORTED_TEXT",
                "Not UTF-8 text; download the file to view it",
            ))
        }
    };
    Ok(json!({
        "kind": "text", "content": content, "bytes": size.max(observed_size),
        "truncated": truncated, "preview_limit_bytes": limit,
    }))
}

/// Stream without allocating the whole file or an unbounded single line.
pub fn read_page(
    mut reader: impl BufRead,
    offset: u64,
    limit: u64,
    scan_limit: u64,
) -> Result<Value> {
    if offset == 0 || !(1..=MAX_LINES).contains(&limit) {
        return Err(error(
            "INVALID_ARGUMENT",
            "offset must be >=1; limit must be 1-2000",
        ));
    }
    let mut scanned = 0u64;
    let mut line_number = 1u64;
    let mut returned = 0u64;
    let mut content = String::new();
    let mut eof = false;
    let mut reason = None;
    let mut long_line = false;
    loop {
        // One-byte lookahead is bounded by the reader buffer, not a whole line.
        if reader.fill_buf()?.is_empty() {
            eof = true;
            break;
        }
        if scanned >= scan_limit {
            reason = Some("scan_bytes");
            break;
        }
        if returned >= limit {
            reason = Some("requested_limit");
            break;
        }
        let selected = line_number >= offset;
        let mut line = Vec::new();
        let mut complete = false;
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                complete = true;
                eof = true;
                break;
            }
            let remaining = scan_limit.saturating_sub(scanned) as usize;
            if remaining == 0 {
                break;
            }
            let end = available
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(available.len())
                .min(remaining);
            let segment = &available[..end];
            let newline = segment.last() == Some(&b'\n');
            if selected {
                let keep = end.min((MAX_BYTES + 1).saturating_sub(line.len()));
                line.extend_from_slice(&segment[..keep]);
            }
            scanned += end as u64;
            reader.consume(end);
            if selected && line.len() > MAX_BYTES {
                long_line = true;
                break;
            }
            if newline {
                complete = true;
                break;
            }
        }
        if long_line {
            reason = Some(if returned == 0 {
                "long_line"
            } else {
                "output_bytes"
            });
            break;
        }
        if !complete {
            reason = Some("scan_bytes");
            break;
        }
        if selected {
            if content.len() + line.len() > MAX_BYTES {
                reason = Some("output_bytes");
                break;
            }
            if line.contains(&0) {
                return Err(error(
                    "UNSUPPORTED_TEXT",
                    "Not UTF-8 text; download the file or use exec if permitted",
                ));
            }
            let text = std::str::from_utf8(&line).map_err(|_| {
                error(
                    "UNSUPPORTED_TEXT",
                    "Not UTF-8 text; download the file or use exec if permitted",
                )
            })?;
            content.push_str(text);
            returned += 1;
        }
        line_number += 1;
        if eof {
            break;
        }
    }
    if returned == 0 && ((eof && offset > 1) || reason == Some("scan_bytes")) {
        return Err(if eof {
            error(
                "OFFSET_OUT_OF_RANGE",
                "Offset is beyond EOF; read again from offset=1",
            )
        } else {
            error(
                "READ_SCAN_LIMIT",
                "Cannot locate offset within scan budget; use bounded exec inspection if permitted",
            )
        });
    }
    let has_more = !eof;
    let first_line_exceeds_limit = returned == 0 && long_line;
    let next = if has_more && !first_line_exceeds_limit {
        Some(offset + returned)
    } else {
        None
    };
    Ok(json!({
        "content":content, "start_line":if returned > 0 {Some(offset)} else {None},
        "end_line":if returned > 0 {Some(offset + returned - 1)} else {None},
        "returned_lines":returned, "has_more":has_more, "next_offset":next,
        "total_lines":if eof {Some(line_number.saturating_sub(1))} else {None},
        "truncated":has_more, "truncated_by":reason,
        "first_line_exceeds_limit":first_line_exceeds_limit, "scanned_bytes":scanned
    }))
}

pub fn list_page(entries: Vec<Value>, offset: u64, limit: u64) -> Result<Value> {
    list_page_with_budget(entries, offset, limit, MAX_BYTES - 1024)
}

pub fn list_page_with_budget(
    entries: Vec<Value>,
    offset: u64,
    limit: u64,
    budget: usize,
) -> Result<Value> {
    if !(1..=1000).contains(&limit) {
        return Err(error("INVALID_ARGUMENT", "Directory limit must be 1-1000"));
    }
    let mut selected = Vec::new();
    let mut bytes = 2usize;
    let mut reason = None;
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(entries.len());
    for entry in entries.iter().skip(start) {
        if selected.len() as u64 >= limit {
            reason = Some("requested_limit");
            break;
        }
        let size = serde_json::to_vec(entry)?.len() + 1;
        if bytes + size > budget {
            if selected.is_empty() {
                return Err(error(
                    "ENTRY_TOO_LARGE",
                    "Directory entry exceeds output budget",
                ));
            }
            reason = Some("output_bytes");
            break;
        }
        bytes += size;
        selected.push(entry.clone());
    }
    let end = start + selected.len();
    let more = end < entries.len();
    Ok(
        json!({"entries":selected,"has_more":more,"next_offset":if more {Some(end)} else {None},"truncated":more,"truncated_by":reason}),
    )
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
fn fuzzy(text: &str) -> String {
    let normalized: String = text
        .nfkc()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{a0}' | '\u{2002}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
            _ => c,
        })
        .collect();
    normalized
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct EditResult {
    pub content: String,
    pub response: Value,
}

pub fn edit_text(original: &str, edits: &[FileEdit], output_limit: u64) -> Result<EditResult> {
    if edits.is_empty() || edits.len() > 100 {
        return Err(error(
            "INVALID_ARGUMENT",
            "edits must contain 1-100 replacements",
        ));
    }
    if original.contains('\0') {
        return Err(error("UNSUPPORTED_TEXT", "Cannot edit binary files"));
    }
    let (bom, raw) = original
        .strip_prefix('\u{feff}')
        .map(|s| ("\u{feff}", s))
        .unwrap_or(("", original));
    let crlf = raw
        .find('\n')
        .map(|i| i > 0 && raw.as_bytes()[i - 1] == b'\r')
        .unwrap_or(false);
    let normalized = lf(raw);
    let pairs: Vec<_> = edits
        .iter()
        .map(|e| (lf(&e.old_text), lf(&e.new_text)))
        .collect();
    let use_fuzzy = pairs.iter().any(|(old, _)| !normalized.contains(old));
    let fuzzy_base = fuzzy(&normalized);
    let base = if use_fuzzy { &fuzzy_base } else { &normalized };
    let mut replacements = Vec::new();
    for (old, new) in &pairs {
        let fuzzy_old = fuzzy(old);
        if old.is_empty() || fuzzy_old.is_empty() {
            return Err(error(
                "INVALID_ARGUMENT",
                "old_text must not be empty after normalization",
            ));
        }
        let needle = if use_fuzzy { &fuzzy_old } else { old };
        let start = base.find(needle).ok_or_else(|| {
            error(
                "EDIT_NOT_FOUND",
                "Text not found; read again and provide matching context",
            )
        })?;
        // Count in fuzzy space even when exact matching succeeded, like Pi.
        if fuzzy_base.match_indices(&fuzzy_old).take(2).count() > 1 {
            return Err(error(
                "EDIT_AMBIGUOUS",
                "Text is not unique; include more surrounding context",
            ));
        }
        replacements.push((start, start + needle.len(), new.as_str()));
    }
    replacements.sort_by_key(|r| r.0);
    if replacements.windows(2).any(|r| r[0].1 > r[1].0) {
        return Err(error(
            "EDIT_OVERLAP",
            "Replacements overlap; merge nearby changes into one edit",
        ));
    }
    let final_size = replacements
        .iter()
        .try_fold(base.len() as u64, |n, (start, end, new)| {
            n.checked_sub((end - start) as u64)
                .and_then(|n| n.checked_add(new.len() as u64))
        })
        .ok_or_else(|| error("PAYLOAD_TOO_LARGE", "Edited file is too large"))?;
    if final_size > output_limit {
        return Err(error(
            "PAYLOAD_TOO_LARGE",
            "Edited file exceeds upload limit",
        ));
    }
    let mut changed = base.to_string();
    for (start, end, new) in replacements.iter().rev() {
        changed.replace_range(*start..*end, new);
    }
    if use_fuzzy {
        // Overlay changed line blocks onto the original so equal lines keep original punctuation/spacing.
        let original_lines: Vec<_> = normalized.split_inclusive('\n').collect();
        let diff = similar::TextDiff::configure()
            .timeout(Duration::from_millis(200))
            .diff_lines(base, &changed);
        let mut preserved = String::new();
        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Equal => {
                    preserved.push_str(original_lines[change.old_index().unwrap()])
                }
                similar::ChangeTag::Insert => preserved.push_str(change.value()),
                similar::ChangeTag::Delete => {}
            }
        }
        changed = preserved;
    }
    if changed == normalized {
        return Err(error("EDIT_NO_CHANGE", "Replacement produced no changes"));
    }
    let common = normalized
        .bytes()
        .zip(changed.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let first_line = changed.as_bytes()[..common]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1;
    let final_content = format!(
        "{bom}{}",
        if crlf {
            changed.replace('\n', "\r\n")
        } else {
            changed.clone()
        }
    );
    if final_content.len() as u64 > output_limit {
        return Err(error(
            "PAYLOAD_TOO_LARGE",
            "Edited file exceeds upload limit",
        ));
    }
    let mut patch = BoundedText::default();
    let diff = similar::TextDiff::configure()
        .timeout(Duration::from_millis(200))
        .diff_lines(&normalized, &changed);
    let _ = fmt::write(
        &mut patch,
        format_args!(
            "{}",
            diff.unified_diff()
                .context_radius(4)
                .header("before", "after")
        ),
    );
    Ok(EditResult {
        response: json!({"changed":true,"applied_edits":edits.len(),"first_changed_line":first_line,
        "written_bytes":final_content.len(),"diff":patch.text,"diff_truncated":patch.truncated,"used_fuzzy_match":use_fuzzy}),
        content: final_content,
    })
}

#[derive(Default)]
struct BoundedText {
    text: String,
    truncated: bool,
}
impl fmt::Write for BoundedText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut end = s.len().min(MAX_BYTES.saturating_sub(self.text.len()));
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&s[..end]);
        if end < s.len() {
            self.truncated = true;
            return Err(fmt::Error);
        }
        Ok(())
    }
}
