use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use tempfile::tempdir;

use super::hashline::{anchor_hash, line_hash, parse_ref, render_directory, render_file};
use super::*;

fn local_fixture(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("m5-fixtures")
        .join(format!("{name}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

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

#[test]
fn builtin_registry_exposes_all_m4_tools() {
    let registry = ToolRegistry::with_builtins().unwrap();
    let names = registry
        .descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["bash", "edit", "glob", "grep", "list", "read", "write"]
    );
}

#[test]
fn edit_validates_hashlines_and_preserves_crlf_and_bom() {
    let root = local_fixture("edit-file");
    let path = root.join("file.txt");
    fs::write(&path, b"\xef\xbb\xbfone\r\ntwo\r\n").unwrap();
    let context = ToolContext::new(&root).unwrap();
    let read = match ReadTool
        .execute(
            ReadInput {
                file_path: "file.txt".into(),
                offset: None,
                limit: None,
            },
            &context,
        )
        .unwrap()
    {
        ReadOutput::File(read) => read,
        _ => panic!("expected text file"),
    };
    let reference = read
        .rendered
        .lines()
        .find(|line| line.starts_with("#HL 1#"))
        .unwrap()
        .split('|')
        .next()
        .unwrap()
        .to_string();
    EditTool
        .execute(
            EditInput {
                path: "file.txt".into(),
                content: Some("#HL REV:ignored\n#HL 1#ignored#ignored|updated".into()),
                reference: Some(reference),
                start_ref: None,
                end_ref: None,
                file_rev: Some(read.rev),
                safe_reapply: None,
                operations: None,
            },
            &context,
        )
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"\xef\xbb\xbfupdated\r\ntwo\r\n");
}

#[test]
fn edit_rejects_stale_revision_and_supports_directory_operations() {
    let root = local_fixture("edit-directory");
    let directory = root.join("workspace");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("old.txt"), "old").unwrap();
    let context = ToolContext::new(&root).unwrap();
    let read = match ReadTool
        .execute(
            ReadInput {
                file_path: "workspace".into(),
                offset: None,
                limit: None,
            },
            &context,
        )
        .unwrap()
    {
        ReadOutput::Directory(read) => read,
        _ => panic!("expected directory"),
    };
    let reference = read
        .rendered
        .lines()
        .find(|line| line.contains("|01|old.txt"))
        .unwrap()
        .split('|')
        .next()
        .unwrap()
        .to_string();
    EditTool
        .execute(
            EditInput {
                path: "workspace".into(),
                content: None,
                reference: None,
                start_ref: None,
                end_ref: None,
                file_rev: Some(read.rev.clone()),
                safe_reapply: None,
                operations: Some(vec![EditOperation {
                    op: Some("rename".into()),
                    reference: Some(reference),
                    start_ref: None,
                    end_ref: None,
                    content: None,
                    parent: None,
                    name: Some("new.txt".into()),
                    kind: None,
                }]),
            },
            &context,
        )
        .unwrap();
    assert!(directory.join("new.txt").exists());
    assert!(!directory.join("old.txt").exists());

    fs::write(root.join("stale.txt"), "one\ntwo\n").unwrap();
    let stale = EditTool.execute(
        EditInput {
            path: "stale.txt".into(),
            content: Some("changed".into()),
            reference: Some("#HL 1#000#000".into()),
            start_ref: None,
            end_ref: None,
            file_rev: Some("00000000".into()),
            safe_reapply: None,
            operations: None,
        },
        &context,
    );
    assert!(matches!(stale, Err(ToolError::StaleRevision { .. })));
}

#[test]
fn write_is_atomic_and_bash_reports_classification() {
    let root = local_fixture("write-bash");
    let context = ToolContext::new(&root).unwrap();
    let written = WriteTool
        .execute(
            WriteInput {
                path: "nested/file.txt".into(),
                content: "hello".into(),
            },
            &context,
        )
        .unwrap();
    assert!(!written.existed);
    assert_eq!(
        fs::read_to_string(root.join("nested/file.txt")).unwrap(),
        "hello"
    );
    assert!(root.join(".tau/snapshots").exists());

    let output = BashTool
        .execute(
            BashInput {
                command: "printf 'hello'".into(),
                workdir: None,
                timeout: Some(5),
            },
            &context,
        )
        .unwrap();
    assert_eq!(output.stdout, "hello");
    assert_eq!(output.classification, CommandClass::ReadOnly);
    assert_eq!(
        classify_command("sed -i 's/a/b/' file.txt"),
        CommandClass::PotentialMutation
    );
}
