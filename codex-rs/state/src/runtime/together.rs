use crate::StateRuntime;
use crate::TogetherClientSession;
use crate::TogetherServerRecord;
use anyhow::Context;

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

    pub async fn upsert_together_client_session(
        &self,
        session: &TogetherClientSession,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO together_client_session(
                id, mode, server_id, owner_email, endpoint, updated_at, created_at
            )
            VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                mode = excluded.mode,
                server_id = excluded.server_id,
                owner_email = excluded.owner_email,
                endpoint = excluded.endpoint,
                updated_at = excluded.updated_at,
                created_at = excluded.created_at
            "#,
        )
        .bind(session.mode.as_sql())
        .bind(&session.server_id)
        .bind(&session.owner_email)
        .bind(&session.endpoint)
        .bind(session.updated_at)
        .bind(session.created_at)
        .execute(self.pool.as_ref())
        .await
        .context("failed to upsert together client session")?;
        Ok(())
    }
}
