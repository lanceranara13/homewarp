//! Backups of a server's files (PLAN.md §11, Phase 4). A backup is one file,
//! a tar packed with Zstandard, kept beside the servers' files and not among
//! them: a server cannot reach its own backups, and so neither can whoever
//! takes a server over.
//!
//! Making one takes minutes for a large world, so nothing waits for it: the
//! request is answered at once, the work goes on, and the row says how it
//! went. Putting one back is the same, and the server's console says how.

use std::{
    io,
    path::{Path, PathBuf},
};

use axum::{
    Json,
    extract::{Path as InPath, State},
    http::StatusCode,
    response::Response,
};
use homewarp_runtime::{ServerDir, Unpacked};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    accounts::{self, Permission},
    api::{AppState, Problem, ProblemBody, SignedIn, blocking},
    audit, auth, files,
    runtime::files_at,
    servers::MISSING,
    store,
};

const LONGEST_NAME: usize = 60;
/// More than a home's disk is likely to hold of anything worth backing up.
const MOST_KEPT: i64 = 20;

const NO_BACKUP: Problem = Problem::NotFound("There is no such backup.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_backups, make_backup))
        .routes(routes!(keep_backups))
        .routes(routes!(download_backup))
        .routes(routes!(restore_backup))
        .routes(routes!(copy_backup))
        .routes(routes!(remove_backup))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum BackupState {
    /// It is being made.
    Running,
    Done,
    Failed,
}

impl BackupState {
    fn read(text: &str) -> Self {
        match text {
            "running" => Self::Running,
            "done" => Self::Done,
            _ => Self::Failed,
        }
    }
}

/// Where a backup's copy in the store elsewhere stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum Stored {
    /// It is on its way there.
    Copying,
    Copied,
    Failed,
}

impl Stored {
    fn read(text: &str) -> Self {
        match text {
            "copying" => Self::Copying,
            "copied" => Self::Copied,
            _ => Self::Failed,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct Backup {
    pub(crate) id: i64,
    name: String,
    state: BackupState,
    /// How long the file is. Nothing until it is done.
    size_bytes: i64,
    /// Why it failed, if it did.
    problem: Option<String>,
    /// When it was begun, in Unix seconds.
    created_at: i64,
    finished_at: Option<i64>,
    /// Where its copy in the store elsewhere stands. Nothing where no copy
    /// was asked for: there was no store when it was made.
    stored: Option<Stored>,
    /// Why it was not copied, if it was not.
    stored_problem: Option<String>,
}

/// A server's backups, and how many of them are kept.
#[derive(Serialize, ToSchema)]
struct Backups {
    /// When one more is done, the oldest beyond this many go.
    kept: i64,
    /// Whether there is a store elsewhere that backups are copied to.
    store: bool,
    /// The newest first.
    backups: Vec<Backup>,
}

#[derive(Deserialize, ToSchema)]
struct NewBackup {
    /// What to call it. Something is made up if nothing is given.
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, ToSchema)]
struct Kept {
    /// From 1 to 20.
    kept: i64,
}

/// Where a backup is kept.
fn file_of(data: &Path, uuid: &str, id: i64) -> PathBuf {
    data.join("backups")
        .join(uuid)
        .join(format!("{id}.tar.zst"))
}

async fn uuid_of(db: &SqlitePool, server_id: i64) -> Result<String, Problem> {
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_optional(db)
        .await?;
    uuid.ok_or(MISSING)
}

/// A backup of this server that is done, by its id: its name and when it was begun.
async fn done(db: &SqlitePool, server_id: i64, id: i64) -> Result<(String, i64), Problem> {
    let found: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT name, state, created_at FROM backups WHERE id = ? AND server_id = ?",
    )
    .bind(id)
    .bind(server_id)
    .fetch_optional(db)
    .await?;
    match found {
        None => Err(NO_BACKUP),
        Some((name, state, created_at)) if state == "done" => Ok((name, created_at)),
        Some(_) => Err(Problem::Conflict(
            "That backup was not finished, so there is nothing in it to use.".into(),
        )),
    }
}

/// What went wrong with a file, as the Backups page says it.
fn said(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::StorageFull => {
            "There was not enough room left on this machine's disk.".to_owned()
        }
        _ => format!("Homewarp could not do it: {error}."),
    }
}

/// Writes a server's files to a backup's file. Returns how long that came to be.
fn write(from: &Path, to: &Path) -> io::Result<u64> {
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let (uid, gid) = files::owner();
    let files = ServerDir::open(from, uid, gid)?;
    let room = files.free()?.saturating_sub(files::KEPT_FREE);
    files.back_up(std::fs::File::create(to)?, room)
}

