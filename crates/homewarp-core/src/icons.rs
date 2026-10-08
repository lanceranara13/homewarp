//! A server's icon: a small picture given to it, to tell it from the others
//! at a glance. It is kept in the database, as a PNG and nothing else. The
//! page makes one of whatever picture is chosen, so that what Homewarp keeps
//! is small, and what it serves back is only ever what it held to be a PNG.

use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Path, Query, State},
    http::{
        HeaderValue,
        header::{CACHE_CONTROL, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS},
    },
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    accounts::{self, Permission},
    api::{AppState, Problem, ProblemBody, SignedIn},
    audit,
    servers::MISSING,
};

/// The page sends 128 pixels a side, which is some tens of kilobytes at the
/// most. This leaves room for a picture made by something else.
const LARGEST: usize = 256 << 10;
const WIDEST: u32 = 512;
/// What every PNG begins with.
const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// And what comes next in every one: its first part, thirteen bytes long,
/// which says how wide and how high it is.
const HEADER: &[u8] = b"\0\0\0\x0dIHDR";

const NONE: Problem = Problem::NotFound("This server has no icon.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_icon, set_icon, remove_icon))
}

/// Where a server's icon is fetched from. The address says which picture it
/// is, so one that was fetched before need not be asked for again.
pub(crate) fn address(server_id: i64, tag: &str) -> String {
    format!("/api/v1/servers/{server_id}/icon?v={tag}")
}

/// Where the icon of one server is fetched from, if it has been given one.
pub(crate) async fn of(db: &SqlitePool, server_id: i64) -> Result<Option<String>, sqlx::Error> {
    let tag: Option<String> =
        sqlx::query_scalar("SELECT tag FROM server_icons WHERE server_id = ?")
            .bind(server_id)
            .fetch_optional(db)
            .await?;
    Ok(tag.map(|tag| address(server_id, &tag)))
}

/// How wide and how high a PNG says it is. Nothing, for what is not one.
fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    let said = bytes
        .strip_prefix(SIGNATURE)?
        .strip_prefix(HEADER)?
        .get(..8)?;
    let number =
        |at: usize| u32::from_be_bytes([said[at], said[at + 1], said[at + 2], said[at + 3]]);
    Some((number(0), number(4)))
}

/// Which picture an address is for.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Which {
    /// What the address a server is listed with ends in. Left out, the icon
    /// is sent as it is now.
    v: Option<String>,
}

/// Where a server's icon is fetched from now.
#[derive(Serialize, ToSchema)]
struct ServerIcon {
    /// Nothing, for a server that has none: it is shown by the first letter
    /// of its name.
    icon: Option<String>,
}

/// A server's icon, as a PNG.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/icon",
    params(("id" = i64, Path, description = "The server's id."), Which),
    responses(
        (status = OK, body = Vec<u8>, content_type = "image/png", description = "The icon."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or it has no icon."),
    )
)]
async fn get_icon(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Query(which): Query<Which>,
) -> Result<Response, Problem> {
    accounts::may(&state.db, &who, id, None).await?;
    let found: Option<(Vec<u8>, String)> =
        sqlx::query_as("SELECT png, tag FROM server_icons WHERE server_id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (png, tag) = found.ok_or(NONE)?;
    // An address that names this picture is this picture for good, and a
    // browser may keep it so. One that names another, or none, is asked for
    // again each time: the icon may have changed since. Either way it is for
    // whoever is signed in, and for nothing that sits between.
    let kept = match which.v.as_deref() == Some(tag.as_str()) {
        true => "private, max-age=31536000, immutable",
        false => "private, no-cache",
    };
    let headers = [
        (CONTENT_TYPE, HeaderValue::from_static("image/png")),
        (CACHE_CONTROL, HeaderValue::from_static(kept)),
        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
    ];
    Ok((headers, png).into_response())
}

/// Gives a server an icon, in place of the one it has. The body is a PNG, of
/// 256 KB and 512 pixels a side at the most. Unlike the rest of what a server
/// is made of, it can be changed while the server runs.
#[utoipa::path(
    put,
    path = "/api/v1/servers/{id}/icon",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body(content = Vec<u8>, content_type = "image/png", description = "The picture's bytes."),
    responses(
        (status = OK, body = ServerIcon),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not a PNG, or it is too large."),
    )
)]
async fn set_icon(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    body: Body,
) -> Result<Json<ServerIcon>, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Settings)).await?;
    let refused = |sentence: &'static str| Problem::Invalid(sentence.into());
    let png = to_bytes(body, LARGEST)
        .await
        .map_err(|_| refused("An icon is 256 KB at the most."))?;
    let (width, height) = size(&png).ok_or_else(|| refused("An icon is a PNG picture."))?;
    if !(1..=WIDEST).contains(&width) || !(1..=WIDEST).contains(&height) {
        return Err(refused("An icon is 1 to 512 pixels a side."));
    }
    // Enough of what the picture comes to, to tell it from the one before.
    let tag: String = Sha256::digest(&png)[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let kept = sqlx::query(
        "INSERT INTO server_icons (server_id, png, tag) VALUES (?, ?, ?)
         ON CONFLICT (server_id) DO UPDATE SET png = excluded.png, tag = excluded.tag",
    )
    .bind(id)
    .bind(&png[..])
    .bind(&tag)
    .execute(&state.db)
    .await;
    match kept {
        Ok(_) => {}
        // The owner may do anything with any server, one that is not there included.
        Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
            return Err(MISSING);
        }
        Err(error) => return Err(error.into()),
    }
    audit::record(&state.db, &who, Some(id), "server.icon", "").await;
    Ok(Json(ServerIcon {
        icon: Some(address(id, &tag)),
    }))
}

/// Takes a server's icon away. It is shown by the first letter of its name again.
#[utoipa::path(
    delete,
    path = "/api/v1/servers/{id}/icon",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = OK, body = ServerIcon),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
    )
)]
async fn remove_icon(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
) -> Result<Json<ServerIcon>, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Settings)).await?;
    let there: Option<i64> = sqlx::query_scalar("SELECT 1 FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    there.ok_or(MISSING)?;
    let removed = sqlx::query("DELETE FROM server_icons WHERE server_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() > 0 {
        audit::record(&state.db, &who, Some(id), "server.icon", "removed").await;
    }
    Ok(Json(ServerIcon { icon: None }))
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn a_png_says_how_large_it_is_and_nothing_else_is_taken_for_one() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend([0, 0, 0, 128, 0, 0, 1, 0, 8, 6, 0, 0, 0]);
        assert_eq!(size(&png), Some((128, 256)));
        // Cut off before it has said both.
        assert_eq!(size(&png[..20]), None);
        assert_eq!(size(b""), None);
        assert_eq!(size(b"GIF89a\x80\0\x80\0"), None);
        assert_eq!(size(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"), None);
        // A PNG's beginning, and then something that is not its first part.
        assert_eq!(
            size(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIDAT\0\0\0\x01\0\0\0\x01"),
            None
        );
    }
}
