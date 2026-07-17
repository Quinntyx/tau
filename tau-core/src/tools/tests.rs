use std::fs;

use serde_json::json;
use tempfile::tempdir;

use super::hashline::{anchor_hash, line_hash, parse_ref, render_directory, render_file};
use super::*;

#[test]
fn default_limits_match_m4_contract() {
    let limits = ToolLimits::default();
    assert_eq!(limits.read_lines, 2_000);
    assert_eq!(limits.read_bytes, 50 * 1024);
    assert_eq!(limits.max_line_chars, 2_000);
    assert_eq!(limits.search_matches, 100);
    assert_eq!(limits.glob_results, 100);
    assert_eq!(limits.directory_entries, 2_000);
    assert_eq!(limits.binary_bytes, 50 * 1024);
}

#[test]
fn hashline_render_and_parse_are_stable() {
    let raw = "one\r\ntwo\r\n";
    let rendered =
        render_file(raw, std::path::Path::new("x.txt"), 1, 10, 2_000, 50 * 1024).unwrap();
    assert_eq!(
        rendered.rev,
        super::hashline::compute_file_rev("one\ntwo\n")
    );
    assert!(
        rendered
            .content
            .contains(&format!("#HL REV:{}", rendered.rev))
    );
    let reference = rendered
        .content
        .lines()
        .find(|line| line.starts_with("#HL 1#"))
        .unwrap();
    let parsed = parse_ref(reference).unwrap();
    assert_eq!(parsed.line, 1);
    assert_eq!(parsed.hash, line_hash("one", 3));
    assert_eq!(
        parsed.anchor,
        Some(anchor_hash(None, "one", Some("two"), 3))
    );
}

#[test]
fn directory_hashlines_keep_ids_separate_from_line_numbers() {
    let entries = vec![
        DirectoryEntry {
            id: 1,
            name: "alpha".into(),
            kind: EntryKind::Directory,
        },
        DirectoryEntry {
            id: 2,
            name: "beta.txt".into(),
            kind: EntryKind::File,
        },
    ];
    let rendered = render_directory(&entries, std::path::Path::new("."), 1, 10);
    assert!(rendered.content.contains("|01|alpha/"));
    assert!(rendered.content.contains("|02|beta.txt"));
    assert!(rendered.content.contains("#HL REV:"));
}

#[test]
fn policy_requires_approval_outside_registered_roots() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let context = ToolContext::new(root.path()).unwrap();
    let error = context
        .policy
        .resolve(root.path(), outside.path(), "read")
        .unwrap_err();
    assert!(matches!(error, ToolError::ApprovalNeeded { .. }));
    context.policy.register_root(outside.path()).unwrap();
    assert!(
        context
            .policy
            .resolve(root.path(), outside.path(), "read")
            .is_ok()
    );
}

#[test]
fn filesystem_tools_match_expected_search_behavior() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join("src/a.rs"), "needle\nother\n").unwrap();
    fs::write(dir.path().join("src/b.txt"), "needle\n").unwrap();
    fs::write(dir.path().join(".hidden.rs"), "needle\n").unwrap();
    fs::write(dir.path().join(".git/config"), "needle\n").unwrap();
    fs::write(dir.path().join("image.bin"), [0, 1, 2, 3]).unwrap();
    let context = ToolContext::new(dir.path()).unwrap();

    let read = ReadTool
        .execute(
            ReadInput {
                file_path: "src/a.rs".into(),
                offset: None,
                limit: None,
            },
            &context,
        )
        .unwrap();
    assert!(matches!(read, ReadOutput::File(_)));
    assert!(ReadTool.render(&read).contains("#HL REV:"));

    let binary = ReadTool
        .execute(
            ReadInput {
                file_path: "image.bin".into(),
                offset: None,
                limit: None,
            },
            &context,
        )
        .unwrap();
    assert!(matches!(binary, ReadOutput::Binary(_)));

    let list = ListTool
        .execute(
            ListInput {
                path: Some("src".into()),
            },
            &context,
        )
        .unwrap();
    assert_eq!(
        list.entries
            .iter()
            .map(|entry| &entry.name)
            .collect::<Vec<_>>(),
        [&"a.rs".to_string(), &"b.txt".to_string()]
    );

    let glob = GlobTool
        .execute(
            GlobInput {
                pattern: "**/*.rs".into(),
                path: None,
            },
            &context,
        )
        .unwrap();
    assert_eq!(glob.entries, vec![std::path::PathBuf::from("src/a.rs")]);

    let grep = GrepTool
        .execute(
            GrepInput {
                pattern: "needle".into(),
                path: None,
                include: Some("**/*.rs".into()),
            },
            &context,
        )
        .unwrap();
    assert_eq!(grep.matches.len(), 2);
    assert!(
        grep.matches
            .iter()
            .any(|item| item.path.ends_with("src/a.rs"))
    );
    assert!(
        grep.matches
            .iter()
            .any(|item| item.path.ends_with(".hidden.rs"))
    );
    assert!(
        !grep
            .matches
            .iter()
            .any(|item| item.path.ends_with(".git/config"))
    );
}

#[test]
fn registry_dispatches_typed_tools_and_rejects_duplicates() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    let context = ToolContext::new(dir.path()).unwrap();
    let mut registry = ToolRegistry::default();
    registry.register(ReadTool).unwrap();
    assert!(matches!(
        registry.register(ReadTool),
        Err(ToolError::DuplicateTool(_))
    ));
    let result = registry
        .execute("read", json!({"file_path": "a.txt"}), &context)
        .unwrap();
    assert!(result.rendered.contains("#HL REV:"));
    assert!(registry.execute("missing", json!({}), &context).is_err());
}
