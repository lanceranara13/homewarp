//! Newer releases of Homewarp (PLAN.md §11, Phase 8): looking for one, and
//! putting it in place of the one that runs.
//!
//! Where releases are served there is a short list of them, one to a channel:
//! `stable` for what is released, `beta` for what is being tried before it is.
//! The list is signed with the key every release is signed with, and so is the
//! list of checksums that each release has. Nothing is believed for where it
//! came from: the list and the program are held against that key, whose public
//! half this program was built with.
//!
//! A Homewarp that the installer set up can replace itself. It fetches the
//! program, checks it, copies its own database beside it, and hands over to a
//! small helper that is started apart from it: the helper puts the program
//! where the installer put the old one and has Docker start everything again.
//! If the new one does not come up and stay up, the helper puts the old one
//! back, and the database as it was.

use std::{
    path::Path,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use axum::{Json, extract::State, http::StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use homewarp_runtime::Apart;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody},
    audit, auth, backups, fetch,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The public half of the key releases are signed with, as a release is built
/// with it: its 32 bytes in base64. None in a program built from its sources
/// for trying, which then says that it cannot tell a release from anything else.
const BUILT_WITH: Option<&str> = option_env!("HOMEWARP_RELEASE_KEY");
/// How long after it starts Homewarp first looks for a newer release, and how
/// often after that.
const FIRST_AFTER: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// The most a list, a signature and a program may be, and how long each may take.
const LIST: usize = 64 * 1024;
const PROGRAM: usize = 256 * 1024 * 1024;
const PATIENCE: Duration = Duration::from_secs(20);
const PATIENCE_FOR_A_PROGRAM: Duration = Duration::from_secs(15 * 60);
/// How long the servers' backups may take before an update is given up.
const PATIENCE_FOR_BACKUPS: Duration = Duration::from_secs(2 * 60 * 60);
/// What the helper runs from: Docker's own image of its command line, which
/// has Compose in it. The installer started Homewarp with the same.
const HELPER: &str = "docker:cli";

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_update, change_update))
        .routes(routes!(check_update))
        .routes(routes!(install_update))
}

/// Which releases a Homewarp takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum Channel {
    /// What has been released.
    #[default]
    Stable,
    /// What is being tried before it is released, and what has been released
    /// where that is newer.
    Beta,
}

/// A version as releases are numbered: three numbers, and for one that comes
/// before a release a word after a hyphen (`1.2.0-beta.1`). Ordered as
/// versions are: by the numbers, and a release after everything that came
/// before it under its number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    numbers: [u64; 3],
    before: Before,
}

/// What follows the hyphen. The order of the two says that anything with one
/// comes before the release that has none.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Before {
    Release(Vec<Part>),
    Nothing,
}

/// One of the parts between the dots there: a number, which comes before any word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Number(u64),
    Word(String),
}

impl Version {
    fn read(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        // What follows a plus sign says how it was built, and orders nothing.
        let text = text.split('+').next()?;
        let (numbers, before) = match text.split_once('-') {
            Some((numbers, before)) => (numbers, Some(before)),
            None => (text, None),
        };
        let mut each = numbers.split('.').map(|number| number.parse::<u64>().ok());
        let numbers = [each.next()??, each.next()??, each.next()??];
        if each.next().is_some() {
            return None;
        }
        let before = match before {
            None => Before::Nothing,
            Some("") => return None,
            // Letters, digits and hyphens between the dots, as versions are
            // written: a version goes into addresses, and into a script.
            Some(before)
                if !before.split('.').all(|part| {
                    !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                }) =>
            {
                return None;
            }
            Some(before) => Before::Release(
                before
                    .split('.')
                    .map(|part| match part.parse() {
                        Ok(number) => Part::Number(number),
                        Err(_) => Part::Word(part.to_owned()),
                    })
                    .collect(),
            ),
        };
        Some(Self { numbers, before })
    }
}

/// Whether `candidate` is a newer version than `than`. What is not a version
/// is newer than nothing.
fn newer(candidate: &str, than: &str) -> bool {
    match (Version::read(candidate), Version::read(than)) {
        (Some(candidate), Some(than)) => candidate > than,
        _ => false,
    }
}

/// The newest release of each channel, as the list where releases are served says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Listed {
    stable: Option<String>,
    beta: Option<String>,
}

