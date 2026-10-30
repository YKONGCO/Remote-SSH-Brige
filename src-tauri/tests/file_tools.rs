use remote_desk_lib::{
    file_tools::{self, edit_text, read_page, MAX_BYTES},
    model::{FileEdit, FileRequest, Operation, RuntimeSettings},
    security,
};
use serde_json::json;
use std::io::{BufReader, Cursor};

fn edits(pairs: &[(&str, &str)]) -> Vec<FileEdit> {
    pairs
        .iter()
        .map(|(old, new)| FileEdit {
            old_text: (*old).into(),
            new_text: (*new).into(),
        })
        .collect()
}
fn code(error: anyhow::Error) -> &'static str {
    error.downcast_ref::<file_tools::FileError>().unwrap().code
}

#[test]
fn read_preserves_original_text_and_paginates_by_one_based_lines() {
    let bytes = "第一行\r\n\r\nthird\nlast";
    let first = read_page(Cursor::new(bytes), 1, 2, 1000).unwrap();
    assert_eq!(first["content"], "第一行\r\n\r\n");
    assert_eq!(first["next_offset"], 3);
    assert_eq!(first["total_lines"], json!(null));
    let last = read_page(Cursor::new(bytes), 3, 2, 1000).unwrap();
    assert_eq!(last["content"], "third\nlast");
    assert_eq!(last["has_more"], false);
    assert_eq!(last["total_lines"], 4);
}

#[test]
fn read_handles_empty_files_eof_and_invalid_parameters() {
    let empty = read_page(Cursor::new(""), 1, 10, 1000).unwrap();
    assert_eq!(empty["returned_lines"], 0);
    assert_eq!(empty["start_line"], json!(null));
    assert_eq!(empty["total_lines"], 0);
    assert_eq!(
        read_page(Cursor::new("a\n"), 1, 10, 1000).unwrap()["total_lines"],
        1
    );
    for bytes in ["a", "a\n", ""] {
        assert_eq!(
            code(read_page(Cursor::new(bytes), 2, 10, 1000).unwrap_err()),
            "OFFSET_OUT_OF_RANGE"
        );
    }
    assert_eq!(
        code(read_page(Cursor::new("a"), 0, 10, 1000).unwrap_err()),
        "INVALID_ARGUMENT"
    );
    assert_eq!(
        code(read_page(Cursor::new("a"), 1, 2001, 1000).unwrap_err()),
        "INVALID_ARGUMENT"
    );
}

#[test]
fn read_caps_lines_and_bytes_without_cutting_utf8_or_partial_lines() {
    let bytes = "中文\n".repeat(10000);
    let result = read_page(Cursor::new(&bytes), 1, 2000, 100000).unwrap();
    assert_eq!(result["returned_lines"], 2000);
    assert_eq!(result["next_offset"], 2001);
    let line = format!("{}\n", "中".repeat(5000));
    let bytes = line.repeat(10);
    let result = read_page(Cursor::new(bytes), 1, 2000, 1000000).unwrap();
    assert_eq!(result["returned_lines"], 3);
    assert_eq!(result["next_offset"], 4);
    assert_eq!(result["truncated_by"], "output_bytes");
    assert!(result["content"].as_str().unwrap().len() <= MAX_BYTES);
    let long = read_page(Cursor::new("x".repeat(MAX_BYTES + 1)), 1, 2000, 100000).unwrap();
    assert_eq!(long["first_line_exceeds_limit"], true);
    assert_eq!(long["next_offset"], json!(null));
    assert_eq!(long["content"], "");
}

#[test]
fn read_streams_and_enforces_scan_budget_without_partial_results() {
    // A read that loads the whole input would hit the injected I/O failure.
    struct FailsAfterPrefix {
        prefix: Cursor<Vec<u8>>,
    }
    impl std::io::Read for FailsAfterPrefix {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.prefix.position() as usize == self.prefix.get_ref().len() {
                return Err(std::io::Error::other("must not scan entire file"));
            }
            std::io::Read::read(&mut self.prefix, out)
        }
    }
    let reader = FailsAfterPrefix {
        prefix: Cursor::new(b"a\nb\nc\n".to_vec()),
    };
    assert_eq!(
        read_page(BufReader::with_capacity(2, reader), 1, 1, 100).unwrap()["content"],
        "a\n"
    );
    assert_eq!(
        code(read_page(Cursor::new("abcdef\n"), 1, 10, 3).unwrap_err()),
        "READ_SCAN_LIMIT"
    );
    assert_eq!(
        code(read_page(Cursor::new("a\nb\nc\n"), 3, 10, 2).unwrap_err()),
        "READ_SCAN_LIMIT"
    );
}

