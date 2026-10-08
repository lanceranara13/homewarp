//! Webhooks (PLAN.md §11, Phase 7, where they were called notices): what
//! happens to this Homewarp is told to addresses of the owner's choosing as it
//! happens, the way Discord, Slack and their like take a message: JSON, sent
//! to an address that is itself the secret.
//!
//! Whatever is told was written down for the Activity page first, so there is
//! one list of what counts as having happened, and what a webhook is sent is a
//! line of it. Each webhook is told of what its owner chose for it: some of
//! the things that are written down, or every one.
//!
//! An address is held to what any address Core fetches from is held to
//! (`fetch`): `https`, a name, an address on the internet. Nothing waits for a
//! webhook, and a line that cannot be delivered is tried three times and let go.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use serde::Serialize;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::mpsc::{self, error::TrySendError};
use utoipa::ToSchema;

use crate::{auth, fetch};

/// Everything that is written down, and so everything a webhook can be told
/// of, in the order the page lists it in. With each, whether it happens with
/// nobody at the panel: what a webhook is most often wanted for.
pub(crate) const EVENTS: &[(&str, bool)] = &[
    ("server.crash", true),
    ("server.gave_up", true),
    ("server.sleep", true),
    ("server.wake", true),
    ("server.start", false),
    ("server.stop", false),
    ("server.kill", false),
    ("server.command", false),
    ("server.create", false),
    ("server.change", false),
    ("server.icon", false),
    ("server.install", false),
    ("server.remove", false),
    ("server.let_in", false),
    ("server.turn_out", false),
    ("files.write", false),
    ("files.download", false),
    ("files.folder", false),
    ("files.move", false),
    ("files.remove", false),
    ("files.pack", false),
    ("files.unpack", false),
    ("mods.install", false),
    ("backup.copy_failed", true),
    ("backup.create", false),
    ("backup.restore", false),
    ("backup.copy", false),
    ("backup.download", false),
    ("backup.remove", false),
    ("backup.keep", false),
    ("schedule.ran", true),
    ("schedule.run", false),
    ("schedule.create", false),
    ("schedule.change", false),
    ("schedule.remove", false),
    ("gate.lost", true),
    ("gate.back", true),
    ("gate.connect", false),
    ("gate.check", false),
    ("gate.rename", false),
    ("gate.disconnect", false),
    ("gate.harden", false),
    ("gate.harden_keep", false),
    ("gate.unharden", false),
    ("panel.certificate", true),
    ("panel.name", false),
    ("panel.ask", false),
    ("update.available", true),
    ("update.done", true),
    ("update.failed", true),
    ("update.install", false),
    ("update.channel", false),
    ("account.sign_in", false),
    ("account.sign_in_failed", false),
    ("account.sign_out", false),
    ("sftp.sign_in", false),
    ("sftp.sign_in_failed", false),
    ("account.setup", false),
    ("account.create", false),
    ("account.remove", false),
    ("account.password", false),
    ("account.two_steps_on", false),
    ("account.two_steps_off", false),
    ("account.passkey_add", false),
    ("account.passkey_remove", false),
    ("template.import", false),
    ("template.remove", false),
    ("template.fetch", false),
    ("catalogue.fetch", false),
    ("settings.change", false),
    ("webhook.create", false),
    ("webhook.change", false),
    ("webhook.remove", false),
];

/// How many lines may wait their turn, to be sorted and for each webhook. One
/// more is not sent: a site that takes none is not queued for without end.
const WAITING: usize = 64;
/// The way to whatever sorts the lines, one after another, in the order
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

/// An address that is told, and what it is told of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Webhook {
    pub(crate) id: i64,
    /// What its owner calls it.
    pub(crate) name: String,
    pub(crate) url: String,
    /// Every line of the Activity page, whatever is written down there.
    pub(crate) everything: bool,
    /// The names of what it is told of, where that is not everything.
    pub(crate) events: Vec<String>,
    /// How what it was last sent went, once it has been sent something.
    pub(crate) last: Option<Delivery>,
}

impl Webhook {
    fn told_of(&self, action: &str) -> bool {
        self.everything || self.events.iter().any(|event| event == action)
    }
}

/// How the last thing a webhook was sent went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
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
    pub(crate) server_id: Option<i64>,
    pub(crate) action: &'static str,
    pub(crate) detail: String,
    /// When, in Unix seconds.
    pub(crate) at: i64,
}

