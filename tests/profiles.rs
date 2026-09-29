use async_trait::async_trait;
use portspace::{
    Operation, Provider, Result, Runtime, WorkspaceInfo,
    local::LocalProvider,
    profiles::pi::{self, Edit, EditInput},
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

// Force a scheduling point after reading. Without one compound-operation lock,
// concurrent edits could both read the old file and overwrite each other's work.
struct DelayedRead(LocalProvider);
#[async_trait]
impl Provider for DelayedRead {
    fn info(&self, id: &str) -> WorkspaceInfo {
        self.0.info(id)
    }
    async fn execute(&self, operation: Operation) -> Result<Value> {
        let read = matches!(operation, Operation::Read { .. });
        let value = self.0.execute(operation).await?;
        if read {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(value)
    }
}
fn edit(old: &str, new: &str) -> EditInput {
    EditInput {
        path: "file".into(),
        edits: vec![Edit {
            old_text: old.into(),
            new_text: new.into(),
        }],
    }
}

#[tokio::test]
async fn profile_edits_hold_workspace_lock_across_read_compute_write() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "alpha beta").unwrap();
    let mut runtime = Runtime::default();
    runtime
        .register(
            "main".into(),
            Arc::new(DelayedRead(LocalProvider::new(dir.path()).unwrap())),
        )
        .unwrap();
    let (a, b) = tokio::join!(
        pi::edit(&runtime, "main", edit("alpha", "ALPHA")),
        pi::edit(&runtime, "main", edit("beta", "BETA")),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file")).unwrap(),
        "ALPHA BETA"
    );
}

#[tokio::test]
async fn dropped_profile_edit_does_not_write_and_releases_lock() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "original").unwrap();
    let mut runtime = Runtime::default();
    runtime
        .register(
            "main".into(),
            Arc::new(DelayedRead(LocalProvider::new(dir.path()).unwrap())),
        )
        .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(2),
            pi::edit(&runtime, "main", edit("original", "cancelled"))
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file")).unwrap(),
        "original"
    );
    pi::edit(&runtime, "main", edit("original", "next"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file")).unwrap(),
        "next"
    );
}
