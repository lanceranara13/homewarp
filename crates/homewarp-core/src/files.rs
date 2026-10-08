//! A server's files, as the panel's file manager reaches them (PLAN.md §11,
//! Phase 4). Every path a request names is opened through
//! [`homewarp_runtime::ServerDir`], which holds a read or a write inside the
//! server's own directory whatever the path, or a link on the way, says
//! (PLAN.md §6).

use std::{borrow::Cow, fmt::Write as _, io, sync::Arc};

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{
        HeaderValue, StatusCode,
        header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS},
    },
    response::{IntoResponse, Response},
};
use futures_util::{StreamExt, stream};
use homewarp_runtime::{Kind, ServerDir, running_as};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    accounts::{self, Permission},
    api::{AppState, Problem, ProblemBody, SignedIn, User, blocking},
    audit, auth, clock,
    runtime::{USER, files_at},
    servers::MISSING,
};

/// The longest file the editor opens. A log is the usual long one, and this is
/// a long log.
const LONGEST_TEXT: u64 = 4 << 20;
/// How much of the disk is left alone whatever is uploaded, packed or
/// unpacked. The machine has other things to keep alive, and Homewarp's own
/// database is on the same disk.
pub(crate) const KEPT_FREE: u64 = 1 << 30;
/// How much of a download is read at a time.
const CHUNK: usize = 64 << 10;

pub(crate) const NO_ROOM: Problem = Problem::Conflict(Cow::Borrowed(
    "There is not enough room left on this machine's disk for that.",
));

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_files))
        .routes(routes!(read_file, write_file))
        .routes(routes!(download_file))
        .routes(routes!(make_folder))
        .routes(routes!(move_file))
        .routes(routes!(remove_files))
        .routes(routes!(pack_files))
        .routes(routes!(unpack_file))
        .routes(routes!(disk_use))
}

/// Where in a server's files a request points.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct At {
    /// A path from the top of the server's files. Empty, or left out, is the top itself.
    #[serde(default)]
    path: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum FileKind {
    File,
    Folder,
    /// A symbolic link. Opened, it is what it leads to, if that is in the server's files.
    Link,
    /// A pipe, a socket, a device: nothing there is anything to do with here.
    Other,
}

/// One thing in a folder.
#[derive(Serialize, ToSchema)]
struct FileEntry {
    name: String,
    kind: FileKind,
    /// In bytes. Nothing for what is not a file.
    size: u64,
    /// When it was last changed, in Unix seconds.
    modified: i64,
}

#[derive(Serialize, ToSchema)]
struct FileText {
    text: String,
}

#[derive(Deserialize, ToSchema)]
struct FolderRequest {
    /// Where the new folder goes. The folders it sits in are made with it.
    path: String,
}

#[derive(Deserialize, ToSchema)]
struct MoveRequest {
    from: String,
    /// Where it goes, its own name included: the same folder for a new name.
    to: String,
}