/// Whether something is written down under this name, and so can be told.
pub(crate) fn known(action: &str) -> bool {
    EVENTS.iter().any(|(name, _)| *name == action)
}

type Row = (
    i64,
    String,
    String,
    bool,
    String,
    Option<i64>,
    Option<String>,
);

fn read((id, name, url, everything, events, last_at, problem): Row) -> Webhook {
    Webhook {
        id,
        name,
        url,
        everything,
        events: serde_json::from_str(&events).unwrap_or_default(),
        last: last_at.map(|at| Delivery { at, problem }),
    }
}

/// Every webhook, by name.
pub(crate) async fn all(db: &SqlitePool) -> Result<Vec<Webhook>, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, url, everything, events, last_at, last_problem
         FROM webhooks ORDER BY name COLLATE NOCASE, id",
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(read).collect())
}

pub(crate) async fn one(db: &SqlitePool, id: i64) -> Result<Option<Webhook>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, name, url, everything, events, last_at, last_problem
         FROM webhooks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(read))
}

/// Has an address told from now on, and says which webhook it has become.
pub(crate) async fn add(
    db: &SqlitePool,
    name: &str,
    url: &str,
    everything: bool,
    events: &[String],
) -> Result<i64, sqlx::Error> {
    let made = sqlx::query(
        "INSERT INTO webhooks (name, url, everything, events, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(url)
    .bind(everything)
    .bind(json!(events).to_string())
    .bind(auth::now())
    .execute(db)
    .await?;
    Ok(made.last_insert_rowid())
}

/// Changes a webhook, and says whether there was one to change. Without a
/// `url` its address stays. With one, how the last thing went to the address
/// before is forgotten: it says nothing of this one.
pub(crate) async fn change(
    db: &SqlitePool,
    id: i64,
    name: &str,
    url: Option<&str>,
    everything: bool,
    events: &[String],
) -> Result<bool, sqlx::Error> {
    let changed = sqlx::query(
        "UPDATE webhooks
         SET name = ?1, everything = ?2, events = ?3, url = COALESCE(?4, url),
             last_at = CASE WHEN ?4 IS NULL THEN last_at END,
             last_problem = CASE WHEN ?4 IS NULL THEN last_problem END
         WHERE id = ?5",
    )
    .bind(name)
    .bind(everything)
    .bind(json!(events).to_string())
    .bind(url)
    .bind(id)
    .execute(db)
    .await?;
    Ok(changed.rows_affected() > 0)
}

/// Has a webhook told no more, and says what it was called, if there was one.
pub(crate) async fn remove(db: &SqlitePool, id: i64) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("DELETE FROM webhooks WHERE id = ? RETURNING name")
        .bind(id)
        .fetch_optional(db)
        .await
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
            "A VPS has stopped answering Homewarp: {detail}. Players may not be reaching the servers behind it."
        ),
        "gate.back" => format!("The VPS {detail} answers again."),
        "update.available" => {
            format!("Homewarp {detail} is out. It can be put in place under Settings, Updates.")
        }
        "update.done" => format!("Homewarp was updated to {detail}."),
        "update.failed" => format!(
            "The update to Homewarp {detail} did not start, and the version before it was put back."
        ),
        "webhook.test" => "Homewarp can tell this address what happens.".to_owned(),
        // Every other line: who, what, to which server, and what was written
        // beside it.
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
///
/// Much of a sentence was typed by somebody: a schedule's name, a command, a
/// file's. It is told as what they typed and not as a message of their
/// making: Discord is to mention nobody for it, and Slack is given its three
/// marks written out, so that none of it is read as a mention or a link.
fn body(happened: &Happened, server: Option<&str>) -> serde_json::Value {
    let said = sentence(happened, server);
    let written_out = said
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    json!({
        "content": said,
        "allowed_mentions": { "parse": [] },
        "text": written_out,
        "event": happened.action,
        "server": server,
        "by": happened.by,
        "detail": happened.detail,
        "at": happened.at,
    })
}

/// Sends once, and writes down how it went: beside the webhook, for as long
/// as it still has the address this went to.
async fn send(db: &SqlitePool, id: i64, url: &str, body: &serde_json::Value) -> Result<(), String> {
    let sent = fetch::send_json(url, body).await;
    let written =
        sqlx::query("UPDATE webhooks SET last_at = ?, last_problem = ? WHERE id = ? AND url = ?")
            .bind(auth::now())
            .bind(sent.as_ref().err().map(String::as_str))
            .bind(id)
            .bind(url)
            .execute(db)
            .await;
    if let Err(error) = written {
        tracing::error!("how a webhook was told could not be written down: {error:#}");
    }
    sent
}