#[test]
fn read_rejects_binary_and_invalid_utf8() {
    for bytes in [vec![0, 1], vec![0xff]] {
        assert_eq!(
            code(read_page(Cursor::new(bytes), 1, 10, 100).unwrap_err()),
            "UNSUPPORTED_TEXT"
        );
    }
}

#[test]
fn edits_match_original_and_commit_multiple_disjoint_changes() {
    let result = edit_text(
        "port=80\ndebug=true\n",
        &edits(&[("port=80", "port=90"), ("debug=true", "debug=false")]),
        1000,
    )
    .unwrap();
    assert_eq!(result.content, "port=90\ndebug=false\n");
    assert_eq!(result.response["applied_edits"], 2);
    assert!(result.response["diff"]
        .as_str()
        .unwrap()
        .contains("+port=90"));
    assert_eq!(
        code(
            edit_text("a b", &edits(&[("a", "c"), ("c", "d")]), 100)
                .err()
                .unwrap()
        ),
        "EDIT_NOT_FOUND"
    );
}

#[test]
fn edits_reject_missing_ambiguous_overlapping_empty_and_unchanged_input() {
    for (text, pairs, expected) in [
        ("abc", vec![("xyz", "x")], "EDIT_NOT_FOUND"),
        ("abc abc", vec![("abc", "x")], "EDIT_AMBIGUOUS"),
        ("abc", vec![("ab", "x"), ("bc", "y")], "EDIT_OVERLAP"),
        ("abc", vec![("", "x")], "INVALID_ARGUMENT"),
        ("abc", vec![("abc", "abc")], "EDIT_NO_CHANGE"),
        ("a  \na\n", vec![("a  ", "b")], "EDIT_AMBIGUOUS"),
    ] {
        assert_eq!(
            code(edit_text(text, &edits(&pairs), 1000).err().unwrap()),
            expected
        );
    }
}

#[test]
fn edits_handle_bom_crlf_and_fuzzy_matches_without_rewriting_unchanged_lines() {
    let result = edit_text(
        "\u{feff}keep “quotes”  \r\nvalue = “old”;\r\n",
        &edits(&[("value = \"old\";", "value = \"new\";")]),
        1000,
    )
    .unwrap();
    assert_eq!(
        result.content,
        "\u{feff}keep “quotes”  \r\nvalue = \"new\";\r\n"
    );
    assert_eq!(result.response["used_fuzzy_match"], true);
    let result = edit_text("a\r\nb\r\n", &edits(&[("a\nb", "x\ny")]), 100).unwrap();
    assert_eq!(result.content, "x\r\ny\r\n");
    assert_eq!(
        edit_text("delete me\nkeep", &edits(&[("delete me\n", "")]), 100)
            .unwrap()
            .content,
        "keep"
    );
}

#[test]
fn edits_enforce_final_size_and_bound_diff_output() {
    assert_eq!(
        code(edit_text("a", &edits(&[("a", "large")]), 2).err().unwrap()),
        "PAYLOAD_TOO_LARGE"
    );
    let result = edit_text("a", &edits(&[("a", &"中\n".repeat(20000))]), 100000).unwrap();
    assert!(result.response["diff"].as_str().unwrap().len() <= MAX_BYTES);
    assert_eq!(result.response["diff_truncated"], true);
}

#[test]
fn list_pages_are_bounded_and_cursors_advance() {
    let entries: Vec<_> = (0..10).map(|n| json!({"name":n.to_string()})).collect();
    let page = file_tools::list_page(entries.clone(), 0, 3).unwrap();
    assert_eq!(page["next_offset"], 3);
    assert_eq!(
        file_tools::list_page(entries.clone(), 9, 3).unwrap()["has_more"],
        false
    );
    assert_eq!(
        file_tools::list_page(entries, u64::MAX, 3).unwrap()["entries"],
        json!([])
    );
    assert_eq!(
        code(file_tools::list_page(vec![json!({"name":"x".repeat(MAX_BYTES)})], 0, 1).unwrap_err()),
        "ENTRY_TOO_LARGE"
    );
}

#[test]
fn edits_are_sensitive_and_payload_validation_happens_before_io() {
    let mut op = Operation {
        kind: "edit_file".into(),
        file_request: Some(FileRequest::Edit {
            edits: edits(&[("a", "b")]),
        }),
        ..Default::default()
    };
    let mut settings = RuntimeSettings::default();
    assert!(security::sensitive(&op, "mcp"));
    settings.remote_read_only = true;
    assert!(security::check(&settings, &op, "mcp").is_err());
    settings.approval_mode = "mcp".into();
    assert!(security::needs_approval(&settings, &op, "mcp"));
    op.file_request = Some(FileRequest::Edit { edits: vec![] });
    assert!(file_tools::validate(&op, &settings).is_err());
}
