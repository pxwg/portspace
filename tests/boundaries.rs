use portspace::{
    local::{FILE_LIMIT, LocalProvider},
    *,
};
use std::{collections::BTreeMap, sync::Arc};

#[tokio::test]
async fn explicit_limits_and_invalid_inputs() {
    let root = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(root.path()).unwrap();
    std::fs::write(root.path().join("large"), vec![b'x'; FILE_LIMIT + 1]).unwrap();
    for operation in [
        Operation::Read {
            path: "large".into(),
            encoding: Encoding::Utf8,
        },
        Operation::Write {
            path: "new".into(),
            content: "x".repeat(FILE_LIMIT + 1),
            encoding: Encoding::Utf8,
        },
    ] {
        assert_eq!(
            provider.execute(operation).await.unwrap_err().code,
            ErrorCode::LimitExceeded
        );
    }
    assert!(!root.path().join("new").exists());
    for operation in [
        Operation::Write {
            path: "new".into(),
            content: "!bad".into(),
            encoding: Encoding::Base64,
        },
        Operation::Search {
            path: ".".into(),
            pattern: "[".into(),
            limit: 1,
        },
        Operation::Search {
            path: ".".into(),
            pattern: "x".into(),
            limit: 0,
        },
        Operation::Exec {
            program: "/bin/true".into(),
            args: vec![],
            cwd: ".".into(),
            env: BTreeMap::new(),
            timeout_ms: 0,
        },
    ] {
        assert_eq!(
            provider.execute(operation).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
    assert!(
        serde_json::from_value::<Request>(serde_json::json!({
            "workspace":"main", "operation":{"op":"stat","path":".","extra":true}
        }))
        .is_err()
    );
}

#[tokio::test]
async fn independent_workspaces_are_not_blocked_and_cancelled_lock_recovers() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::default();
    runtime
        .register("a".into(), Arc::new(LocalProvider::new(a.path()).unwrap()))
        .unwrap();
    runtime
        .register("b".into(), Arc::new(LocalProvider::new(b.path()).unwrap()))
        .unwrap();
    assert!(
        runtime
            .register("a".into(), Arc::new(LocalProvider::new(b.path()).unwrap()))
            .is_err()
    );
    let runtime = Arc::new(runtime);
    let worker = runtime.clone();
    let task = tokio::spawn(async move {
        worker
            .execute(Request {
                workspace: "a".into(),
                operation: Operation::Exec {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "touch ready; sleep 10".into()],
                    cwd: ".".into(),
                    env: BTreeMap::new(),
                    timeout_ms: 20000,
                },
            })
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !a.path().join("ready").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let request = |id: &str| Request {
        workspace: id.into(),
        operation: Operation::Stat { path: ".".into() },
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.execute(request("b")),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            runtime.execute(request("a"))
        )
        .await
        .is_err()
    );
    task.abort();
    let _ = task.await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.execute(request("a")),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn search_skips_symlinks_binary_and_oversized_files() {
    let root = tempfile::tempdir().unwrap();
    let provider = LocalProvider::new(root.path()).unwrap();
    std::os::unix::fs::symlink(root.path(), root.path().join("loop")).unwrap();
    std::fs::write(root.path().join("binary"), [0xff]).unwrap();
    std::fs::write(root.path().join("large"), vec![b'x'; FILE_LIMIT + 1]).unwrap();
    std::fs::write(root.path().join("text"), "needle").unwrap();
    let result = provider
        .execute(Operation::Search {
            path: ".".into(),
            pattern: "needle".into(),
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    assert_eq!(result["skipped_files"], 2);
    assert_eq!(result["truncated"], false);
}
