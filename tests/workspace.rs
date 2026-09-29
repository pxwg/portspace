use portspace::{
    local::{LocalProvider, replace_exact},
    *,
};
use std::{collections::BTreeMap, sync::Arc};
fn edit(old: &str, new: &str) -> Replacement {
    Replacement {
        old_text: old.into(),
        new_text: new.into(),
    }
}
#[test]
fn exact_edits_are_original_based_and_preserve_crlf() {
    assert_eq!(
        replace_exact("a\r\nb\r\n", &[edit("a", "b"), edit("b", "c")]).unwrap(),
        "b\r\nc\r\n"
    );
    assert_eq!(
        replace_exact("aaa", &[edit("aa", "x")]).unwrap_err().code,
        ErrorCode::AmbiguousEdit
    );
    assert_eq!(
        replace_exact("abc", &[edit("ab", "x"), edit("bc", "y")])
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        replace_exact("abc", &[edit("d", "x")]).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        replace_exact("abc", &[edit("", "x")]).unwrap_err().code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        replace_exact("你好", &[edit("好", "世界")]).unwrap(),
        "你世界"
    );
}
#[tokio::test]
async fn filesystem_process_and_workspace_isolation() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::default();
    runtime
        .register("a".into(), Arc::new(LocalProvider::new(a.path()).unwrap()))
        .unwrap();
    runtime
        .register("b".into(), Arc::new(LocalProvider::new(b.path()).unwrap()))
        .unwrap();
    let req = |workspace: &str, operation| Request {
        workspace: workspace.into(),
        operation,
    };
    runtime
        .execute(req(
            "a",
            Operation::Write {
                path: "hello".into(),
                content: "before\r\n".into(),
                encoding: Encoding::Utf8,
            },
        ))
        .await
        .unwrap();
    runtime
        .execute(req(
            "a",
            Operation::Edit {
                path: "hello".into(),
                replacements: vec![edit("before", "after")],
            },
        ))
        .await
        .unwrap();
    let out = runtime
        .execute(req(
            "a",
            Operation::Exec {
                program: "/bin/cat".into(),
                args: vec!["hello".into()],
                cwd: ".".into(),
                env: BTreeMap::new(),
                timeout_ms: 1000,
            },
        ))
        .await
        .unwrap();
    assert_eq!(out["stdout"], "after\r\n");
    assert_eq!(
        runtime
            .execute(req(
                "b",
                Operation::Stat {
                    path: "hello".into()
                }
            ))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        runtime
            .execute(req("missing", Operation::Stat { path: ".".into() }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::WorkspaceUnavailable
    );
    assert_eq!(runtime.workspaces().len(), 2);
}
#[tokio::test]
async fn path_policy_and_binary() {
    let root = tempfile::tempdir().unwrap();
    let p = LocalProvider::new(root.path()).unwrap();
    for path in ["../outside", "/tmp", "foo/../../escape", ""] {
        assert_eq!(
            p.execute(Operation::Stat { path: path.into() })
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidPath
        );
    }
    std::os::unix::fs::symlink("/tmp", root.path().join("link")).unwrap();
    assert_eq!(
        p.execute(Operation::Write {
            path: "link/test".into(),
            content: "bad".into(),
            encoding: Encoding::Utf8
        })
        .await
        .unwrap_err()
        .code,
        ErrorCode::InvalidPath
    );
    p.execute(Operation::Write {
        path: "binary".into(),
        content: "AP8=".into(),
        encoding: Encoding::Base64,
    })
    .await
    .unwrap();
    assert_eq!(
        p.execute(Operation::Read {
            path: "binary".into(),
            encoding: Encoding::Base64
        })
        .await
        .unwrap()["content"],
        "AP8="
    );
    assert_eq!(
        p.execute(Operation::Read {
            path: "binary".into(),
            encoding: Encoding::Utf8
        })
        .await
        .unwrap_err()
        .code,
        ErrorCode::InvalidInput
    );
    assert!(
        p.execute(Operation::Remove {
            path: ".".into(),
            recursive: true
        })
        .await
        .is_err()
    );
}
#[tokio::test]
async fn failed_edit_does_not_mutate_and_write_preserves_mode() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let p = LocalProvider::new(root.path()).unwrap();
    std::fs::write(root.path().join("f"), "abc").unwrap();
    std::fs::set_permissions(
        root.path().join("f"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(
        p.execute(Operation::Edit {
            path: "f".into(),
            replacements: vec![edit("a", "z"), edit("missing", "x")]
        })
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("f")).unwrap(),
        "abc"
    );
    p.execute(Operation::Edit {
        path: "f".into(),
        replacements: vec![edit("a", "z")],
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::metadata(root.path().join("f"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
fn shell(script: &str, timeout_ms: u64) -> Operation {
    Operation::Exec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: ".".into(),
        env: BTreeMap::new(),
        timeout_ms,
    }
}
#[tokio::test]
async fn process_status_timeout_and_capture() {
    let root = tempfile::tempdir().unwrap();
    let p = LocalProvider::new(root.path()).unwrap();
    let result = p
        .execute(shell("printf out; printf err >&2; exit 7", 1000))
        .await
        .unwrap();
    assert_eq!(result["exit_code"], 7);
    assert_eq!(result["stdout"], "out");
    assert_eq!(result["stderr"], "err");
    assert_eq!(
        p.execute(shell("sleep 10", 30)).await.unwrap_err().code,
        ErrorCode::Timeout
    );
    let result = p
        .execute(shell("head -c 2000000 /dev/zero", 3000))
        .await
        .unwrap();
    assert_eq!(result["stdout_truncated"], true);
    assert_eq!(result["stdout"].as_str().unwrap().len(), 1048576);
}
#[tokio::test]
async fn cancellation_kills_descendants() {
    let root = tempfile::tempdir().unwrap();
    let p = LocalProvider::new(root.path()).unwrap();
    let task = tokio::spawn(async move {
        p.execute(shell("(sleep 1; touch leaked) & wait", 5000))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    task.abort();
    let _ = task.await;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(!root.path().join("leaked").exists());
}
#[tokio::test]
async fn search_limits_and_directory_operations() {
    let root = tempfile::tempdir().unwrap();
    let p = LocalProvider::new(root.path()).unwrap();
    p.execute(Operation::Mkdir {
        path: "src/nested".into(),
    })
    .await
    .unwrap();
    std::fs::write(root.path().join("src/f"), "one\ntwo\none\n").unwrap();
    let result = p
        .execute(Operation::Search {
            path: "src".into(),
            pattern: "^one$".into(),
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(result["truncated"], true);
    assert_eq!(result["matches"][0]["path"], "src/f");
    assert_eq!(result["matches"][0]["line"], 1);
    assert_eq!(
        p.execute(Operation::List { path: "src".into() })
            .await
            .unwrap()["entries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    p.execute(Operation::Remove {
        path: "src".into(),
        recursive: true,
    })
    .await
    .unwrap();
    assert!(!root.path().join("src").exists());
}