/// Makes a server's files what a backup's file says they were.
fn put_back(from: &Path, to: &Path) -> io::Result<Unpacked> {
    let (uid, gid) = files::owner();
    ServerDir::open(to, uid, gid)?.restore(std::fs::File::open(from)?, files::KEPT_FREE)
}

/// Begins a backup of a server and returns at once, with the backup as it
/// stands: being made. For a request, and for a schedule.
pub(crate) async fn begin(state: &AppState, server_id: i64, name: &str) -> Result<Backup, Problem> {
    let uuid = uuid_of(&state.db, server_id).await?;
    let name = match name.trim() {
        "" => "Backup".to_owned(),
        given => given.to_owned(),
    };
    let created_at = auth::now();
    let inserted = sqlx::query(
        "INSERT INTO backups (server_id, name, state, created_at) VALUES (?, ?, 'running', ?)",
    )
    .bind(server_id)
    .bind(&name)
    .bind(created_at)
    .execute(&state.db)
    .await;
    let id = match inserted {
        Ok(inserted) => inserted.last_insert_rowid(),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(Problem::Conflict(
                "A backup of this server is being made already.".into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    tokio::spawn(make(state.clone(), server_id, uuid, id));
    Ok(Backup {
        id,
        name,
        state: BackupState::Running,
        size_bytes: 0,
        problem: None,
        created_at,
        finished_at: None,
        stored: None,
        stored_problem: None,
    })
}

/// A server's name as part of a name in a bucket: small letters and digits,
/// with a hyphen for whatever else there was.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for letter in name.chars() {
        match letter {
            letter if letter.is_ascii_alphanumeric() => slug.push(letter.to_ascii_lowercase()),
            _ if !slug.ends_with('-') => slug.push('-'),
            _ => {}
        }
    }
    slug.trim_matches('-').to_owned()
}

/// Copies a finished backup to the store elsewhere, if there is one, and
/// writes down how that went. Where it does not go, the owner is told: a
/// copy that is thought to be there and is not is worse than none.
async fn copy(state: &AppState, server_id: i64, uuid: &str, id: i64) {
    let Some(store) = store::kept(&state.db).await else {
        return;
    };
    let found: Result<Option<(String, String, i64)>, _> = sqlx::query_as(
        "SELECT servers.name, backups.name, backups.created_at
         FROM backups JOIN servers ON servers.id = backups.server_id
         WHERE backups.id = ? AND backups.state = 'done'",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;
    let Ok(Some((server, name, created_at))) = found else {
        return;
    };
    // By the server's name, for whoever looks in the bucket, and by enough of
    // its id to tell two servers of one name apart.
    let key = store.key(&format!(
        "{}-{}/{}-{id}.tar.zst",
        slug(&server),
        uuid.get(..8).unwrap_or(uuid),
        files::stamp(created_at)
    ));
    let begun = sqlx::query(
        "UPDATE backups SET stored = 'copying', stored_key = ?, stored_problem = NULL WHERE id = ?",
    )
    .bind(&key)
    .bind(id)
    .execute(&state.db)
    .await;
    if let Err(error) = begun {
        tracing::error!("that backup {id} is being copied could not be written down: {error}");
        return;
    }
    let copied = store.put(&key, &file_of(&state.data, uuid, id)).await;
    let written = match &copied {
        Ok(()) => sqlx::query("UPDATE backups SET stored = 'copied' WHERE id = ?").bind(id),
        Err(problem) => {
            sqlx::query("UPDATE backups SET stored = 'failed', stored_problem = ? WHERE id = ?")
                .bind(problem.clone())
                .bind(id)
        }
    };
    if let Err(error) = written.execute(&state.db).await {
        tracing::error!("how copying backup {id} went could not be written down: {error}");
    }
    if let Err(problem) = copied {
        let detail = format!("{name}: {problem}");
        audit::record_by_homewarp(&state.db, Some(server_id), "backup.copy_failed", &detail).await;
    }
}

/// Takes a backup's copy out of the store elsewhere, where it has one. Not
/// waited for, and said in the log if it does not go: the backup itself is
/// gone either way.
fn uncopy(state: &AppState, key: Option<String>) {
    let Some(key) = key else {
        return;
    };
    let db = state.db.clone();
    tokio::spawn(async move {
        let Some(store) = store::kept(&db).await else {
            return;
        };
        if let Err(problem) = store.remove(&key).await {
            tracing::warn!("a backup's copy was left in the store ({key}): {problem}");
        }
    });
}

/// Makes the backup a row was begun for, writes down how it went, and lets
/// the oldest go if there are now more than are kept.
async fn make(state: AppState, server_id: i64, uuid: String, id: i64) {
    let to = file_of(&state.data, &uuid, id);
    let (from, file) = (files_at(&state.data, &uuid), to.clone());
    let written = match blocking(move || write(&from, &file)).await {
        Ok(Ok(size)) => Ok(size),
        Ok(Err(error)) => Err(said(&error)),
        Err(_) => Err("Homewarp could not do it. Its log has the details.".to_owned()),
    };
    let finished = match &written {
        Ok(size) => sqlx::query(
            "UPDATE backups SET state = 'done', size_bytes = ?, finished_at = ? WHERE id = ?",
        )
        .bind(i64::try_from(*size).unwrap_or(i64::MAX))
        .bind(auth::now())
        .bind(id),
        Err(problem) => {
            let _ = tokio::fs::remove_file(&to).await;
            sqlx::query(
                "UPDATE backups SET state = 'failed', problem = ?, finished_at = ? WHERE id = ?",
            )
            .bind(problem.clone())
            .bind(auth::now())
            .bind(id)
        }
    };
    if let Err(error) = finished.execute(&state.db).await {
        tracing::error!("how backup {id} went could not be written down: {error}");
        return;
    }
    if written.is_err() {
        return;
    }
    if let Err(error) = thin(&state, server_id, &uuid, id).await {
        tracing::error!("the old backups of server {server_id} could not be cleared: {error}");
    }
    copy(&state, server_id, &uuid, id).await;
}

/// Lets go of what a backup that has just been done makes old: the finished
/// ones beyond those that are kept, and the failed ones before it, whose
/// news it has overtaken.
async fn thin(
    state: &AppState,
    server_id: i64,
    uuid: &str,
    newest: i64,
) -> Result<(), sqlx::Error> {
    let old: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id, stored_key FROM backups WHERE server_id = ?1 AND state = 'done'
         ORDER BY id DESC LIMIT -1 OFFSET (SELECT backups_kept FROM servers WHERE id = ?1)",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await?;
    for (id, stored_key) in old {
        let _ = tokio::fs::remove_file(file_of(&state.data, uuid, id)).await;
        // Its copy elsewhere goes with it: what is kept there is what is kept here.
        uncopy(state, stored_key);
        sqlx::query("DELETE FROM backups WHERE id = ?")
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    sqlx::query("DELETE FROM backups WHERE server_id = ? AND state = 'failed' AND id < ?")
        .bind(server_id)
        .bind(newest)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// A backup that was being made when Homewarp last stopped is not being made
/// any more. Done when Homewarp starts: says so in its row, and drops what
/// there is of its file.
pub(crate) async fn settle(db: &SqlitePool, data: &Path) -> Result<(), sqlx::Error> {
    let cut_short: Vec<(i64, String)> = sqlx::query_as(
        "SELECT backups.id, servers.uuid FROM backups JOIN servers ON servers.id = backups.server_id
         WHERE backups.state = 'running'",
    )
    .fetch_all(db)
    .await?;
    for (id, uuid) in cut_short {
        let _ = tokio::fs::remove_file(file_of(data, &uuid, id)).await;
        sqlx::query(
            "UPDATE backups SET state = 'failed', finished_at = ?,
                    problem = 'Homewarp stopped while this backup was being made.'
             WHERE id = ?",
        )
        .bind(auth::now())
        .bind(id)
        .execute(db)
        .await?;
    }
    // Nor is one that was on its way to the store still on its way there.
    sqlx::query(
        "UPDATE backups SET stored = 'failed',
                stored_problem = 'Homewarp stopped while this backup was being copied.'
         WHERE stored = 'copying'",
    )
    .execute(db)
    .await?;
    Ok(())
}

/// A server's backups, the newest first, and how many are kept. The one
/// request the Backups tab needs.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/backups",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = OK, body = Backups),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
    )
)]
async fn list_backups(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath(id): InPath<i64>,
) -> Result<Json<Backups>, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    let kept: Option<i64> = sqlx::query_scalar("SELECT backups_kept FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    type Row = (
        i64,
        String,
        String,
        i64,
        Option<String>,
        i64,
        Option<i64>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, state, size_bytes, problem, created_at, finished_at, stored,
                stored_problem
         FROM backups WHERE server_id = ? ORDER BY id DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let backups = rows
        .into_iter()
        .map(
            |(
                id,
                name,
                state,
                size_bytes,
                problem,
                created_at,
                finished_at,
                stored,
                stored_problem,
            )| Backup {
                id,
                name,
                state: BackupState::read(&state),
                size_bytes,
                problem,
                created_at,
                finished_at,
                stored: stored.as_deref().map(Stored::read),
                stored_problem,
            },
        )
        .collect();
    Ok(Json(Backups {
        kept: kept.ok_or(MISSING)?,
        store: store::kept(&state.db).await.is_some(),
        backups,
    }))
}

/// Begins a backup of everything in a server's files. The answer comes at
/// once, with the backup as it stands; asking for the backups again shows how
/// it went. A server that runs meanwhile goes on running.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/backups",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = NewBackup,
    responses(
        (status = ACCEPTED, body = Backup, description = "The backup is being made."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "A backup of it is being made already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The name will not do."),
    )
)]
async fn make_backup(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath(id): InPath<i64>,
    Json(new): Json<NewBackup>,
) -> Result<(StatusCode, Json<Backup>), Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    if new.name.trim().chars().count() > LONGEST_NAME {
        return Err(Problem::Invalid(
            "A backup's name is 60 characters at the most.".into(),
        ));
    }
    let backup = begin(&state, id, &new.name).await?;
    audit::record(&state.db, &who, Some(id), "backup.create", &backup.name).await;
    Ok((StatusCode::ACCEPTED, Json(backup)))
}

