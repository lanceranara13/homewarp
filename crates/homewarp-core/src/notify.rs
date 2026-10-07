//! Notices (PLAN.md §11, Phase 7): what happens to this Homewarp is told to an
//! address of the owner's choosing as it happens, the way Discord, Slack and
//! their like take a message: JSON, sent to an address that is itself the
//! secret.
//!
//! Whatever is told was written down for the Activity page first, so there is
//! one list of what counts as having happened, and a notice is a line of it
//! sent on. By itself that is what nobody was at the panel for: a crash, a
//! server put to sleep or woken, what a schedule did, a VPS that stopped
//! answering. The owner may ask for every line instead.
//!
//! The address is held to what any address Core fetches from is held to
//! (`fetch`): `https`, a name, an address on the internet. Nothing waits for a
//! notice, and one that cannot be delivered is tried three times and let go.

use std::{
    sync::{Mutex, PoisonError},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::mpsc::{self, error::TrySendError};
use utoipa::ToSchema;

use crate::{auth, fetch};

/// The keys in `settings`: where notices go, and how the last one went.
const WEBHOOK: &str = "webhook";
const LAST: &str = "webhook_last";
/// How many notices may wait their turn. One more is not sent: a site that
/// takes none is not queued for without end.
const WAITING: usize = 64;
/// The way to whatever sends the notices, one after another, in the order
/// things happened. Each carries the database it is about.
static QUEUE: Mutex<Option<mpsc::Sender<(SqlitePool, Happened)>>> = Mutex::new(None);
/// How long is waited before each try. A site that is down for a moment is
/// there again by the third.
const TRIES: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(5),
    Duration::from_secs(30),
];
/// Discord takes 2000 characters, and nothing told here needs as many.
const LONGEST: usize = 1800;

/// Where notices go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Webhook {
    pub(crate) url: String,
    /// Every line of the Activity page, and not only what happened by itself.
    pub(crate) everything: bool,
}

/// How the last notice went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub(crate) struct Delivery {
    /// When, in Unix seconds.
    at: i64,
    /// Why the site did not take it. Nothing if it did.
    problem: Option<String>,
}

/// One line of the Activity page, as it was written down.
pub(crate) struct Happened {
    /// The account that did it, or Homewarp.
    pub(crate) by: String,
    /// Whether nobody asked for it just now.
    pub(crate) by_itself: bool,
    pub(crate) server_id: Option<i64>,
    pub(crate) action: &'static str,
    pub(crate) detail: String,
    /// When, in Unix seconds.
    pub(crate) at: i64,
}

async fn kept<T: serde::de::DeserializeOwned>(db: &SqlitePool, key: &str) -> Option<T> {
    let json: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await
        .ok()?;
    serde_json::from_str(&json?).ok()
}

async fn keep<T: Serialize>(db: &SqlitePool, key: &str, value: &T) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(serde_json::to_string(value)?)
    .execute(db)
    .await?;
    Ok(())
}

/// Where notices go, if they go anywhere.
pub(crate) async fn webhook(db: &SqlitePool) -> Option<Webhook> {
    kept(db, WEBHOOK).await
}

/// How the last notice went, if one was ever sent.
pub(crate) async fn last(db: &SqlitePool) -> Option<Delivery> {
    kept(db, LAST).await
}

/// Has notices go to `webhook` from now on. How the last one went to the
/// address before is forgotten: it says nothing of this one.
pub(crate) async fn set(db: &SqlitePool, webhook: &Webhook) -> anyhow::Result<()> {
    keep(db, WEBHOOK, webhook).await?;
    forget(db, LAST).await
}

/// Has notices go nowhere.
pub(crate) async fn unset(db: &SqlitePool) -> anyhow::Result<()> {
    forget(db, WEBHOOK).await?;
    forget(db, LAST).await
}

async fn forget(db: &SqlitePool, key: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(db)
        .await?;
    Ok(())
}

/// An address with what makes it a secret left out: the site, and the last
/// few characters of the rest, enough to tell one from another.
pub(crate) fn shortened(url: &str) -> String {
    let site = fetch::site(url).unwrap_or_default();
    let end: String = url
        .trim()
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{site}/…{end}")
}

