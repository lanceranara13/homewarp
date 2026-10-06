//! What is set for this Homewarp as a whole (PLAN.md §5.6, §11 Phases 4 and
//! 5): where servers look names up, and how many new connections a second a
//! connected VPS lets one address open to them.

use std::net::Ipv4Addr;

use anyhow::Context;
use axum::{Json, extract::State};
use homewarp_proto::NEW_PER_SECOND;
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
/// The fewest and the most new connections a second that can be set.
const NEW_CONNECTIONS: std::ops::RangeInclusive<u32> = 1..=10_000;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_settings, change_settings))
}

#[derive(Serialize, ToSchema)]
struct Settings {
    /// Where servers look names up: one to three IPv4 addresses on the
    /// internet. A change counts from each server's next start.
    resolvers: Vec<String>,
    /// How many new connections a second a connected VPS lets one address on
    /// the internet open to the servers, with twice as many at once. More
    /// are dropped at the VPS, so that one address cannot crowd the rest out.
    new_connections: u32,
}

/// What is to be changed. What is not named is left as it is.
#[derive(Deserialize, ToSchema)]
struct SettingsChange {
    #[serde(default)]
    resolvers: Option<Vec<String>>,
    #[serde(default)]
    new_connections: Option<u32>,
}

/// How many new connections a second a Gate is to let one address open: what
/// has been set, or what a Gate allows by itself.
pub(crate) async fn new_connections(db: &SqlitePool) -> anyhow::Result<u32> {
    let set: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'new_connections'")
            .fetch_optional(db)
            .await?;
    match set {
        Some(json) => serde_json::from_str(&json).context("reading the limit that was set"),
        None => Ok(NEW_PER_SECOND),
    }
}

async fn keep(db: &SqlitePool, key: &str, value: String) -> Result<(), Problem> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(db)
    .await?;
    Ok(())
}

async fn all(state: &AppState) -> Result<Settings, Problem> {
    // Neither read depends on the other, so neither waits for the other.
    let (resolvers, new_connections) =
        tokio::try_join!(resolvers(&state.db), new_connections(&state.db))?;
    Ok(Settings {
        resolvers,
        new_connections,
    })
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
    Ok(Json(all(&state).await?))
}

/// Changes what is set, and leaves what is not named as it is. Where servers
/// look names up counts from each server's next start: one that is running
/// goes on asking where it asked. The limit on new connections counts as soon
/// as the Gate is told, which is at once.
#[utoipa::path(
    put,
    path = "/api/v1/settings",
    request_body = SettingsChange,
    responses(
        (status = OK, body = Settings),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "A resolver is not an address servers can reach, or the limit is not one that can be set."),
    )
)]
async fn change_settings(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<SettingsChange>,
) -> Result<Json<Settings>, Problem> {
    // All of it is held against what can be set before any of it is kept.
    let resolvers = asked.resolvers.as_deref().map(sound).transpose()?;
    if let Some(limit) = asked.new_connections
        && !NEW_CONNECTIONS.contains(&limit)
    {
        return Err(Problem::Invalid(
            "A limit on new connections is 1 to 10000 a second.".into(),
        ));
    }
    let mut changed = Vec::new();
    if let Some(resolvers) = resolvers {
        let json = serde_json::to_string(&resolvers).map_err(anyhow::Error::new)?;
        keep(&state.db, "resolvers", json).await?;
        if let Some(runtime) = &state.runtime {
            runtime.set_resolvers(resolvers.clone());
        }
        changed.push(format!("resolvers: {}", resolvers.join(", ")));
    }
    if let Some(limit) = asked.new_connections {
        keep(&state.db, "new_connections", limit.to_string()).await?;
        // The Gate is told now, and not at the next round.
        state.tunnel.wake();
        changed.push(format!("new connections: {limit} a second"));
    }
    if !changed.is_empty() {
        audit::record(
            &state.db,
            &who,
            None,
            "settings.change",
            &changed.join("; "),
        )
        .await;
    }
    Ok(Json(all(&state).await?))
}