/// Says how many finished backups of a server are kept. It counts from the
/// next backup that is done: none goes because of this alone.
#[utoipa::path(
    put,
    path = "/api/v1/servers/{id}/backups/kept",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = Kept,
    responses(
        (status = NO_CONTENT, description = "That many are kept from now on."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "It is not a number from 1 to 20."),
    )
)]
async fn keep_backups(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath(id): InPath<i64>,
    Json(asked): Json<Kept>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    if !(1..=MOST_KEPT).contains(&asked.kept) {
        return Err(Problem::Invalid(
            "Homewarp keeps from 1 to 20 backups of a server.".into(),
        ));
    }
    let changed = sqlx::query("UPDATE servers SET backups_kept = ? WHERE id = ?")
        .bind(asked.kept)
        .bind(id)
        .execute(&state.db)
        .await?;
    if changed.rows_affected() == 0 {
        return Err(MISSING);
    }
    let detail = asked.kept.to_string();
    audit::record(&state.db, &who, Some(id), "backup.keep", &detail).await;
    Ok(StatusCode::NO_CONTENT)
}

/// A backup's file as it is: a tar packed with Zstandard, for the browser to save.
/// It asks for `Files` as well as `Backups`: it is all of the server's files.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/backups/{backup_id}/download",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("backup_id" = i64, Path, description = "The backup's id."),
    ),
    responses(
        (status = OK, body = Vec<u8>, content_type = "application/octet-stream", description = "The backup, as an attachment."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such backup."),
        (status = CONFLICT, body = ProblemBody, description = "The backup was not finished."),
    )
)]
async fn download_backup(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath((id, backup_id)): InPath<(i64, i64)>,
) -> Result<Response, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    // A backup is every file of the server. Whoever takes one away reads
    // them all, which is what `Files` is for.
    accounts::may(&state.db, &who, id, Some(Permission::Files)).await?;
    let uuid = uuid_of(&state.db, id).await?;
    let (name, created_at) = done(&state.db, id, backup_id).await?;
    let path = file_of(&state.data, &uuid, backup_id);
    let opened = blocking(move || {
        let file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        io::Result::Ok((file, size))
    });
    let (file, size) = opened.await?.map_err(|error| {
        Problem::Internal(anyhow::Error::new(error).context("opening a backup's file"))
    })?;
    audit::record(&state.db, &who, Some(id), "backup.download", &name).await;
    let saved_as = format!("{name}-{}.tar.zst", files::stamp(created_at));
    Ok(files::sent(file, size, &saved_as))
}

