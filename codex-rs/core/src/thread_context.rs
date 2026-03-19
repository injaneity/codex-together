use std::path::Path;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use chrono::Utc;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadStatus;
use codex_app_server_protocol::build_turns_from_rollout_items;
use codex_context_graph::build_context_query;
use codex_context_graph::repo_context_documents;
use codex_context_graph::thread_context_documents;
use codex_together_protocol::ContextPrecursorKind;
use codex_together_protocol::ContextQueryParams;
use codex_together_protocol::ContextQueryResponse;
use serde::Deserialize;
use serde::Serialize;

use crate::error::CodexErr;
use crate::error::Result as CodexResult;
use crate::find_thread_path_by_id_str;
use crate::git_info::current_branch_name;
use crate::git_info::get_git_repo_root;
use crate::protocol::InitialHistory;
use crate::rollout::RolloutRecorder;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThreadContextMountKind {
    Fork,
    Handoff,
}

impl From<ContextPrecursorKind> for ThreadContextMountKind {
    fn from(value: ContextPrecursorKind) -> Self {
        match value {
            ContextPrecursorKind::Fork => Self::Fork,
            ContextPrecursorKind::Handoff => Self::Handoff,
        }
    }
}

impl From<ThreadContextMountKind> for ContextPrecursorKind {
    fn from(value: ThreadContextMountKind) -> Self {
        match value {
            ThreadContextMountKind::Fork => Self::Fork,
            ThreadContextMountKind::Handoff => Self::Handoff,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadContextMount {
    pub precursor_thread_id: String,
    pub precursor_kind: ThreadContextMountKind,
    pub goal: Option<String>,
    #[serde(default)]
    pub seed_ref_ids: Vec<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LocalContextQueryInput {
    pub codex_home: PathBuf,
    pub cwd: PathBuf,
    pub current_thread_id: Option<String>,
    pub current_rollout_path: Option<PathBuf>,
    pub query: Option<String>,
    pub limit: Option<u32>,
}

pub fn thread_context_mount_sidecar_path(rollout_path: &Path) -> PathBuf {
    let file_name = rollout_path
        .file_name()
        .map(|name| format!("{}.context.json", name.to_string_lossy()))
        .unwrap_or_else(|| "thread.context.json".to_string());
    rollout_path.with_file_name(file_name)
}

pub async fn read_thread_context_mount(
    rollout_path: &Path,
) -> CodexResult<Option<ThreadContextMount>> {
    let path = thread_context_mount_sidecar_path(rollout_path);
    let contents = match tokio::fs::read_to_string(&path).await {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CodexErr::Io(std::io::Error::new(
                err.kind(),
                format!(
                    "failed to read thread context mount {}: {err}",
                    path.display()
                ),
            )));
        }
    };
    serde_json::from_str(&contents).map(Some).map_err(|err| {
        CodexErr::Fatal(format!(
            "failed to parse thread context mount {}: {err}",
            path.display()
        ))
    })
}

pub async fn read_thread_context_mount_for_thread_id(
    codex_home: &Path,
    thread_id: &str,
) -> CodexResult<Option<ThreadContextMount>> {
    let Some(rollout_path) = find_thread_path_by_id_str(codex_home, thread_id).await? else {
        return Ok(None);
    };
    read_thread_context_mount(rollout_path.as_path()).await
}

pub async fn write_thread_context_mount(
    rollout_path: &Path,
    mount: &ThreadContextMount,
) -> CodexResult<()> {
    let path = thread_context_mount_sidecar_path(rollout_path);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(CodexErr::Io)?;
    }
    let contents = serde_json::to_string_pretty(mount).map_err(|err| {
        CodexErr::Fatal(format!(
            "failed to serialize thread context mount {}: {err}",
            path.display()
        ))
    })?;
    tokio::fs::write(&path, contents)
        .await
        .map_err(CodexErr::Io)
}

pub async fn build_local_context_query(
    input: LocalContextQueryInput,
) -> CodexResult<ContextQueryResponse> {
    let repo_root = get_git_repo_root(&input.cwd).unwrap_or_else(|| input.cwd.clone());
    let git_branch = current_branch_name(repo_root.as_path()).await;
    let current_rollout_path = if let Some(rollout_path) = input.current_rollout_path {
        Some(rollout_path)
    } else if let Some(thread_id) = input.current_thread_id.as_deref() {
        find_thread_path_by_id_str(&input.codex_home, thread_id).await?
    } else {
        None
    };
    let mount = if let Some(rollout_path) = current_rollout_path.as_deref() {
        read_thread_context_mount(rollout_path).await?
    } else if let Some(thread_id) = input.current_thread_id.as_deref() {
        read_thread_context_mount_for_thread_id(&input.codex_home, thread_id).await?
    } else {
        None
    };

    let mut documents = repo_context_documents(repo_root.as_path());
    if let Some(thread) = load_thread_from_rollout(
        input.codex_home.as_path(),
        input.current_thread_id.as_deref(),
        current_rollout_path.as_deref(),
        repo_root.as_path(),
        git_branch.clone(),
    )
    .await?
    {
        documents.extend(thread_context_documents(
            &thread,
            true,
            Some(repo_root.as_path()),
        ));
    }
    if let Some(precursor_thread_id) = mount
        .as_ref()
        .map(|mount| mount.precursor_thread_id.as_str())
        && let Some(thread) = load_thread_from_rollout(
            input.codex_home.as_path(),
            Some(precursor_thread_id),
            None,
            repo_root.as_path(),
            git_branch.clone(),
        )
        .await?
    {
        documents.extend(thread_context_documents(
            &thread,
            false,
            Some(repo_root.as_path()),
        ));
    }

    Ok(build_context_query(
        documents,
        &ContextQueryParams {
            current_thread_id: input.current_thread_id,
            precursor_thread_id: mount
                .as_ref()
                .map(|mount| mount.precursor_thread_id.clone()),
            precursor_kind: mount
                .as_ref()
                .map(|mount| ContextPrecursorKind::from(mount.precursor_kind)),
            actor_id: mount.as_ref().and_then(|mount| mount.actor_id.clone()),
            repo_root: Some(repo_root.display().to_string()),
            git_branch,
            goal: mount.as_ref().and_then(|mount| mount.goal.clone()),
            query: input.query,
            seed_ref_ids: mount
                .as_ref()
                .map(|mount| mount.seed_ref_ids.clone())
                .unwrap_or_default(),
            limit: input.limit,
        },
    ))
}

async fn load_thread_from_rollout(
    codex_home: &Path,
    thread_id: Option<&str>,
    rollout_path: Option<&Path>,
    repo_root: &Path,
    git_branch: Option<String>,
) -> CodexResult<Option<Thread>> {
    let rollout_path = if let Some(rollout_path) = rollout_path {
        if !tokio::fs::try_exists(rollout_path)
            .await
            .map_err(CodexErr::Io)?
        {
            return Ok(None);
        }
        rollout_path.to_path_buf()
    } else if let Some(thread_id) = thread_id {
        let Some(rollout_path) = find_thread_path_by_id_str(codex_home, thread_id).await? else {
            return Ok(None);
        };
        rollout_path
    } else {
        return Ok(None);
    };

    let items = match RolloutRecorder::get_rollout_history(rollout_path.as_path())
        .await
        .map_err(CodexErr::Io)?
    {
        InitialHistory::New => Vec::new(),
        InitialHistory::Resumed(history) => history.history,
        InitialHistory::Forked(history) => history,
    };
    let turns = build_turns_from_rollout_items(&items);
    let updated_at = std::fs::metadata(&rollout_path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_else(|| Utc::now().timestamp());
    let thread_id = thread_id
        .map(ToOwned::to_owned)
        .or_else(|| thread_id_from_rollout_path(&rollout_path))
        .unwrap_or_else(|| "local".to_string());

    Ok(Some(Thread {
        id: thread_id,
        preview: String::new(),
        ephemeral: false,
        model_provider: "openai".to_string(),
        created_at: updated_at,
        updated_at,
        status: ThreadStatus::Idle,
        path: Some(rollout_path),
        cwd: repo_root.to_path_buf(),
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        source: codex_app_server_protocol::SessionSource::Cli,
        agent_nickname: None,
        agent_role: None,
        git_info: git_branch.map(|branch| codex_app_server_protocol::GitInfo {
            sha: None,
            branch: Some(branch),
            origin_url: None,
        }),
        name: None,
        turns,
    }))
}

fn thread_id_from_rollout_path(rollout_path: &Path) -> Option<String> {
    let file_name = rollout_path.file_name()?.to_string_lossy();
    file_name
        .strip_prefix("rollout-")?
        .strip_suffix(".jsonl")
        .and_then(|stem| {
            stem.rsplit_once('-')
                .map(|(_, thread_id)| thread_id.to_string())
        })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::ThreadContextMount;
    use super::ThreadContextMountKind;
    use super::read_thread_context_mount;
    use super::thread_context_mount_sidecar_path;
    use super::write_thread_context_mount;

    #[tokio::test]
    async fn thread_context_mount_round_trips_next_to_rollout() {
        let temp_dir = TempDir::new().expect("tempdir");
        let rollout_path = temp_dir.path().join("rollout-test-thread.jsonl");
        tokio::fs::write(&rollout_path, b"")
            .await
            .expect("write rollout");
        let mount = ThreadContextMount {
            precursor_thread_id: "thread-1".to_string(),
            precursor_kind: ThreadContextMountKind::Handoff,
            goal: Some("Continue the work".to_string()),
            seed_ref_ids: vec!["ref-1".to_string(), "ref-2".to_string()],
            actor_id: Some("member@local".to_string()),
        };

        write_thread_context_mount(rollout_path.as_path(), &mount)
            .await
            .expect("write mount");
        let loaded = read_thread_context_mount(rollout_path.as_path())
            .await
            .expect("read mount");

        assert_eq!(loaded, Some(mount));
        assert_eq!(
            thread_context_mount_sidecar_path(rollout_path.as_path()),
            temp_dir
                .path()
                .join("rollout-test-thread.jsonl.context.json")
        );
    }
}