/// Sends a webhook a line that says only that it is reached, now, and says
/// how it went.
pub(crate) async fn test(db: &SqlitePool, webhook: &Webhook, by: &str) -> Result<(), String> {
    let happened = Happened {
        by: by.to_owned(),
        server_id: None,
        action: "webhook.test",
        detail: String::new(),
        at: auth::now(),
    };
    send(db, webhook.id, &webhook.url, &body(&happened, None)).await
}

/// Tells the webhooks of a line that was just written down, each one that is
/// told of such a line. Nothing waits for it: the line joins those waiting to
/// be sorted, which go one after another.
pub(crate) fn tell(db: &SqlitePool, happened: Happened) {
    let mut queue = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut line = (db.clone(), happened);
    if let Some(sender) = queue.as_ref() {
        match sender.try_send(line) {
            Ok(()) | Err(TrySendError::Full(_)) => return,
            // What sorted them went with the runtime it ran on.
            Err(TrySendError::Closed(back)) => line = back,
        }
    }
    let (sender, waiting) = mpsc::channel(WAITING);
    let _ = sender.try_send(line);
    *queue = Some(sender);
    tokio::spawn(sort_each(waiting));
}

/// What one webhook is still to be sent: which it is and where it is, so that
/// one changed since is known, and what.
type Parcel = (SqlitePool, i64, String, serde_json::Value);