/// Makes a server's files what a backup says they were: everything in them
/// now goes, and the backup is unpacked in their place. The server has to be
/// stopped. The answer comes at once; the server shows as being restored
/// until it is done, and its console says how it went.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/backups/{backup_id}/restore",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("backup_id" = i64, Path, description = "The backup's id."),
    ),
    responses(
        (status = ACCEPTED, description = "The backup is being put back."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such backup."),
        (status = CONFLICT, body = ProblemBody, description = "The server is running, or the backup was not finished."),
    )
)]
async fn restore_backup(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath((id, backup_id)): InPath<(i64, i64)>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    let uuid = uuid_of(&state.db, id).await?;
    let (name, _) = done(&state.db, id, backup_id).await?;
    // Held from here until the work is over, so that nothing starts the
    // server on files that are half way back.
    let hold = match &state.runtime {
        Some(runtime) => Some(runtime.hold(id).map_err(|now| {
            match now {
                None => MISSING,
                Some(now) => Problem::Conflict(
                    format!(
                        "This server is {}. Stop it before putting a backup back.",
                        now.in_words()
                    )
                    .into(),
                ),
            }
        })?),
        None => None,
    };
    audit::record(&state.db, &who, Some(id), "backup.restore", &name).await;
    let (from, to) = (
        file_of(&state.data, &uuid, backup_id),
        files_at(&state.data, &uuid),
    );
    tokio::spawn(async move {
        let say = |line: String| match &hold {
            Some(hold) => hold.say(line),
            None => tracing::info!("server {id}: {line}"),
        };
        say(format!(
            "Putting the backup \"{name}\" back. What is here now goes first."
        ));
        say(match blocking(move || put_back(&from, &to)).await {
            Ok(Ok(back)) if back.files == 1 => "The backup is back: 1 file.".to_owned(),
            Ok(Ok(back)) => format!("The backup is back: {} files.", back.files),
            Ok(Err(error)) => format!("The backup could not be put back. {}", said(&error)),
            Err(_) => {
                "The backup could not be put back. Homewarp's log has the details.".to_owned()
            }
        });
    });
    Ok(StatusCode::ACCEPTED)
}