#[derive(Deserialize, ToSchema)]
struct RemoveRequest {
    /// Files, and folders with all that is in them.
    paths: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
struct PackRequest {
    /// The folder the archive is made in.
    folder: String,
    /// What goes into it: names in that folder.
    names: Vec<String>,
}

#[derive(Serialize, ToSchema)]
struct Packed {
    /// The archive's name, in the folder it was asked for in.
    name: String,
}

#[derive(Deserialize, ToSchema)]
struct UnpackRequest {
    /// A zip, a tar or a gzipped tar. It is unpacked into the folder it is in.
    path: String,
}

#[derive(Serialize, ToSchema)]
struct Unpacked {
    /// How many files were written.
    files: u64,
    /// How many entries were left out: those that would have gone outside the
    /// server's files, and what is neither a file, a folder nor a link.
    skipped: u64,
}

#[derive(Serialize, ToSchema)]
struct DiskUse {
    /// What the server's files take of the disk.
    used_bytes: u64,
    /// What is left on the disk they are on.
    free_bytes: u64,
}

/// Who a server's files belong to. A Homewarp that is not root cannot give a
/// file away, so there what it writes stays its own: the case of the tests.
pub(crate) fn owner() -> (u32, u32) {
    match running_as() {
        (0, _) => (USER, USER),
        own => own,
    }
}

/// What the file system said, in words.
fn problem(error: io::Error) -> Problem {
    use io::ErrorKind as Said;
    match error.kind() {
        Said::NotFound => Problem::NotFound("There is no such file or folder."),
        // What `cap-std` says to a path, or to a link in one, that leads out.
        Said::PermissionDenied => Problem::Forbidden("That leads out of this server's files."),
        Said::AlreadyExists => Problem::Conflict("Something by that name is there already.".into()),
        Said::IsADirectory => Problem::Invalid("That is a folder, not a file.".into()),
        Said::NotADirectory => Problem::Invalid("That is a file, not a folder.".into()),
        Said::InvalidInput | Said::InvalidFilename => {
            Problem::Invalid("That is not the path of something in this server's files.".into())
        }
        Said::FileTooLarge => {
            Problem::Invalid("That file is too long to open here. Download it instead.".into())
        }
        Said::InvalidData => Problem::Invalid("That file is not text. Download it instead.".into()),
        Said::Unsupported => Problem::Invalid(
            "Homewarp could not read that file as a zip, tar or tar.gz archive.".into(),
        ),
        Said::StorageFull => NO_ROOM,
        Said::QuotaExceeded => Problem::Conflict(
            "There is more there than Homewarp takes at once: a folder of over a hundred thousand names, or an archive of over a million files.".into(),
        ),
        _ => Problem::Internal(
            anyhow::Error::new(error).context("reading or writing a server's files"),
        ),
    }
}

/// What reading a broken archive ends in has no one kind: each library says it
/// its own way. Here they are all the one that means "not an archive".
fn unreadable(error: io::Error) -> io::Error {
    use io::ErrorKind::{InvalidData, Other, UnexpectedEof, Unsupported};
    match error.kind() {
        InvalidData | UnexpectedEof | Other => Unsupported.into(),
        _ => error,
    }
}

/// Opens a server's files.
pub(crate) async fn files_of(
    state: &AppState,
    who: &User,
    id: i64,
) -> Result<Arc<ServerDir>, Problem> {
    accounts::may(&state.db, who, id, Some(Permission::Files)).await?;
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let path = files_at(&state.data, &uuid.ok_or(MISSING)?);
    let (uid, gid) = owner();
    let files = blocking(move || ServerDir::open(&path, uid, gid)).await?;
    Ok(Arc::new(files.map_err(problem)?))
}

/// Does something with a server's files, off the async threads, and puts what
/// the file system said to it in words.
pub(crate) async fn with<T: Send + 'static>(
    files: &Arc<ServerDir>,
    work: impl FnOnce(&ServerDir) -> io::Result<T> + Send + 'static,
) -> Result<T, Problem> {
    let files = Arc::clone(files);
    blocking(move || work(&files)).await?.map_err(problem)
}

/// How many bytes may still be written to a server's files: what is free on
/// the disk, less what is kept free.
pub(crate) async fn room(files: &Arc<ServerDir>) -> Result<u64, Problem> {
    let free = with(files, |files| files.free()).await?;
    Ok(free.saturating_sub(KEPT_FREE))
}

/// Writes a request's body to a file as it arrives, and stops at `room` bytes.
async fn receive(file: std::fs::File, body: Body, mut room: u64) -> Result<(), Problem> {
    let mut file = tokio::fs::File::from_std(file);
    let mut chunks = body.into_data_stream();
    while let Some(chunk) = chunks.next().await {
        // The browser gave up, or the connection did.
        let chunk = chunk.map_err(|_| Problem::Invalid("The upload was cut off.".into()))?;
        room = room.checked_sub(chunk.len() as u64).ok_or(NO_ROOM)?;
        file.write_all(&chunk).await.map_err(problem)?;
    }
    file.flush().await.map_err(problem)
}

/// The header that has a browser save a file under its name, and not show it.
/// A server's files are whatever a server or a plugin wrote, and a page among
/// them must never open as a page of the panel's.
fn attachment(name: &str) -> HeaderValue {
    let kept = |byte: u8| byte.is_ascii_alphanumeric() || b"._-".contains(&byte);
    let plain: String = name
        .bytes()
        .map(|byte| match byte {
            byte if kept(byte) || byte == b' ' => byte as char,
            _ => '_',
        })
        .collect();
    let mut exact = String::new();
    for byte in name.bytes() {
        if kept(byte) {
            exact.push(byte as char);
        } else {
            let _ = write!(exact, "%{byte:02X}");
        }
    }
    let header = format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{exact}");
    HeaderValue::from_str(&header).unwrap_or(HeaderValue::from_static("attachment"))
}

/// A moment as a file's name can carry it, `2026-10-06-153045`, in UTC.
pub(crate) fn stamp(unix: i64) -> String {
    let (days, second) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    let (year, month, day) = clock::civil(days);
    format!(
        "{year:04}-{month:02}-{day:02}-{:02}{:02}{:02}",
        second / 3600,
        second / 60 % 60,
        second % 60
    )
}

