//! What was done through the panel, by whom and to which server (PLAN.md §6,
//! §11 Phase 4). A handler writes a line here once what it was asked for has
//! been done; the owner reads the lines on the Activity page.

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody, User},
    auth,
};

/// How long a line is kept: half a year.
const KEPT_SECONDS: i64 = 180 * 24 * 60 * 60;
/// The longest detail kept. A command typed into a console is the long one.
const LONGEST_DETAIL: usize = 300;
/// How many lines one request answers with.
const PAGE: i64 = 100;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list_activity))
}

/// Writes down that `who` did `action`, to `server` if it was to one. The
/// server's name is kept as it is now, for when the server is no more.
///
/// It is called once the thing has been done, and what has been done is not
/// answered with an error because its record could not be kept: that is said
/// in Homewarp's own log, and nowhere else.
pub(crate) async fn record(
    db: &SqlitePool,
    who: &User,
    server: Option<i64>,
    action: &'static str,
    detail: &str,
) {
    write(db, Some(who.id), &who.username, server, action, detail).await;
}

/// Writes down what nobody asked for just now: what a schedule did when its
/// time came. It stands under Homewarp's own name, and under no account.
pub(crate) async fn record_by_homewarp(
    db: &SqlitePool,
    server: Option<i64>,
    action: &'static str,
    detail: &str,
) {
    write(db, None, "Homewarp", server, action, detail).await;
}

async fn write(
    db: &SqlitePool,
    user_id: Option<i64>,
    username: &str,
    server: Option<i64>,
    action: &'static str,
    detail: &str,
) {
    // One line: what a detail is made of is typed by people, and by servers.
    let detail: String = detail
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(LONGEST_DETAIL)
        .collect();
    let written = sqlx::query(
        "INSERT INTO audit_log (at, user_id, username, server_id, server, action, detail)
         VALUES (?, ?, ?, ?, (SELECT name FROM servers WHERE id = ?), ?, ?)",
    )
    .bind(auth::now())
    .bind(user_id)
    .bind(username)
    .bind(server)
    .bind(server)
    .bind(action)
    .bind(detail)
    .execute(db)
    .await;
    if let Err(error) = written {
        tracing::error!("{action} by {username} could not be written down: {error}");
    }
}

/// Drops the lines that are older than what is kept. Done when Homewarp starts.
pub(crate) async fn forget_old(db: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM audit_log WHERE at < ?")
        .bind(auth::now() - KEPT_SECONDS)
        .execute(db)
        .await?;
    Ok(())
}

/// Which lines are asked for.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Asked {
    /// Only what was done to the server with this id.
    server: Option<i64>,
    /// Only what the account with this id did.
    user: Option<i64>,
    /// Only what is older than the line with this id: for the page after the first.
    before: Option<i64>,
}

/// One thing that was done.
#[derive(Serialize, ToSchema)]
struct ActivityEntry {
    id: i64,
    /// When, in Unix seconds.
    at: i64,
    /// Whose account did it. None once that account has been removed.
    user_id: Option<i64>,
    /// That account's name at the time.
    username: String,
    /// The server it was done to, if it was done to one.
    server_id: Option<i64>,
    /// That server's name at the time.
    server: Option<String>,
    /// What was done, as a name such as `server.start` or `files.remove`.
    action: String,
    /// What it was done with: a path, a name, a command.
    detail: String,
}

/// What was done through the panel, the newest first, a hundred lines at a
/// time. The one request the Activity page needs.
#[utoipa::path(
    get,
    path = "/api/v1/activity",
    params(Asked),
    responses(
        (status = OK, body = Vec<ActivityEntry>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn list_activity(
    State(state): State<AppState>,
    _: Owner,
    Query(asked): Query<Asked>,
) -> Result<Json<Vec<ActivityEntry>>, Problem> {
    type Row = (
        i64,
        i64,
        Option<i64>,
        String,
        Option<i64>,
        Option<String>,
        String,
        String,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, at, user_id, username, server_id, server, action, detail FROM audit_log
         WHERE (?1 IS NULL OR server_id = ?1) AND (?2 IS NULL OR user_id = ?2)
           AND (?3 IS NULL OR id < ?3)
         ORDER BY id DESC LIMIT ?4",
    )
    .bind(asked.server)
    .bind(asked.user)
    .bind(asked.before)
    .bind(PAGE)
    .fetch_all(&state.db)
    .await?;
    let entries = rows
        .into_iter()
        .map(
            |(id, at, user_id, username, server_id, server, action, detail)| ActivityEntry {
                id,
                at,
                user_id,
                username,
                server_id,
                server,
                action,
                detail,
            },
        )
        .collect();
    Ok(Json(entries))
}