impl Listed {
    /// Reads the list: a line to a channel, its name and then its version.
    /// What is no such line is passed over, so that a list may one day say more.
    fn read(text: &str) -> Self {
        let mut listed = Self::default();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let (Some(channel), Some(version), None) = (words.next(), words.next(), words.next())
            else {
                continue;
            };
            if Version::read(version).is_none() {
                continue;
            }
            match channel {
                "stable" => listed.stable = Some(version.to_owned()),
                "beta" => listed.beta = Some(version.to_owned()),
                _ => {}
            }
        }
        listed
    }

    /// The newest release for whoever takes `channel`. Beta takes what has
    /// been released too, where that is the newer of the two.
    fn newest(&self, channel: Channel) -> Option<&str> {
        let stable = self.stable.as_deref();
        match (channel, self.beta.as_deref()) {
            (Channel::Stable, _) | (Channel::Beta, None) => stable,
            (Channel::Beta, Some(beta)) => match stable {
                Some(stable) if !newer(beta, stable) => Some(stable),
                _ => Some(beta),
            },
        }
    }
}

/// Where an update that is being made stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum Step {
    /// The release is being fetched and held against its signature.
    Fetching,
    /// Homewarp's database is being copied, and the servers backed up where
    /// that was asked for.
    BackingUp,
    /// The new version is being put in place. Homewarp stops answering for a
    /// moment, and answers again as the new version or as the old one.
    StartingAgain,
}

/// An update that is being made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
struct Updating {
    /// The version it is to.
    to: String,
    step: Step,
}

/// How the last update went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
struct Last {
    /// The version it was to.
    to: String,
    /// Whether that version is the one that runs now.
    ok: bool,
    /// When that was found out, in Unix seconds.
    at: i64,
    /// What went wrong, where something did: the end of what was written
    /// down while it was tried.
    detail: Option<String>,
}

/// What Homewarp knows of newer releases, between one look and the next.
#[derive(Default)]
pub(crate) struct Updates {
    said: Mutex<Said>,
}

#[derive(Default, Clone)]
struct Said {
    /// When the list was last asked for, in Unix seconds.
    checked_at: Option<i64>,
    listed: Option<Listed>,
    /// Why the list could not be read, or why an update could not be made.
    problem: Option<String>,
    updating: Option<Updating>,
}

fn lock(updates: &Updates) -> MutexGuard<'_, Said> {
    updates.said.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The public half of the key releases are signed with: its 32 bytes. What
/// the deployment says, for a release that is tried with a key of its own, or
/// else what this program was built with.
fn key() -> Option<Vec<u8>> {
    let said = std::env::var("HOMEWARP_RELEASE_KEY").ok();
    let text = said
        .filter(|key| !key.trim().is_empty())
        .or_else(|| BUILT_WITH.map(str::to_owned))?;
    let key = STANDARD.decode(text.trim()).ok()?;
    (key.len() == 32).then_some(key)
}

/// Whether `signature` is that of `what`, made with the key whose public half is `key`.
fn signed(key: &[u8], what: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, key)
        .verify(what, signature)
        .is_ok()
}

/// A setting of this module's, as it is kept: JSON, under a name of its own.
async fn kept<T: serde::de::DeserializeOwned>(db: &SqlitePool, key: &str) -> Option<T> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await
        .ok()?;
    serde_json::from_str(&row?.0).ok()
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

/// The list of releases, fetched from where releases are and believed for its signature.
async fn look(state: &AppState) -> Result<(Listed, Vec<u8>), String> {
    let base = state.releases.as_deref().ok_or(
        "This Homewarp was not told where its releases are, so it cannot check for updates. One that the installer set up is told.",
    )?;
    let key = key().ok_or(
        "This Homewarp was built without the key its releases are signed with, so it could not tell a release from anything else. One that was installed from a release has it.",
    )?;
    let fetched = |file: &'static str| async move {
        fetch::released(&format!("{base}/{file}"), LIST, PATIENCE)
            .await
            .map_err(|why| format!("The list of releases could not be fetched. {why}"))
    };
    let list = fetched("RELEASES").await?;
    let signature = fetched("RELEASES.sig").await?;
    if !signed(&key, &list, &signature) {
        return Err(
            "The list of releases is not signed with Homewarp's key. Nothing in it was believed."
                .to_owned(),
        );
    }
    Ok((Listed::read(&String::from_utf8_lossy(&list)), key))
}