/// Copies a finished backup to the store elsewhere: one that was made before
/// there was a store, or whose copy did not arrive. The answer comes at once,
/// and asking for the backups again shows how it went.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/backups/{backup_id}/copy",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("backup_id" = i64, Path, description = "The backup's id."),
    ),
    responses(
        (status = ACCEPTED, description = "The backup is being copied."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such backup."),
        (status = CONFLICT, body = ProblemBody, description = "There is no store, the backup was not finished, or it is being copied already."),
    )
)]
async fn copy_backup(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath((id, backup_id)): InPath<(i64, i64)>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    let uuid = uuid_of(&state.db, id).await?;
    let (name, _) = done(&state.db, id, backup_id).await?;
    if store::kept(&state.db).await.is_none() {
        return Err(Problem::Conflict(
            "There is no store to copy it to. The owner sets one under Settings.".into(),
        ));
    }
    // Marked here and not when the work gets to it, so that a second click
    // finds the first one counted.
    let begun = sqlx::query(
        "UPDATE backups SET stored = 'copying', stored_problem = NULL
         WHERE id = ? AND (stored IS NULL OR stored != 'copying')",
    )
    .bind(backup_id)
    .execute(&state.db)
    .await?;
    if begun.rows_affected() == 0 {
        return Err(Problem::Conflict(
            "That backup is being copied already.".into(),
        ));
    }
    audit::record(&state.db, &who, Some(id), "backup.copy", &name).await;
    tokio::spawn(async move { copy(&state, id, &uuid, backup_id).await });
    Ok(StatusCode::ACCEPTED)
}

/// Deletes a backup. It cannot be brought back.
#[utoipa::path(
    delete,
    path = "/api/v1/servers/{id}/backups/{backup_id}",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("backup_id" = i64, Path, description = "The backup's id."),
    ),
    responses(
        (status = NO_CONTENT, description = "The backup is gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such backup."),
        (status = CONFLICT, body = ProblemBody, description = "The backup is still being made."),
    )
)]
async fn remove_backup(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    InPath((id, backup_id)): InPath<(i64, i64)>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Backups)).await?;
    let uuid = uuid_of(&state.db, id).await?;
    let found: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT name, state, stored, stored_key FROM backups WHERE id = ? AND server_id = ?",
    )
    .bind(backup_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (name, now, stored, stored_key) = found.ok_or(NO_BACKUP)?;
    if now == "running" {
        return Err(Problem::Conflict(
            "That backup is still being made. It can be deleted once it is done.".into(),
        ));
    }
    if stored.as_deref() == Some("copying") {
        return Err(Problem::Conflict(
            "That backup is being copied to the store. It can be deleted once that is done.".into(),
        ));
    }
    let _ = tokio::fs::remove_file(file_of(&state.data, &uuid, backup_id)).await;
    uncopy(&state, stored_key);
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(backup_id)
        .execute(&state.db)
        .await?;
    audit::record(&state.db, &who, Some(id), "backup.remove", &name).await;
    Ok(StatusCode::NO_CONTENT)
}