/// A line of the Activity page as a sentence for somebody who is not looking
/// at the page.
fn sentence(happened: &Happened, server: Option<&str>) -> String {
    let Happened { by, detail, .. } = happened;
    let server = server.unwrap_or("A server that is no more");
    let said = match happened.action {
        "server.crash" => format!("{server} crashed: {detail}."),
        "server.gave_up" => format!("{server} is not started again by Homewarp: {detail}."),
        "server.sleep" => format!("{server} was put to sleep: {detail}."),
        "server.wake" => format!("{server} was woken by {detail}."),
        // What a schedule came to ends its own sentence.
        "schedule.ran" => format!("{server}, schedule {detail}"),
        "panel.certificate" => format!("The panel has a new certificate for {detail}."),
        "backup.copy_failed" => {
            format!("{server}: a backup was not copied to the store elsewhere. {detail}")
        }
        // The tunnel may still carry players while the Gate's own program is down.
        "gate.lost" => format!(
            "The VPS has stopped answering Homewarp: {detail}. Players may not be reaching the servers."
        ),
        "gate.back" => "The VPS answers again.".to_owned(),
        "notice.test" => "Homewarp can tell this address what happens.".to_owned(),
        // Every other line, for an owner who asked for every line: who, what,
        // to which server, and what was written beside it.
        action => {
            let mut said = format!("{by}: {action}");
            if happened.server_id.is_some() {
                said.push_str(&format!(", {server}"));
            }
            if !detail.is_empty() {
                said.push_str(&format!(" ({detail})"));
            }
            said
        }
    };
    said.chars().take(LONGEST).collect()
}

/// What is sent: the sentence under the two names it is read by (`content` is
/// Discord's, `text` is Slack's and Mattermost's), and the line's parts for a
/// program that would rather have those.
fn body(happened: &Happened, server: Option<&str>) -> serde_json::Value {
    let said = sentence(happened, server);
    json!({
        "content": said,
        "text": said,
        "event": happened.action,
        "server": server,
        "by": happened.by,
        "detail": happened.detail,
        "at": happened.at,
    })
}

/// Sends once, and writes down how it went.
async fn send(db: &SqlitePool, url: &str, body: &serde_json::Value) -> Result<(), String> {
    let sent = fetch::send_json(url, body).await;
    let delivery = Delivery {
        at: auth::now(),
        problem: sent.clone().err(),
    };
    if let Err(error) = keep(db, LAST, &delivery).await {
        tracing::error!("how a notice went could not be written down: {error:#}");
    }
    sent
}

/// Sends a notice that says only that notices arrive, now, and says how it went.
pub(crate) async fn test(db: &SqlitePool, by: &str) -> Result<(), String> {
    let Some(webhook) = webhook(db).await else {
        return Err("There is no address to tell.".to_owned());
    };
    let happened = Happened {
        by: by.to_owned(),
        by_itself: false,
        server_id: None,
        action: "notice.test",
        detail: String::new(),
        at: auth::now(),
    };
    send(db, &webhook.url, &body(&happened, None)).await
}

/// Tells the owner's address of a line that was just written down, where
/// there is an address and the line is one it is told of. Nothing waits for
/// it: the line joins those waiting to be sent, which go one after another.
pub(crate) fn tell(db: &SqlitePool, happened: Happened) {
    let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut line = (db.clone(), happened);
    if let Some(sender) = queue.as_ref() {
        match sender.try_send(line) {
            Ok(()) | Err(TrySendError::Full(_)) => return,
            // What sent them went with the runtime it ran on.
            Err(TrySendError::Closed(back)) => line = back,
        }
    }
    let (sender, waiting) = mpsc::channel(WAITING);
    let _ = sender.try_send(line);
    *queue = Some(sender);
    tokio::spawn(send_each(waiting));
}