/// Looks for a newer release now, and writes down that there is one the first
/// time it is seen: which is how an owner who is not at the panel is told.
pub(crate) async fn check(state: &AppState) {
    let looked = look(state).await;
    let channel: Channel = kept(&state.db, "update.channel").await.unwrap_or_default();
    let newest = {
        let mut said = lock(&state.updates);
        said.checked_at = Some(auth::now());
        match looked {
            Ok((listed, _)) => {
                said.problem = None;
                said.listed = Some(listed);
            }
            Err(why) => said.problem = Some(why),
        }
        let listed = said.listed.as_ref();
        listed
            .and_then(|listed| listed.newest(channel))
            .filter(|newest| newer(newest, VERSION))
            .map(str::to_owned)
    };
    if let Some(newest) = newest
        && kept::<String>(&state.db, "update.told").await.as_deref() != Some(newest.as_str())
        && keep(&state.db, "update.told", &newest).await.is_ok()
    {
        audit::record_by_homewarp(&state.db, None, "update.available", &newest).await;
    }
}

/// Says how the last update went, once, when Homewarp starts after one; then
/// looks for a newer release every few hours for as long as it runs.
pub(crate) async fn keep_looking(state: AppState) {
    settle(&state).await;
    tokio::time::sleep(FIRST_AFTER).await;
    loop {
        check(&state).await;
        tokio::time::sleep(EVERY).await;
    }
}

/// What an update that was begun came to. The Homewarp that began it left a
/// note of which version it was to; if the helper had to put the old one
/// back, it left what it wrote down while it tried.
async fn settle(state: &AppState) {
    let folder = state.data.join("update");
    let pending = tokio::fs::read_to_string(folder.join("pending")).await.ok();
    let failed = tokio::fs::read_to_string(folder.join("failed")).await.ok();
    let last = match (pending, failed) {
        (None, None) => return,
        // The first line of what the helper left is the version it tried.
        (_, Some(failed)) => {
            let (to, log) = failed.split_once('\n').unwrap_or((failed.as_str(), ""));
            Last {
                to: to.trim().to_owned(),
                ok: false,
                at: auth::now(),
                detail: Some(end_of(log)),
            }
        }
        (Some(to), None) => Last {
            ok: to.trim() == VERSION,
            to: to.trim().to_owned(),
            at: auth::now(),
            detail: None,
        },
    };
    // A note of a version that is not running yet, with nothing said against
    // it, is an update that is still being made: the helper has not got to
    // starting the new one. It is settled by whichever Homewarp starts next.
    if !last.ok && last.detail.is_none() {
        return;
    }
    for note in ["pending", "failed"] {
        let _ = tokio::fs::remove_file(folder.join(note)).await;
    }
    if let Err(error) = keep(&state.db, "update.last", &last).await {
        tracing::warn!("how the update went could not be written down: {error:#}");
    }
    match last.ok {
        true => {
            tracing::info!("Homewarp was updated to {}.", last.to);
            audit::record_by_homewarp(&state.db, None, "update.done", &last.to).await;
        }
        false => {
            tracing::warn!(
                "The update to {} did not start, and {VERSION} was put back.",
                last.to
            );
            audit::record_by_homewarp(&state.db, None, "update.failed", &last.to).await;
        }
    }
}

