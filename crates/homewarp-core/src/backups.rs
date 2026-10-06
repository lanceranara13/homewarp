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

#[derive(Serialize, ToSchema)]
pub(crate) struct Backup {
    id: i64,
    name: String,
    state: BackupState,
    /// How long the file is. Nothing until it is done.
    size_bytes: i64,
    /// Why it failed, if it did.
    problem: Option<String>,
    /// When it was begun, in Unix seconds.
    created_at: i64,
    finished_at: Option<i64>,
}

/// A server's backups, and how many of them are kept.
#[derive(Serialize, ToSchema)]
struct Backups {
    /// When one more is done, the oldest beyond this many go.
    kept: i64,
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
    })
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
    if written.is_ok()
        && let Err(error) = thin(&state, server_id, &uuid, id).await
    {
        tracing::error!("the old backups of server {server_id} could not be cleared: {error}");
    }
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
    let old: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM backups WHERE server_id = ?1 AND state = 'done'
         ORDER BY id DESC LIMIT -1 OFFSET (SELECT backups_kept FROM servers WHERE id = ?1)",
    )
    .bind(server_id)
    .fetch_all(&state.db)
    .await?;
    for id in old {
        let _ = tokio::fs::remove_file(file_of(&state.data, uuid, id)).await;
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
    type Row = (i64, String, String, i64, Option<String>, i64, Option<i64>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, state, size_bytes, problem, created_at, finished_at
         FROM backups WHERE server_id = ? ORDER BY id DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let backups = rows
        .into_iter()
        .map(
            |(id, name, state, size_bytes, problem, created_at, finished_at)| Backup {
                id,
                name,
                state: BackupState::read(&state),
                size_bytes,
                problem,
                created_at,
                finished_at,
            },
        )
        .collect();
    Ok(Json(Backups {
        kept: kept.ok_or(MISSING)?,
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
    let found: Option<(String, String)> =
        sqlx::query_as("SELECT name, state FROM backups WHERE id = ? AND server_id = ?")
            .bind(backup_id)
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (name, now) = found.ok_or(NO_BACKUP)?;
    if now == "running" {
        return Err(Problem::Conflict(
            "That backup is still being made. It can be deleted once it is done.".into(),
        ));
    }
    let _ = tokio::fs::remove_file(file_of(&state.data, &uuid, backup_id)).await;
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(backup_id)
        .execute(&state.db)
        .await?;
    audit::record(&state.db, &who, Some(id), "backup.remove", &name).await;
    Ok(StatusCode::NO_CONTENT)
}