/// What is in one folder of a server's files: folders first, then by name.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/files",
    params(("id" = i64, Path, description = "The server's id."), At),
    responses(
        (status = OK, body = Vec<FileEntry>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such folder."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not a folder."),
    )
)]
async fn list_files(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Query(at): Query<At>,
) -> Result<Json<Vec<FileEntry>>, Problem> {
    let files = files_of(&state, &who, id).await?;
    let entries = with(&files, move |files| files.list(&at.path)).await?;
    let entries = entries
        .into_iter()
        .map(|entry| FileEntry {
            name: entry.name,
            kind: match entry.kind {
                Kind::File => FileKind::File,
                Kind::Folder => FileKind::Folder,
                Kind::Link => FileKind::Link,
                Kind::Other => FileKind::Other,
            },
            size: entry.size,
            modified: entry.modified,
        })
        .collect();
    Ok(Json(entries))
}

/// A file's text, for the editor. A file that is not text, or is longer than
/// four megabytes, is refused: those are for downloading.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/files/content",
    params(("id" = i64, Path, description = "The server's id."), At),
    responses(
        (status = OK, body = FileText),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such file."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not a file of text, or it is too long."),
    )
)]
async fn read_file(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Query(at): Query<At>,
) -> Result<Json<FileText>, Problem> {
    let files = files_of(&state, &who, id).await?;
    let text = with(&files, move |files| files.text(&at.path, LONGEST_TEXT)).await?;
    Ok(Json(FileText { text }))
}

/// Writes a file: an upload, or what the editor saves. The body is the file as
/// it is. It takes the place of a file that is there by that name, and only
/// once all of it has arrived; the folders it sits in are made if they are not there.
#[utoipa::path(
    put,
    path = "/api/v1/servers/{id}/files/content",
    params(("id" = i64, Path, description = "The server's id."), At),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The file's bytes."),
    responses(
        (status = NO_CONTENT, description = "The file is there."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The disk has no room for it."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is no path for a file, or the upload was cut off."),
    )
)]
async fn write_file(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Query(at): Query<At>,
    body: Body,
) -> Result<StatusCode, Problem> {
    let files = files_of(&state, &who, id).await?;
    let room = room(&files).await?;
    let (path, named) = (at.path.clone(), at.path.clone());
    let (file, part) = with(&files, move |files| files.begin(&path)).await?;
    let arrived = match receive(file, body, room).await {
        Ok(()) => {
            let part = part.clone();
            with(&files, move |files| files.finish(&part, &at.path)).await
        }
        Err(problem) => Err(problem),
    };
    if arrived.is_err() {
        let dropped = with(&files, move |files| {
            files.abandon(&part);
            Ok(())
        });
        let _ = dropped.await;
    } else {
        audit::record(&state.db, &who, Some(id), "files.write", &named).await;
    }
    arrived.map(|()| StatusCode::NO_CONTENT)
}

/// A file as it is, to be saved by the browser and never shown by it.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/files/download",
    params(("id" = i64, Path, description = "The server's id."), At),
    responses(
        (status = OK, body = Vec<u8>, content_type = "application/octet-stream", description = "The file, as an attachment."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such file."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not a file."),
    )
)]
async fn download_file(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Query(at): Query<At>,
) -> Result<Response, Problem> {
    let files = files_of(&state, &who, id).await?;
    let trimmed = at.path.trim_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_owned();
    let named = at.path.clone();
    let (file, size) = with(&files, move |files| files.file(&at.path)).await?;
    audit::record(&state.db, &who, Some(id), "files.download", &named).await;
    Ok(sent(file, size, &name))
}

/// Answers with a file as it is, `size` bytes of it, for the browser to save
/// under `name`.
pub(crate) fn sent(file: std::fs::File, size: u64, name: &str) -> Response {
    // No more of it than it was said to be long: a log grows while it is sent.
    let file = tokio::fs::File::from_std(file).take(size);
    let chunks = stream::try_unfold(file, |mut file| async move {
        let mut chunk = vec![0; CHUNK];
        let read = file.read(&mut chunk).await?;
        chunk.truncate(read);
        Ok::<_, io::Error>((read > 0).then_some((chunk, file)))
    });
    let headers = [
        (
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        ),
        (CONTENT_LENGTH, HeaderValue::from(size)),
        (CONTENT_DISPOSITION, attachment(name)),
        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
    ];
    (headers, Body::from_stream(chunks)).into_response()
}

