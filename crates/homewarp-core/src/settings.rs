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
    store::{self, Store},
};

/// As many as a resolver's own list holds.
const MOST_RESOLVERS: usize = 3;
/// The fewest and the most new connections a second that can be set.
const NEW_CONNECTIONS: std::ops::RangeInclusive<u32> = 1..=10_000;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_settings, change_settings))
        .routes(routes!(set_store, remove_store))
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
    /// Where backups are copied to beside where they are kept. Nothing if nowhere.
    store: Option<StoreView>,
}

/// The store elsewhere, as the page is told of it: without its key's secret,
/// which is kept and not given back.
#[derive(Serialize, ToSchema)]
struct StoreView {
    endpoint: String,
    region: String,
    bucket: String,
    /// The folder in the bucket. Empty for none.
    prefix: String,
    key_id: String,
}

/// A store for backups: a bucket that is spoken to as Amazon's S3 is.
#[derive(Deserialize, ToSchema)]
struct StoreChange {
    /// Where it is: `https://s3.example.com`, or `http://192.168.1.20:9000`
    /// for one at home.
    endpoint: String,
    /// Its region. `us-east-1` if none is given, which most stores take.
    #[serde(default)]
    region: String,
    bucket: String,
    /// A folder in the bucket to keep to. Left out, none.
    #[serde(default)]
    prefix: String,
    /// The key that may write to the bucket, and its secret.
    key_id: String,
    secret: String,
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
        store: store::kept(&state.db).await.map(|store| StoreView {
            endpoint: store.endpoint,
            region: store.region,
            bucket: store.bucket,
            prefix: store.prefix,
            key_id: store.key_id,
        }),
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

/// Has every backup that is made from now on copied to a store elsewhere: a
/// bucket that is spoken to as Amazon's S3 is. The store is tried first, with
/// a few bytes written and taken away again, and is kept only if it took
/// them: a store that is thought to hold copies and holds none is worse than
/// none. The key's secret is kept and not given back.
#[utoipa::path(
    put,
    path = "/api/v1/settings/store",
    request_body = StoreChange,
    responses(
        (status = OK, body = Settings),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "The store could not be reached, or would not be written to."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Something given is not what a store's settings look like."),
    )
)]
async fn set_store(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<StoreChange>,
) -> Result<Json<Settings>, Problem> {
    let store = Store {
        endpoint: asked.endpoint,
        region: asked.region,
        bucket: asked.bucket,
        key_id: asked.key_id,
        secret: asked.secret,
        prefix: asked.prefix,
    }
    .checked()
    .map_err(|why| Problem::Invalid(why.into()))?;
    store
        .tried()
        .await
        .map_err(|why| Problem::Conflict(why.into()))?;
    store::keep(&state.db, &store).await?;
    let detail = format!("store: the bucket {} at {}", store.bucket, store.endpoint);
    audit::record(&state.db, &who, None, "settings.change", &detail).await;
    Ok(Json(all(&state).await?))
}

/// Has backups copied nowhere from now on. What is in the store stays there.
#[utoipa::path(
    delete,
    path = "/api/v1/settings/store",
    responses(
        (status = OK, body = Settings),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn remove_store(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<Json<Settings>, Problem> {
    store::forget(&state.db).await?;
    audit::record(&state.db, &who, None, "settings.change", "store: none").await;
    Ok(Json(all(&state).await?))
}
