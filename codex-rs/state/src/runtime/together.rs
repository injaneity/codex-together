use crate::StateRuntime;
use crate::TogetherClientMode;
use crate::TogetherClientSession;
use crate::TogetherMemberRecord;
use crate::TogetherRole;
use crate::TogetherServerRecord;
use crate::TogetherThreadAclRecord;
use crate::TogetherThreadForkRecord;
use anyhow::Context;
use sqlx::Row;

impl StateRuntime {
    pub async fn upsert_together_server(
        &self,
        record: &TogetherServerRecord,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO together_servers(server_id, owner_email, public_base_url, invite_token, created_at, closed_at)
            VALUES(?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(server_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                public_base_url = excluded.public_base_url,
                invite_token = excluded.invite_token,
                created_at = excluded.created_at,
                closed_at = excluded.closed_at
            "#,
        )
        .bind(&record.server_id)
        .bind(&record.owner_email)
        .bind(&record.public_base_url)
        .bind(&record.invite_token)
        .bind(record.created_at)
        .bind(record.closed_at)
        .execute(self.pool.as_ref())
        .await
        .with_context(|| format!("failed to upsert together server {}", record.server_id))?;
        Ok(())
    }

    pub async fn close_together_server(
        &self,
        server_id: &str,
        closed_at: i64,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE together_servers SET closed_at = ?2 WHERE server_id = ?1")
            .bind(server_id)
            .bind(closed_at)
            .execute(self.pool.as_ref())
            .await
            .with_context(|| format!("failed to close together server {server_id}"))?;
        Ok(())
    }

    pub async fn upsert_together_member(
        &self,
        record: &TogetherMemberRecord,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO together_members(server_id, email, role, added_at, removed_at)
            VALUES(?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(server_id, email) DO UPDATE SET
                role = excluded.role,
                added_at = excluded.added_at,
                removed_at = excluded.removed_at
            "#,
        )
        .bind(&record.server_id)
        .bind(&record.email)
        .bind(record.role.as_sql())
        .bind(record.added_at)
        .bind(record.removed_at)
        .execute(self.pool.as_ref())
        .await
        .with_context(|| {
            format!(
                "failed to upsert together member {} for server {}",
                record.email, record.server_id
            )
        })?;
        Ok(())
    }

    pub async fn list_together_members(
        &self,
        server_id: &str,
    ) -> anyhow::Result<Vec<TogetherMemberRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT server_id, email, role, added_at, removed_at
            FROM together_members
            WHERE server_id = ?1
            ORDER BY added_at ASC
            "#,
        )
        .bind(server_id)
        .fetch_all(self.pool.as_ref())
        .await
        .with_context(|| format!("failed to list together members for server {server_id}"))?;