/// The last lines of what was written down, short enough to show.
fn end_of(log: &str) -> String {
    let lines: Vec<&str> = log.lines().filter(|line| !line.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(12);
    let end = lines[from..].join("\n");
    let start = end.len().saturating_sub(2000);
    // Not from the middle of a character.
    let start = (start..=end.len())
        .find(|at| end.is_char_boundary(*at))
        .unwrap_or(0);
    end[start..].to_owned()
}

/// Where the installer put this Homewarp, for one that it set up.
struct Installed {
    /// The folder its `compose.yml` and its image are in.
    folder: String,
    /// What its containers are called: the name of its project.
    name: String,
}

/// Finds out whether this Homewarp is one the installer set up, by how its own
/// container was made. The answer otherwise is why it cannot replace itself.
async fn installed(state: &AppState) -> Result<Installed, String> {
    let not_installed = "This Homewarp was not set up by the installer, so it cannot put a newer version in place of itself.";
    let runtime = state
        .runtime
        .as_ref()
        .ok_or("Homewarp cannot reach Docker, which is what would start the new version.")?;
    let (image, labels) = runtime
        .own_making()
        .await
        .map_err(|error| format!("{error:#}."))?
        .ok_or(not_installed)?;
    let label = |name: &str| labels.get(name).map(String::as_str).unwrap_or_default();
    let folder = label("com.docker.compose.project.working_dir");
    let name = label("com.docker.compose.project");
    // What the installer makes: a project whose service `core` runs the image
    // it built for this version, with everything kept in `data` beside it.
    let as_installed = label("com.docker.compose.service") == "core"
        && image == format!("homewarp:{VERSION}")
        && !folder.is_empty()
        && !name.is_empty()
        && Path::new(folder).join("data") == *state.data;
    // What goes into the helper's script goes between quotes.
    let plain = |text: &str| {
        text.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
    };
    if !as_installed || !plain(folder) || !plain(name) {
        return Err(not_installed.to_owned());
    }
    Ok(Installed {
        folder: folder.to_owned(),
        name: name.to_owned(),
    })
}

/// What the helper runs, with the five things it is told filled in. It is
/// written for the shell of Docker's own image, which is a small one.
///
/// The new program goes where the installer put the old one, and Compose is
/// told the new version and asked to start everything again, which builds the
/// image anew. Then the helper waits, and looks whether the new Core is up and
/// has not had to be started twice. If it is not, the old program and the old
/// file are put back, and the database as it was before a newer version read it.
const SCRIPT: &str = r#"set -u
DIR='@DIR@' DATA='@DATA@' NAME='@NAME@' OLD='@OLD@' NEW='@NEW@'
cd "$DIR" || exit 1
log="$DATA/update/log"
: > "$log"
up() { docker compose up -d --build >>"$log" 2>&1; }
well() {
  sleep 25
  [ "$(docker inspect -f '{{.State.Running}} {{.RestartCount}}' "$NAME" 2>/dev/null)" = "true 0" ]
}
cp image/homewarp image/homewarp.before && cp compose.yml compose.yml.before || exit 1
if cp "$DATA/update/homewarp" image/homewarp && chmod 755 image/homewarp &&
  sed -i "s/homewarp:$OLD\$/homewarp:$NEW/; s/for version $OLD\\./for version $NEW./" compose.yml; then
  if up; then
    if well; then
      rm -f image/homewarp.before compose.yml.before "$DATA/update/homewarp"
      exit 0
    fi
    echo "Homewarp $NEW did not come up and stay up, so $OLD was put back, with its database as it was." >>"$log"
    docker compose stop core >>"$log" 2>&1
    if [ -f "$DATA/update/before.db" ]; then
      cp "$DATA/update/before.db" "$DATA/homewarp.db" && rm -f "$DATA/homewarp.db-wal" "$DATA/homewarp.db-shm"
    fi
  else
    echo "Docker could not start Homewarp $NEW, so $OLD was left as it is." >>"$log"
  fi
fi
cp image/homewarp.before image/homewarp
cp compose.yml.before compose.yml
rm -f image/homewarp.before compose.yml.before "$DATA/update/homewarp"
{ echo "$NEW"; cat "$log"; } > "$DATA/update/failed"
up
"#;

/// Which of a release's programs is this machine's.
fn program() -> anyhow::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("homewarp-x86_64"),
        "aarch64" => Ok("homewarp-arm64"),
        other => bail!("there is no Homewarp released for this kind of processor ({other})"),
    }
}

/// The checksum a release's signed list gives for one of its files.
fn checksum_of<'a>(sums: &'a str, file: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        let (sum, name) = (words.next()?, words.next()?);
        // sha256sum marks a file read as it is with a star before its name.
        (name.trim_start_matches('*') == file).then_some(sum)
    })
}

