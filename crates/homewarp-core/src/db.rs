use std::{path::Path, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

/// Opens the database file, creating it if need be, and brings its schema up to date.
pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use sqlx::{
        SqlitePool,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };

    /// The schema as it was before a home could have several VPSes.
    const BEFORE: [&str; 13] = [
        include_str!("../migrations/0001_users_and_sessions.sql"),
        include_str!("../migrations/0002_templates.sql"),
        include_str!("../migrations/0003_servers.sql"),
        include_str!("../migrations/0004_gate.sql"),
        include_str!("../migrations/0005_enrolment.sql"),
        include_str!("../migrations/0006_accounts_and_audit.sql"),
        include_str!("../migrations/0007_backups_and_schedules.sql"),
        include_str!("../migrations/0008_settings_and_traffic.sql"),
        include_str!("../migrations/0009_two_steps.sql"),
        include_str!("../migrations/0010_passkeys.sql"),
        include_str!("../migrations/0011_sleep.sql"),
        include_str!("../migrations/0012_backup_store.sql"),
        include_str!("../migrations/0013_says_offline.sql"),
    ];
    const SEVERAL: &str = include_str!("../migrations/0014_gates.sql");
    const WEBHOOKS: &str = include_str!("../migrations/0015_webhooks.sql");

    /// A database as an older Homewarp left it, with two servers in it.
    async fn older(files: &tempfile::TempDir) -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(files.path().join("homewarp.db"))
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        for migration in BEFORE {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        sqlx::raw_sql(
            "INSERT INTO templates (id, name, definition, source, created_at)
             VALUES (1, 'Paper', '{}', '{}', 0);
             INSERT INTO servers
                 (id, uuid, name, template_id, image, memory_mb, cpu_percent, port, variables,
                  eula, created_at)
             VALUES (1, 'a', 'Survival', 1, 'java', 1024, 0, 25565, '[]', 0, 0),
                    (2, 'b', 'Creative', 1, 'java', 1024, 0, 25566, '[]', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn servers(pool: &SqlitePool) -> Vec<(String, Option<i64>)> {
        sqlx::query_as("SELECT name, gate_id FROM servers ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_homewarp_with_a_vps_comes_through_with_it_as_the_first_of_several() {
        let files = tempfile::tempdir().unwrap();
        let pool = older(&files).await;
        sqlx::raw_sql(
            "INSERT INTO gate
                 (id, address, wg_port, api_port, private_key, gate_public_key, preshared_key,
                  token, mode, checked, note, created_at)
             VALUES (1, '203.0.113.10', 51999, 4857, 'home', 'gate', 'shared', 'token', 'nat',
                     'hidden', 'said once', 5);
             INSERT INTO traffic (port, protocol, hour, bytes)
             VALUES (25565, 'tcp', 100, 7), (25565, 'udp', 100, 3);",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(SEVERAL).execute(&pool).await.unwrap();

        // The one VPS, on the tunnel it had, with everything that was known of it.
        type Gate = (
            i64,
            i64,
            String,
            String,
            i64,
            String,
            String,
            String,
            String,
        );
        let gates: Vec<Gate> = sqlx::query_as(
            "SELECT id, tunnel, name, address, wg_port, private_key, mode, checked, note FROM gates",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let text = str::to_owned;
        assert_eq!(
            gates,
            [(
                1,
                0,
                text("203.0.113.10"),
                text("203.0.113.10"),
                51999,
                text("home"),
                text("nat"),
                text("hidden"),
                text("said once"),
            )]
        );
        // Its servers are reached through it, as they were.
        assert_eq!(
            servers(&pool).await,
            [(text("Survival"), Some(1)), (text("Creative"), Some(1))]
        );
        // And what it counted is still counted, as its own.
        let traffic: Vec<(i64, i64, String, i64)> =
            sqlx::query_as("SELECT gate_id, port, protocol, bytes FROM traffic ORDER BY protocol")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            traffic,
            [(1, 25565, text("tcp"), 7), (1, 25565, text("udp"), 3)]
        );
        // A second VPS has room beside it, and not on the same tunnel.
        let second = "INSERT INTO gates
                 (tunnel, name, address, wg_port, api_port, private_key, gate_public_key,
                  preshared_key, token, mode, created_at)
             VALUES (?, 'Second', '203.0.113.11', 51820, 4857, 'k', 'g', 'p', 't', 'transparent', 6)";
        assert!(sqlx::query(second).bind(0).execute(&pool).await.is_err());
        assert!(sqlx::query(second).bind(8).execute(&pool).await.is_err());
        sqlx::query(second).bind(1).execute(&pool).await.unwrap();
        // A VPS that goes takes its count with it, and leaves its servers with none.
        sqlx::query("DELETE FROM gates WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let left: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM traffic")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left.0, 0);
        assert_eq!(
            servers(&pool).await,
            [(text("Survival"), None), (text("Creative"), None)]
        );
    }

    #[tokio::test]
    async fn a_homewarp_without_a_vps_or_still_waiting_for_one_comes_through_with_none() {
        let text = str::to_owned;
        let none = [(text("Survival"), None), (text("Creative"), None)];
        let files = tempfile::tempdir().unwrap();
        let pool = older(&files).await;
        sqlx::raw_sql(SEVERAL).execute(&pool).await.unwrap();
        assert_eq!(servers(&pool).await, none);

        // One that was handed its command and has not run it is no VPS to be reached through.
        let files = tempfile::tempdir().unwrap();
        let pool = older(&files).await;
        sqlx::raw_sql(
            "INSERT INTO gate
                 (id, address, wg_port, api_port, private_key, gate_public_key, preshared_key,
                  token, join_token, join_expires_at, mode, created_at)
             VALUES (1, 'vps.example.com', 51820, 4857, 'k', 'g', 'p', 't', 'join', 99,
                     'transparent', 5);",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(SEVERAL).execute(&pool).await.unwrap();
        assert_eq!(servers(&pool).await, none);
        let waiting: (String, Option<String>) =
            sqlx::query_as("SELECT address, join_token FROM gates")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(waiting, (text("vps.example.com"), Some(text("join"))));
    }

    /// The webhooks there are once the one address that could be told has become the first.
    async fn webhooks(
        pool: &SqlitePool,
    ) -> Vec<(String, String, bool, Option<String>, Option<i64>)> {
        sqlx::raw_sql(SEVERAL).execute(pool).await.unwrap();
        sqlx::raw_sql(WEBHOOKS).execute(pool).await.unwrap();
        let left: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM settings WHERE key LIKE 'webhook%'")
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(left.0, 0);
        sqlx::query_as("SELECT name, url, everything, last_problem, last_at FROM webhooks")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_one_address_that_was_told_comes_through_as_the_first_webhook() {
        let text = str::to_owned;
        // None was set: there is none, and what else was set stays.
        let files = tempfile::tempdir().unwrap();
        let pool = older(&files).await;
        sqlx::raw_sql(r#"INSERT INTO settings (key, value) VALUES ('resolvers', '["9.9.9.9"]')"#)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(webhooks(&pool).await, []);
        let kept: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kept.0, 1);

        // One that was told of what happens by itself is told of that, by name.
        let files = tempfile::tempdir().unwrap();
        let pool = older(&files).await;
        sqlx::raw_sql(
            r#"INSERT INTO settings (key, value) VALUES
                   ('webhook', '{"url":"https://discord.com/api/webhooks/1/x","everything":false}'),
                   ('webhook_last', '{"at":7,"problem":"discord.com did not take it (404)."}')"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            webhooks(&pool).await,
            [(
                text("Notices"),
                text("https://discord.com/api/webhooks/1/x"),
                false,
                Some(text("discord.com did not take it (404).")),
                Some(7),
            )]
        );
        let events: (String,) = sqlx::query_as("SELECT events FROM webhooks")
            .fetch_one(&pool)
            .await
            .unwrap();
        let events: Vec<String> = serde_json::from_str(&events.0).unwrap();
        assert_eq!(events.len(), 12);
        for event in &events {
            let by_itself = crate::notify::EVENTS
                .iter()
                .any(|(name, by_itself)| name == event && *by_itself);
            assert!(by_itself, "{event}");
        }

        // One that was told of everything still is, and has been sent nothing yet.
        let files = tempfile::tempdir().unwrap();
        let pool = older(&files).await;
        sqlx::raw_sql(
            r#"INSERT INTO settings (key, value) VALUES
                   ('webhook', '{"url":"https://hooks.slack.com/services/1/y","everything":true}')"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            webhooks(&pool).await,
            [(
                text("Notices"),
                text("https://hooks.slack.com/services/1/y"),
                true,
                None,
                None,
            )]
        );
    }
}