/// Sends what waits, one line after another, so that what is told arrives in
/// the order it happened.
async fn send_each(mut waiting: mpsc::Receiver<(SqlitePool, Happened)>) {
    while let Some((db, happened)) = waiting.recv().await {
        let Some(webhook) = webhook(&db).await else {
            continue;
        };
        if !(webhook.everything || happened.by_itself) {
            continue;
        }
        let server: Option<String> = match happened.server_id {
            Some(id) => sqlx::query_scalar("SELECT name FROM servers WHERE id = ?")
                .bind(id)
                .fetch_optional(&db)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let body = body(&happened, server.as_deref());
        for wait in TRIES {
            tokio::time::sleep(wait).await;
            if send(&db, &webhook.url, &body).await.is_ok() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Delivery, Happened, Webhook, body, last, sentence, set, shortened, unset, webhook,
    };

    fn happened(action: &'static str, detail: &str) -> Happened {
        Happened {
            by: "Homewarp".to_owned(),
            by_itself: true,
            server_id: Some(1),
            action,
            detail: detail.to_owned(),
            at: 1_791_333_715,
        }
    }

    #[test]
    fn what_happened_by_itself_is_said_in_a_sentence() {
        for (action, detail, said) in [
            (
                "server.crash",
                "exit code 137",
                "Lobby crashed: exit code 137.",
            ),
            (
                "server.gave_up",
                "4 crashes one after another",
                "Lobby is not started again by Homewarp: 4 crashes one after another.",
            ),
            (
                "server.sleep",
                "nobody on for 10 minutes",
                "Lobby was put to sleep: nobody on for 10 minutes.",
            ),
            (
                "server.wake",
                "Steve from 203.0.113.50",
                "Lobby was woken by Steve from 203.0.113.50.",
            ),
            (
                "schedule.ran",
                "Nightly: Done.",
                "Lobby, schedule Nightly: Done.",
            ),
        ] {
            assert_eq!(sentence(&happened(action, detail), Some("Lobby")), said);
        }
        assert_eq!(
            sentence(&happened("gate.back", ""), None),
            "The VPS answers again."
        );
        assert_eq!(
            sentence(&happened("gate.lost", "it did not answer"), None),
            "The VPS has stopped answering Homewarp: it did not answer. Players may not be reaching the servers."
        );
        // Any other line, for an owner who asked for every line.
        let typed = Happened {
            by: "lance".to_owned(),
            by_itself: false,
            ..happened("server.command", "say hello")
        };
        assert_eq!(
            sentence(&typed, Some("Lobby")),
            "lance: server.command, Lobby (say hello)"
        );
        let signed_in = Happened {
            server_id: None,
            ..happened("account.sign_in", "")
        };
        assert_eq!(sentence(&signed_in, None), "Homewarp: account.sign_in");
        // And never longer than a site takes.
        let long = sentence(&happened("server.crash", &"x".repeat(5000)), Some("Lobby"));
        assert_eq!(long.chars().count(), 1800);
    }

    #[test]
    fn a_notice_is_read_by_discord_and_by_slack() {
        let sent = body(&happened("server.crash", "exit code 1"), Some("Lobby"));
        assert_eq!(sent["content"], "Lobby crashed: exit code 1.");
        assert_eq!(sent["text"], sent["content"]);
        assert_eq!(sent["event"], "server.crash");
        assert_eq!(sent["server"], "Lobby");
        assert_eq!(sent["at"], 1_791_333_715);
    }

    #[test]
    fn an_address_is_shown_without_what_makes_it_a_secret() {
        let shown = shortened("https://discord.com/api/webhooks/123456789/AbCdEf-secret-Token9xYz");
        assert_eq!(shown, "discord.com/…9xYz");
        assert!(!shown.contains("secret"));
    }

    #[tokio::test]
    async fn where_notices_go_is_kept_and_forgotten() {
        let files = tempfile::tempdir().unwrap();
        let db = crate::open(&files.path().join("homewarp.db"))
            .await
            .unwrap();
        assert_eq!(webhook(&db).await, None);
        let to = Webhook {
            url: "https://discord.com/api/webhooks/1/x".to_owned(),
            everything: true,
        };
        set(&db, &to).await.unwrap();
        assert_eq!(webhook(&db).await, Some(to));
        assert_eq!(last(&db).await, None::<Delivery>);
        unset(&db).await.unwrap();
        assert_eq!(webhook(&db).await, None);
    }
}
