//! What is set for this Homewarp as a whole (PLAN.md §5.6, §11 Phase 4). For
//! now that is one thing: where servers look names up.

use std::net::Ipv4Addr;

use anyhow::Context;
use axum::{Json, extract::State};
use homewarp_runtime::RESOLVERS;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody},
    audit,
};

/// As many as a resolver's own list holds.
const MOST_RESOLVERS: usize = 3;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_settings, change_settings))
}

#[derive(Serialize, Deserialize, ToSchema)]
struct Settings {
    /// Where servers look names up: one to three IPv4 addresses on the
    /// internet. A change counts from each server's next start.
    resolvers: Vec<String>,
}

/// Where servers look names up: what has been set, or the two that Homewarp
/// would give them by itself.
pub(crate) async fn resolvers(db: &SqlitePool) -> anyhow::Result<Vec<String>> {
    let set: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'resolvers'")
            .fetch_optional(db)
            .await?;
    match set {
        Some(json) => serde_json::from_str(&json).context("reading the resolvers that were set"),
        None => Ok(RESOLVERS.map(str::to_owned).to_vec()),
    }
}

/// The addresses as they are kept: each one an address a server can reach, once.
fn sound(asked: &[String]) -> Result<Vec<String>, Problem> {
    let invalid = |sentence: String| Err(Problem::Invalid(sentence.into()));
    let mut resolvers: Vec<String> = Vec::new();
    for typed in asked {
        let Ok(address) = typed.trim().parse::<Ipv4Addr>() else {
            return invalid(format!("{} is not an IPv4 address.", typed.trim()));
        };
        // A server is kept from every address at home (PLAN.md §5.6), the
        // router's among them: a resolver there would answer it nothing.
        if address.is_private()
            || address.is_loopback()
            || address.is_link_local()
            || address.is_unspecified()
            || address.is_broadcast()
            || address.is_multicast()
        {
            return invalid(format!(
                "Servers are kept from the home network, so they could not ask {address}. Give a resolver on the internet, such as 1.1.1.1 or 9.9.9.9."
            ));
        }
        let address = address.to_string();
        if !resolvers.contains(&address) {
            resolvers.push(address);
        }
    }
    if !(1..=MOST_RESOLVERS).contains(&resolvers.len()) {
        return invalid("Give one to three resolvers.".to_owned());
    }
    Ok(resolvers)
}

/// What is set. The one request the owner's part of the Settings page needs
/// beside the accounts.
#[utoipa::path(
    get,
    path = "/api/v1/settings",
    responses(
        (status = OK, body = Settings),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn get_settings(State(state): State<AppState>, _: Owner) -> Result<Json<Settings>, Problem> {
    Ok(Json(Settings {
        resolvers: resolvers(&state.db).await?,
    }))
}

/// Changes what is set. Where servers look names up counts from each
/// server's next start: one that is running goes on asking where it asked.
#[utoipa::path(
    put,
    path = "/api/v1/settings",
    request_body = Settings,
    responses(
        (status = OK, body = Settings),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "A resolver is not an address servers can reach."),
    )
)]
async fn change_settings(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<Settings>,
) -> Result<Json<Settings>, Problem> {
    let resolvers = sound(&asked.resolvers)?;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('resolvers', ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(serde_json::to_string(&resolvers).map_err(anyhow::Error::new)?)
    .execute(&state.db)
    .await?;
    if let Some(runtime) = &state.runtime {
        runtime.set_resolvers(resolvers.clone());
    }
    let detail = format!("resolvers: {}", resolvers.join(", "));
    audit::record(&state.db, &who, None, "settings.change", &detail).await;
    Ok(Json(Settings { resolvers }))
}
