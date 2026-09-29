use async_trait::async_trait;
use portspace::{
    Operation, Provider, Result, Runtime, WorkspaceInfo,
    local::LocalProvider,
    profiles::claude::{self, EditInput, ReadInput, State},
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
struct DelayedRead(LocalProvider);
#[async_trait]
impl Provider for DelayedRead {
    fn info(&self, id: &str) -> WorkspaceInfo {
        self.0.info(id)
    }
    async fn execute(&self, op: Operation) -> Result<Value> {
        let read = matches!(op, Operation::Read { .. });
        let value = self.0.execute(op).await?;
        if read {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(value)
    }
}
fn edit(old: &str, new: &str) -> EditInput {
    EditInput {
        file_path: "file".into(),
        old_string: old.into(),
        new_string: new.into(),
        replace_all: false,
    }
}
async fn read(runtime: &Runtime, state: &State) {
    claude::read(
        runtime,
        "main",
        state,
        ReadInput {
            file_path: "file".into(),
            offset: None,
            limit: None,
            pages: None,
        },
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn compound_lock_refreshes_shared_state_and_rejects_stale_other_session() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "alpha beta").unwrap();
    let mut runtime = Runtime::default();
    runtime
        .register(
            "main".into(),
            Arc::new(DelayedRead(LocalProvider::new(dir.path()).unwrap())),
        )
        .unwrap();
    let state = State::default();
    let other = State::default();
    read(&runtime, &state).await;
    read(&runtime, &other).await;
    let (a, b) = tokio::join!(
        claude::edit(&runtime, "main", &state, edit("alpha", "ALPHA")),
        claude::edit(&runtime, "main", &state, edit("beta", "BETA"))
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file")).unwrap(),
        "ALPHA BETA"
    );
    assert!(
        claude::edit(&runtime, "main", &other, edit("ALPHA", "stale"))
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(2),
            claude::edit(&runtime, "main", &state, edit("ALPHA", "cancelled"))
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file")).unwrap(),
        "ALPHA BETA"
    );
    claude::edit(&runtime, "main", &state, edit("ALPHA", "next"))
        .await
        .unwrap();
}