        rows.into_iter().map(row_to_together_member).collect()
    }

    pub async fn upsert_together_thread_acl(
        &self,
        record: &TogetherThreadAclRecord,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO together_thread_acl(server_id, thread_id, owner_email, shared_by_email, shared_at)
            VALUES(?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(server_id, thread_id) DO UPDATE SET
                owner_email = excluded.owner_email,
                shared_by_email = excluded.shared_by_email,
                shared_at = excluded.shared_at
            "#,
        )
        .bind(&record.server_id)
        .bind(&record.thread_id)
        .bind(&record.owner_email)
        .bind(&record.shared_by_email)
        .bind(record.shared_at)
        .execute(self.pool.as_ref())
        .await
        .with_context(|| {
            format!(
                "failed to upsert together thread acl {} for server {}",
                record.thread_id, record.server_id
            )
        })?;
        Ok(())
    }

    pub async fn insert_together_thread_fork(
        &self,
        record: &TogetherThreadForkRecord,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO together_thread_forks(server_id, child_thread_id, parent_thread_id, actor_email, created_at)
            VALUES(?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(server_id, child_thread_id) DO UPDATE SET
                parent_thread_id = excluded.parent_thread_id,
                actor_email = excluded.actor_email,
                created_at = excluded.created_at
            "#,
        )
        .bind(&record.server_id)
        .bind(&record.child_thread_id)
        .bind(&record.parent_thread_id)
        .bind(&record.actor_email)
        .bind(record.created_at)
        .execute(self.pool.as_ref())
        .await
        .with_context(|| {
            format!(
                "failed to insert together thread fork {} for server {}",
                record.child_thread_id, record.server_id
            )
        })?;
        Ok(())
    }

    pub async fn list_together_thread_forks(
        &self,
        server_id: &str,
    ) -> anyhow::Result<Vec<TogetherThreadForkRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT server_id, child_thread_id, parent_thread_id, actor_email, created_at
            FROM together_thread_forks
            WHERE server_id = ?1
            ORDER BY created_at ASC
            "#,
        )
        .bind(server_id)
        .fetch_all(self.pool.as_ref())
        .await
        .with_context(|| format!("failed to list together thread forks for server {server_id}"))?;

        rows.into_iter().map(row_to_together_thread_fork).collect()
    }

    pub async fn upsert_together_client_session(
        &self,
        session: &TogetherClientSession,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO together_client_session(
                id, mode, server_id, owner_email, endpoint, checked_out_thread_id, host_pid, updated_at, created_at
            )
            VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(id) DO UPDATE SET
                mode = excluded.mode,
                server_id = excluded.server_id,
                owner_email = excluded.owner_email,
                endpoint = excluded.endpoint,
                checked_out_thread_id = excluded.checked_out_thread_id,
                host_pid = excluded.host_pid,
                updated_at = excluded.updated_at,
                created_at = excluded.created_at
            "#,
        )
        .bind(session.mode.as_sql())
        .bind(&session.server_id)
        .bind(&session.owner_email)
        .bind(&session.endpoint)
        .bind(&session.checked_out_thread_id)
        .bind(session.host_pid)
        .bind(session.updated_at)
        .bind(session.created_at)
        .execute(self.pool.as_ref())
        .await
        .context("failed to upsert together client session")?;
        Ok(())
    }

    pub async fn get_together_client_session(
        &self,
    ) -> anyhow::Result<Option<TogetherClientSession>> {
        let row = sqlx::query(
            r#"
            SELECT mode, server_id, owner_email, endpoint, checked_out_thread_id, host_pid, updated_at, created_at
            FROM together_client_session
            WHERE id = 1
            "#,
        )
        .fetch_optional(self.pool.as_ref())
        .await
        .context("failed to fetch together client session")?;

        row.map(row_to_together_client_session).transpose()
    }
}

fn row_to_together_member(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<TogetherMemberRecord> {
    let role = parse_role(row.try_get("role")?)?;
    Ok(TogetherMemberRecord {
        server_id: row.try_get("server_id")?,
        email: row.try_get("email")?,
        role,
        added_at: row.try_get("added_at")?,
        removed_at: row.try_get("removed_at")?,
    })
}

fn row_to_together_thread_fork(
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<TogetherThreadForkRecord> {
    Ok(TogetherThreadForkRecord {
        server_id: row.try_get("server_id")?,
        child_thread_id: row.try_get("child_thread_id")?,
        parent_thread_id: row.try_get("parent_thread_id")?,
        actor_email: row.try_get("actor_email")?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_together_client_session(
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<TogetherClientSession> {
    Ok(TogetherClientSession {
        mode: parse_mode(row.try_get("mode")?)?,
        server_id: row.try_get("server_id")?,
        owner_email: row.try_get("owner_email")?,
        endpoint: row.try_get("endpoint")?,
        checked_out_thread_id: row.try_get("checked_out_thread_id")?,
        host_pid: row.try_get("host_pid")?,
        updated_at: row.try_get("updated_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn parse_role(role: &str) -> anyhow::Result<TogetherRole> {
    match role {
        "owner" => Ok(TogetherRole::Owner),
        "member" => Ok(TogetherRole::Member),
        _ => anyhow::bail!("invalid together role in db: {role}"),
    }
}

fn parse_mode(mode: &str) -> anyhow::Result<TogetherClientMode> {
    match mode {
        "disconnected" => Ok(TogetherClientMode::Disconnected),
        "host" => Ok(TogetherClientMode::Host),
        "member" => Ok(TogetherClientMode::Member),
        _ => anyhow::bail!("invalid together mode in db: {mode}"),
    }
}