/// Hands each line to the webhooks that are told of it, in the order things
/// happened. Each webhook has what waits for it sent by itself, so that one
/// whose site is down holds up no other.
async fn sort_each(mut waiting: mpsc::Receiver<(SqlitePool, Happened)>) {
    let mut ways: HashMap<i64, mpsc::Sender<Parcel>> = HashMap::new();
    while let Some((db, happened)) = waiting.recv().await {
        let Ok(webhooks) = all(&db).await else {
            continue;
        };
        // A webhook that has gone needs no way to it.
        ways.retain(|id, _| webhooks.iter().any(|webhook| webhook.id == *id));
        let told: Vec<&Webhook> = webhooks
            .iter()
            .filter(|webhook| webhook.told_of(happened.action))
            .collect();
        if told.is_empty() {
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
        for webhook in told {
            let way = ways.entry(webhook.id).or_insert_with(|| {
                let (way, parcels) = mpsc::channel(WAITING);
                tokio::spawn(send_each(parcels));
                way
            });
            // One more than may wait is not sent.
            let _ = way.try_send((db.clone(), webhook.id, webhook.url.clone(), body.clone()));
        }
    }
}

/// Sends what waits for one webhook, one line after another, so that what it
/// is told arrives in the order it happened.
async fn send_each(mut parcels: mpsc::Receiver<Parcel>) {
    while let Some((db, id, url, body)) = parcels.recv().await {
        for wait in TRIES {
            tokio::time::sleep(wait).await;
            // A webhook that has gone, or been given another address, since
            // this line was put down for it is not told it.
            let still = one(&db, id).await.ok().flatten();
            if !still.is_some_and(|kept| kept.url == url) {
                break;
            }
            if send(&db, id, &url, &body).await.is_ok() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Delivery, EVENTS, Happened, add, all, body, change, known, one, remove, sentence, shortened,
    };

    fn happened(action: &'static str, detail: &str) -> Happened {
        Happened {
            by: "Homewarp".to_owned(),
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
            sentence(&happened("gate.back", "Frankfurt"), None),
            "The VPS Frankfurt answers again."
        );
        assert_eq!(
            sentence(
                &happened("gate.lost", "Frankfurt (it did not answer)"),
                None
            ),
            "A VPS has stopped answering Homewarp: Frankfurt (it did not answer). Players may not be reaching the servers behind it."
        );
        // Any other line, for a webhook that is told of it.
        let typed = Happened {
            by: "alice".to_owned(),
            ..happened("server.command", "say hello")
        };
        assert_eq!(
            sentence(&typed, Some("Lobby")),
            "alice: server.command, Lobby (say hello)"
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
    fn a_line_is_read_by_discord_and_by_slack() {
        let sent = body(&happened("server.crash", "exit code 1"), Some("Lobby"));
        assert_eq!(sent["content"], "Lobby crashed: exit code 1.");
        assert_eq!(sent["text"], sent["content"]);
        assert_eq!(sent["event"], "server.crash");
        assert_eq!(sent["server"], "Lobby");
        assert_eq!(sent["at"], 1_791_333_715);
        // What somebody typed mentions nobody, and is no link of Slack's making.
        let typed = body(
            &happened(
                "schedule.ran",
                "@everyone <!channel> & <https://example.com|here>: Done.",
            ),
            Some("Lobby"),
        );
        assert_eq!(typed["allowed_mentions"]["parse"], serde_json::json!([]));
        assert_eq!(
            typed["text"],
            "Lobby, schedule @everyone &lt;!channel&gt; &amp; &lt;https://example.com|here&gt;: Done."
        );
    }

    #[test]
    fn an_address_is_shown_without_what_makes_it_a_secret() {
        let shown = shortened("https://discord.com/api/webhooks/123456789/AbCdEf-secret-Token9xYz");
        assert_eq!(shown, "discord.com/…9xYz");
        assert!(!shown.contains("secret"));
    }

    #[test]
    fn what_can_be_told_is_listed_once() {
        for (at, (name, _)) in EVENTS.iter().enumerate() {
            assert!(
                !EVENTS[..at].iter().any(|(before, _)| before == name),
                "{name} is listed twice"
            );
        }
        assert!(known("server.crash"));
        assert!(!known("server.dance"));
        // A line that only says a webhook is reached is not one to be told of.
        assert!(!known("webhook.test"));
    }

    #[tokio::test]
    async fn webhooks_are_kept_changed_and_forgotten() {
        let files = tempfile::tempdir().unwrap();
        let db = crate::open(&files.path().join("homewarp.db"))
            .await
            .unwrap();
        assert_eq!(all(&db).await.unwrap(), []);

        let crashes = ["server.crash".to_owned(), "server.gave_up".to_owned()];
        let first = "https://discord.com/api/webhooks/1/x";
        let id = add(&db, "Crashes", first, false, &crashes).await.unwrap();
        let other = add(
            &db,
            "All of it",
            "https://hooks.slack.com/services/1/y",
            true,
            &[],
        )
        .await
        .unwrap();
        let kept = all(&db).await.unwrap();
        // By name.
        assert_eq!(
            kept.iter().map(|webhook| webhook.id).collect::<Vec<_>>(),
            [other, id]
        );
        let webhook = one(&db, id).await.unwrap().unwrap();
        assert_eq!((webhook.url.as_str(), webhook.everything), (first, false));
        assert_eq!(webhook.events, crashes);
        assert_eq!(webhook.last, None);
        // Each is told of what was chosen for it, and of nothing else.
        assert!(webhook.told_of("server.crash"));
        assert!(!webhook.told_of("server.start"));
        assert!(kept[0].told_of("server.start"));

        // How the last thing went stays for as long as the address does.
        sqlx::query("UPDATE webhooks SET last_at = 5, last_problem = 'no' WHERE id = ?")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
        let wakes = ["server.wake".to_owned()];
        assert!(change(&db, id, "Wakes", None, false, &wakes).await.unwrap());
        let webhook = one(&db, id).await.unwrap().unwrap();
        assert_eq!(
            (webhook.name.as_str(), webhook.url.as_str()),
            ("Wakes", first)
        );
        assert_eq!(webhook.events, wakes);
        let went = Delivery {
            at: 5,
            problem: Some("no".to_owned()),
        };
        assert_eq!(webhook.last, Some(went));
        let second = "https://discord.com/api/webhooks/2/z";
        assert!(
            change(&db, id, "Wakes", Some(second), true, &wakes)
                .await
                .unwrap()
        );
        let webhook = one(&db, id).await.unwrap().unwrap();
        assert_eq!((webhook.url.as_str(), webhook.everything), (second, true));
        assert_eq!(webhook.last, None);

        assert_eq!(remove(&db, id).await.unwrap().as_deref(), Some("Wakes"));
        assert_eq!(remove(&db, id).await.unwrap(), None);
        assert!(!change(&db, id, "Wakes", None, true, &[]).await.unwrap());
        assert_eq!(all(&db).await.unwrap().len(), 1);
    }
}