/// Makes a folder.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/files/folder",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = FolderRequest,
    responses(
        (status = NO_CONTENT, description = "The folder is there."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "Something by that name is there already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is no path for a folder."),
    )
)]
async fn make_folder(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<FolderRequest>,
) -> Result<StatusCode, Problem> {
    let files = files_of(&state, &who, id).await?;
    let named = asked.path.clone();
    with(&files, move |files| files.make_dir(&asked.path)).await?;
    audit::record(&state.db, &who, Some(id), "files.folder", &named).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves a file or a folder, which is also how it is given another name.
/// Never onto something that is there already.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/files/move",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = MoveRequest,
    responses(
        (status = NO_CONTENT, description = "It is where it was asked to be."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "A path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or nothing at `from`."),
        (status = CONFLICT, body = ProblemBody, description = "Something is at `to` already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "One of the two is no path in the server's files."),
    )
)]
async fn move_file(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<MoveRequest>,
) -> Result<StatusCode, Problem> {
    let files = files_of(&state, &who, id).await?;
    let named = format!("{} to {}", asked.from, asked.to);
    with(&files, move |files| files.rename(&asked.from, &asked.to)).await?;
    audit::record(&state.db, &who, Some(id), "files.move", &named).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes files, and folders with all that is in them. They cannot be
/// brought back. It stops at the first that cannot be deleted.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/files/remove",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = RemoveRequest,
    responses(
        (status = NO_CONTENT, description = "They are gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "A path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or one of them is not there."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "One of them is no path in the server's files."),
    )
)]
async fn remove_files(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<RemoveRequest>,
) -> Result<StatusCode, Problem> {
    let files = files_of(&state, &who, id).await?;
    let named = asked.paths.join(", ");
    with(&files, move |files| {
        asked.paths.iter().try_for_each(|path| files.remove(path))
    })
    .await?;
    audit::record(&state.db, &who, Some(id), "files.remove", &named).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Packs files and folders into one gzipped tar, in the folder they are in.
/// The answer comes when it is done, which for a world takes a while.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/files/pack",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = PackRequest,
    responses(
        (status = OK, body = Packed),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "A path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or one of the names is not there."),
        (status = CONFLICT, body = ProblemBody, description = "The disk has no room for it."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Nothing was named, or one of the names is not one."),
    )
)]
async fn pack_files(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<PackRequest>,
) -> Result<Json<Packed>, Problem> {
    if asked.names.is_empty() {
        return Err(Problem::Invalid("Choose what to pack first.".into()));
    }
    let files = files_of(&state, &who, id).await?;
    let room = room(&files).await?;
    let name = format!("archive-{}.tar.gz", stamp(auth::now()));
    let to = format!("{}/{name}", asked.folder.trim_matches('/'));
    let named = to.trim_start_matches('/').to_owned();
    with(&files, move |files| {
        files.pack(&asked.folder, &asked.names, &to, room)
    })
    .await?;
    audit::record(&state.db, &who, Some(id), "files.pack", &named).await;
    Ok(Json(Packed { name }))
}

/// Unpacks a zip, a tar or a gzipped tar into the folder it is in, over what is
/// there by the same names. The answer comes when it is done.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/files/unpack",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = UnpackRequest,
    responses(
        (status = OK, body = Unpacked),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The path leads out of the server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such file."),
        (status = CONFLICT, body = ProblemBody, description = "The disk has no room for what is in it."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not an archive Homewarp can read."),
    )
)]
async fn unpack_file(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<UnpackRequest>,
) -> Result<Json<Unpacked>, Problem> {
    let files = files_of(&state, &who, id).await?;
    let room = room(&files).await?;
    let named = asked.path.clone();
    let done = with(&files, move |files| {
        files.unpack(&asked.path, room).map_err(unreadable)
    })
    .await?;
    audit::record(&state.db, &who, Some(id), "files.unpack", &named).await;
    Ok(Json(Unpacked {
        files: done.files,
        skipped: done.skipped,
    }))
}

/// How much of the disk a server's files take, and how much of it is left.
/// Every file is asked, so a page should not wait for this before it paints.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/files/usage",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = OK, body = DiskUse),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
    )
)]
async fn disk_use(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
) -> Result<Json<DiskUse>, Problem> {
    let files = files_of(&state, &who, id).await?;
    let (used_bytes, free_bytes) = with(&files, |files| Ok((files.used(), files.free()?))).await?;
    Ok(Json(DiskUse {
        used_bytes,
        free_bytes,
    }))
}

#[cfg(test)]
mod tests {
    use super::{attachment, stamp};

    #[test]
    fn a_moment_is_named_by_its_date_and_time() {
        assert_eq!(stamp(0), "1970-01-01-000000");
        assert_eq!(stamp(951_782_399), "2000-02-28-235959");
        assert_eq!(stamp(1_709_164_800), "2024-02-29-000000");
        assert_eq!(stamp(1_791_300_645), "2026-10-06-153045");
    }

    #[test]
    fn a_download_is_saved_under_its_own_name() {
        assert_eq!(
            attachment("server.properties"),
            "attachment; filename=\"server.properties\"; filename*=UTF-8''server.properties"
        );
        // Nothing in a name gets to end the header's quotes or start a header of its own.
        assert_eq!(
            attachment("my \"world\"\r\n.zip"),
            "attachment; filename=\"my _world___.zip\"; filename*=UTF-8''my%20%22world%22%0D%0A.zip"
        );
        assert_eq!(
            attachment("monde é.txt"),
            "attachment; filename=\"monde __.txt\"; filename*=UTF-8''monde%20%C3%A9.txt"
        );
    }
}