/// Whether a release's signed list of checksums is the list of this version
/// and of no other. Every release's list is signed and names the same files,
/// so the version is among what is listed: the checksum of a file `VERSION`
/// that holds its number and a line break (scripts/release.sh).
fn is_of(sums: &str, version: &str) -> bool {
    let named = hex(&Sha256::digest(format!("{version}\n")));
    checksum_of(sums, "VERSION").is_some_and(|sum| sum.eq_ignore_ascii_case(&named))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Makes the update, as far as handing over to the helper. What follows is
/// the helper's, and is found out by the Homewarp that starts next.
async fn update(state: &AppState, to: &str, servers: bool) -> anyhow::Result<()> {
    let step = |step| {
        lock(&state.updates).updating = Some(Updating {
            to: to.to_owned(),
            step,
        });
    };
    let said = |why: String| anyhow::anyhow!(why);
    let installed = installed(state).await.map_err(said)?;
    let base = state
        .releases
        .as_deref()
        .context("this Homewarp was not told where its releases are")?;
    let key = key().context("this Homewarp has no key to hold a release against")?;

    step(Step::Fetching);
    let fetched = |file: String, at_most, patience| async move {
        fetch::released(&format!("{base}/{to}/{file}"), at_most, patience)
            .await
            .map_err(|why| anyhow::anyhow!("{file} of {to} could not be fetched. {why}"))
    };
    let sums = fetched("SHA256SUMS".to_owned(), LIST, PATIENCE).await?;
    let signature = fetched("SHA256SUMS.sig".to_owned(), LIST, PATIENCE).await?;
    ensure!(
        signed(&key, &sums, &signature),
        "the list of checksums of {to} is not signed with Homewarp's key. Nothing was changed"
    );
    let file = program()?;
    let sums = String::from_utf8_lossy(&sums).into_owned();
    ensure!(
        is_of(&sums, to),
        "the list of checksums served for {to} is not that version's own. Nothing was changed"
    );
    let wanted = checksum_of(&sums, file)
        .with_context(|| format!("the release {to} has no {file}"))?
        .to_lowercase();
    let fetched = fetched(file.to_owned(), PROGRAM, PATIENCE_FOR_A_PROGRAM).await?;
    ensure!(
        hex(&Sha256::digest(&fetched)) == wanted,
        "{file} is not the file that was released. Nothing was changed"
    );

    step(Step::BackingUp);
    let folder = state.data.join("update");
    tokio::fs::create_dir_all(&folder).await?;
    for left in ["homewarp", "before.db", "failed", "pending"] {
        let _ = tokio::fs::remove_file(folder.join(left)).await;
    }
    let beside = folder.join("homewarp");
    tokio::fs::write(&beside, &fetched).await?;
    // The database as it is, whole, in a file of its own: what the helper
    // puts back if the new version reads it and then does not stay up.
    sqlx::query("VACUUM INTO ?")
        .bind(folder.join("before.db").to_string_lossy().into_owned())
        .execute(&state.db)
        .await
        .context("copying Homewarp's database")?;
    if servers {
        back_up_servers(state, to).await?;
    }

    step(Step::StartingAgain);
    let data = state.data.to_string_lossy();
    ensure!(
        !data.contains('\''),
        "the folder Homewarp keeps everything in has a quote in its name"
    );
    let script = SCRIPT
        .replace("@DIR@", &installed.folder)
        .replace("@DATA@", &data)
        .replace("@NAME@", &installed.name)
        .replace("@OLD@", VERSION)
        .replace("@NEW@", to);
    tokio::fs::write(folder.join("pending"), to).await?;
    let runtime = state.runtime.as_ref().context("Docker cannot be reached")?;
    let name = format!("{}-update", installed.name);
    runtime
        .run_apart(&Apart {
            name: &name,
            image: HELPER,
            command: vec!["sh".to_owned(), "-c".to_owned(), script],
            binds: vec![
                format!("{0}:{0}", installed.folder),
                "/var/run/docker.sock:/var/run/docker.sock".to_owned(),
            ],
        })
        .await
        .context("starting what puts the new version in place")
}

/// Backs every server up, as its Backups tab does, and waits for all of it.
/// A backup that fails stops the update: it was asked for so as to have one.
async fn back_up_servers(state: &AppState, to: &str) -> anyhow::Result<()> {
    let servers: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM servers ORDER BY id")
        .fetch_all(&state.db)
        .await?;
    let mut made = Vec::with_capacity(servers.len());
    for (id, name) in servers {
        match backups::begin(state, id, &format!("Before the update to {to}")).await {
            Ok(backup) => made.push((backup.id, name)),
            Err(_) => bail!("{name} could not be backed up, so nothing was changed"),
        }
    }
    let waited = async {
        for (id, name) in made {
            loop {
                let found: Option<(String,)> =
                    sqlx::query_as("SELECT state FROM backups WHERE id = ?")
                        .bind(id)
                        .fetch_optional(&state.db)
                        .await?;
                match found.as_ref().map(|(state,)| state.as_str()) {
                    Some("running") => tokio::time::sleep(Duration::from_secs(2)).await,
                    Some("done") => break,
                    _ => bail!("the backup of {name} failed, so nothing was changed"),
                }
            }
        }
        Ok(())
    };
    match tokio::time::timeout(PATIENCE_FOR_BACKUPS, waited).await {
        Ok(waited) => waited,
        Err(_) => bail!("the servers' backups took too long, so nothing was changed"),
    }
}

/// What there is to know about updates.
#[derive(Serialize, ToSchema)]
struct UpdateView {
    /// The version that is running.
    version: String,
    /// Which releases this Homewarp takes.
    channel: Channel,
    /// When it last looked for a newer one, in Unix seconds.
    checked_at: Option<i64>,
    /// The newest release for its channel, where the list of them could be read.
    newest: Option<String>,
    /// Whether that is newer than what is running.
    available: bool,
    /// Why the list could not be read, or why the last update could not be begun.
    problem: Option<String>,
    /// Whether this Homewarp can put a newer version in place of itself.
    installs: bool,
    /// Why it cannot, where it cannot.
    by_hand: Option<String>,
    /// The line that installs the newest release by hand, on the machine
    /// Homewarp runs on. Nothing where it is not known where releases are.
    line: Option<String>,
    /// The line that puts this release's Gate in place of an older one, run
    /// as root on a VPS.
    gate_line: Option<String>,
    /// An update that is being made now.
    updating: Option<Updating>,
    /// How the last one went.
    last: Option<Last>,
}

async fn view(state: &AppState) -> UpdateView {
    let channel: Channel = kept(&state.db, "update.channel").await.unwrap_or_default();
    let last: Option<Last> = kept(&state.db, "update.last").await;
    let installed = installed(state).await;
    let said = lock(&state.updates).clone();
    let newest = said
        .listed
        .as_ref()
        .and_then(|listed| listed.newest(channel))
        .map(str::to_owned);
    let available = newest
        .as_deref()
        .is_some_and(|newest| newer(newest, VERSION));
    let releases = state.releases.as_deref();
    UpdateView {
        version: VERSION.to_owned(),
        channel,
        checked_at: said.checked_at,
        available,
        problem: said.problem,
        installs: installed.is_ok(),
        by_hand: installed.err(),
        // A release's own script, which names its version: the one at the
        // top is the newest that was released, and a beta is not that.
        line: releases
            .zip(newest.as_deref().filter(|_| available))
            .map(|(releases, newest)| format!("curl -fsSL {releases}/{newest}/install.sh | sh")),
        gate_line: releases.map(|releases| {
            format!("curl -fsSL {releases}/{VERSION}/install-gate.sh | sh -s -- update")
        }),
        newest,
        updating: said.updating,
        last,
    }
}

/// Which version runs, whether a newer one is out, and whether this Homewarp
/// can put it in place itself. The one request the Updates page needs.
#[utoipa::path(
    get,
    path = "/api/v1/update",
    responses(
        (status = OK, body = UpdateView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn get_update(State(state): State<AppState>, _: Owner) -> Json<UpdateView> {
    Json(view(&state).await)
}

/// What can be set about updates.
#[derive(Deserialize, ToSchema)]
struct UpdateSettings {
    channel: Channel,
}

/// Says which releases this Homewarp takes: only what has been released, or
/// what is being tried before it is as well.
#[utoipa::path(
    put,
    path = "/api/v1/update",
    request_body = UpdateSettings,
    responses(
        (status = OK, body = UpdateView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn change_update(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(settings): Json<UpdateSettings>,
) -> Result<Json<UpdateView>, Problem> {
    keep(&state.db, "update.channel", &settings.channel).await?;
    let detail = match settings.channel {
        Channel::Stable => "stable",
        Channel::Beta => "beta",
    };
    audit::record(&state.db, &who, None, "update.channel", detail).await;
    Ok(Json(view(&state).await))
}

/// Looks for a newer release now, where Homewarp otherwise looks every few hours.
#[utoipa::path(
    post,
    path = "/api/v1/update/check",
    responses(
        (status = OK, body = UpdateView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn check_update(State(state): State<AppState>, _: Owner) -> Json<UpdateView> {
    check(&state).await;
    Json(view(&state).await)
}

/// What an update is asked with.
#[derive(Deserialize, ToSchema)]
struct InstallUpdate {
    /// Back every server up first, as its Backups tab does, and go on only
    /// once every backup is made. Homewarp's own database is copied either
    /// way: it is what is put back if the new version does not start.
    #[serde(default)]
    servers: bool,
}

/// Puts the newest release of this Homewarp's channel in place of the one
/// that runs. The answer comes at once; the update goes on, and asking how it
/// stands shows it. Homewarp stops answering for a moment near the end, and
/// answers again as the new version, or as the old one if the new one did not
/// start. Servers that are running go on running all the while.
#[utoipa::path(
    post,
    path = "/api/v1/update/install",
    request_body = InstallUpdate,
    responses(
        (status = ACCEPTED, body = UpdateView, description = "The update has begun."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "There is nothing newer, an update is being made already, or this Homewarp cannot replace itself."),
    )
)]
async fn install_update(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<InstallUpdate>,
) -> Result<(StatusCode, Json<UpdateView>), Problem> {
    if let Err(why) = installed(&state).await {
        return Err(Problem::Conflict(why.into()));
    }
    let channel: Channel = kept(&state.db, "update.channel").await.unwrap_or_default();
    let to = {
        let mut said = lock(&state.updates);
        if said.updating.is_some() {
            return Err(Problem::Conflict("An update is being made already.".into()));
        }
        let newest = said
            .listed
            .as_ref()
            .and_then(|listed| listed.newest(channel))
            .filter(|newest| newer(newest, VERSION))
            .map(str::to_owned);
        let Some(to) = newest else {
            return Err(Problem::Conflict(
                "There is no newer release to update to.".into(),
            ));
        };
        said.problem = None;
        said.updating = Some(Updating {
            to: to.clone(),
            step: Step::Fetching,
        });
        to
    };
    audit::record(&state.db, &who, None, "update.install", &to).await;
    let making = state.clone();
    let version = to.clone();
    tokio::spawn(async move {
        // Handed over, it stays as it is said to be until this Homewarp is
        // stopped: what comes of it is the next one's to say.
        if let Err(error) = update(&making, &version, asked.servers).await {
            tracing::warn!("The update to {version} was not made: {error:#}");
            let mut said = lock(&making.updates);
            said.updating = None;
            said.problem = Some(format!("The update to {version} was not made: {error:#}."));
        }
    });
    Ok((StatusCode::ACCEPTED, Json(view(&state).await)))
}

#[cfg(test)]
mod tests {
    use super::{Channel, Listed, SCRIPT, Version, checksum_of, end_of, hex, is_of, newer, signed};

    #[test]
    fn a_list_of_checksums_is_of_the_version_it_names_and_of_no_other() {
        // As `sha256sum homewarp-* VERSION` writes it, for a VERSION that holds "1.1.0\n".
        let named = hex(&<sha2::Sha256 as sha2::Digest>::digest(b"1.1.0\n"));
        let sums = format!("ade76660c2d5  homewarp-x86_64\n{named}  VERSION\n");
        assert!(is_of(&sums, "1.1.0"));
        // An older release's list, served in a newer one's place.
        assert!(!is_of(&sums, "1.2.0"));
        // A list from before versions were listed says nothing of which it is.
        assert!(!is_of("ade76660c2d5  homewarp-x86_64\n", "1.1.0"));
        // What goes into an address and a script is a version and nothing else.
        for odd in ["1.2.0-beta';id;'", "1.2.0-a/b", "1.2.0-a..b", "1.2.0-a$b"] {
            assert_eq!(Version::read(odd), None, "{odd}");
        }
        assert!(Version::read("1.2.0-beta.1").is_some());
        assert!(Version::read("1.2.0-rc-2").is_some());
    }

    #[test]
    fn versions_are_ordered_as_releases_are_numbered() {
        let in_order = [
            "0.9.9",
            "1.0.0-alpha",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.2.0-beta.1",
            "1.2.0",
            "1.10.0",
            "2.0.0",
        ];
        for pair in in_order.windows(2) {
            assert!(newer(pair[1], pair[0]), "{} then {}", pair[0], pair[1]);
            assert!(!newer(pair[0], pair[1]), "{} then {}", pair[0], pair[1]);
        }
        assert!(!newer("1.0.0", "1.0.0"));
        // A letter before it and how it was built after it order nothing.
        assert_eq!(Version::read("v1.2.3+abc"), Version::read("1.2.3"));
        for odd in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "one.two.three",
            "1.2.3-",
        ] {
            assert_eq!(Version::read(odd), None, "{odd}");
            // What is no version is no newer version.
            assert!(!newer(odd, "0.0.1"));
        }
    }

    #[test]
    fn a_channel_takes_its_own_newest_and_beta_takes_what_was_released_too() {
        let listed = Listed::read("stable 1.1.0\nbeta 1.2.0-beta.1\n");
        assert_eq!(listed.newest(Channel::Stable), Some("1.1.0"));
        assert_eq!(listed.newest(Channel::Beta), Some("1.2.0-beta.1"));
        // What was being tried has been released since, and nothing newer is tried yet.
        let listed = Listed::read("beta 1.2.0-beta.1\nstable 1.2.0\n");
        assert_eq!(listed.newest(Channel::Beta), Some("1.2.0"));
        // A list that says more than is known, and less.
        let listed = Listed::read("nightly 9.9.9\nstable not-a-version\nstable 1.0.0 extra\n\n");
        assert_eq!(listed, Listed::default());
        assert_eq!(listed.newest(Channel::Beta), None);
        let listed = Listed::read("beta 1.1.0-beta.1");
        assert_eq!(listed.newest(Channel::Stable), None);
        assert_eq!(listed.newest(Channel::Beta), Some("1.1.0-beta.1"));
    }

    #[test]
    fn a_list_is_believed_for_its_signature_and_for_nothing_else() {
        use ring::signature::{Ed25519KeyPair, KeyPair};

        let made = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(made.as_ref()).unwrap();
        let list = b"stable 1.1.0\n";
        let signature = pair.sign(list);
        let key = pair.public_key().as_ref();
        assert!(signed(key, list, signature.as_ref()));
        // The same list with another version in it, and a signature of another's.
        assert!(!signed(key, b"stable 9.9.9\n", signature.as_ref()));
        let other = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let other = Ed25519KeyPair::from_pkcs8(other.as_ref()).unwrap();
        assert!(!signed(key, list, other.sign(list).as_ref()));
        assert!(!signed(key, list, b""));
        assert!(!signed(b"not a key", list, signature.as_ref()));
    }

    #[test]
    fn a_program_is_held_against_the_checksum_its_release_lists_for_it() {
        let sums = "aa11  homewarp-arm64\nbb22  homewarp-x86_64\ncc33 *homewarp-gate-x86_64\n";
        assert_eq!(checksum_of(sums, "homewarp-x86_64"), Some("bb22"));
        assert_eq!(checksum_of(sums, "homewarp-gate-x86_64"), Some("cc33"));
        // Not one whose name only ends the same, and not one that is not listed.
        assert_eq!(checksum_of(sums, "x86_64"), None);
        assert_eq!(checksum_of(sums, "homewarp-riscv"), None);
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }

    #[test]
    fn the_end_of_what_was_written_down_is_short_enough_to_show() {
        let log: String = (1..=40).map(|line| format!("line {line}\n\n")).collect();
        let end = end_of(&log);
        assert_eq!(end.lines().count(), 12);
        assert!(end.ends_with("line 40"));
        assert!(end_of(&"é".repeat(5000)).len() <= 2000);
        assert_eq!(end_of(""), "");
    }

    #[test]
    fn the_helper_is_told_everything_it_uses() {
        let told = ["@DIR@", "@DATA@", "@NAME@", "@OLD@", "@NEW@"];
        let filled = told
            .iter()
            .fold(SCRIPT.to_owned(), |script, name| script.replace(name, "x"));
        assert!(!filled.contains('@'), "something is left to fill in");
        for name in told {
            assert!(SCRIPT.contains(name), "{name}");
        }
    }
}
