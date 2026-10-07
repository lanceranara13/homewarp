//! The owner's webhooks, as the Webhooks page asks for them and changes them:
//! each a name, an address that is told, and what it is told of. What is sent,
//! and how, is `notify`'s.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody},
    audit, fetch,
    notify::{self, Delivery, Webhook},
};

const LONGEST_NAME: usize = 60;
/// Longer than any address a site gives out to be told at.
const LONGEST_ADDRESS: usize = 500;
const MOST_WEBHOOKS: usize = 20;

const NO_WEBHOOK: Problem = Problem::NotFound("There is no such webhook.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_webhooks, create_webhook))
        .routes(routes!(change_webhook, remove_webhook))
        .routes(routes!(test_webhook))
}

/// A webhook, as the page is told of it: its address is a secret of the
/// site's making, and is not given back whole.
#[derive(Serialize, ToSchema)]
struct WebhookView {
    id: i64,
    name: String,
    /// The site, and the last few characters of the address.
    address: String,
    /// Whether it is told of every line of the Activity page.
    everything: bool,
    /// What it is told of, where that is not everything: names such as
    /// `server.crash`, in the order they are listed in.
    events: Vec<String>,
    /// How what it was last sent went, once it has been sent something.
    last: Option<Delivery>,
}

/// One thing that is written down, and so can be told.
#[derive(Serialize, ToSchema)]
struct Happening {
    /// Its name, such as `server.crash`.
    name: String,
    /// Whether it happens with nobody at the panel.
    by_itself: bool,
}

#[derive(Serialize, ToSchema)]
struct Webhooks {
    webhooks: Vec<WebhookView>,
    /// Everything a webhook can be told of.
    events: Vec<Happening>,
}

#[derive(Deserialize, ToSchema)]
struct WebhookSettings {
    name: String,
    /// An address that takes a message as JSON, as a Discord or a Slack
    /// webhook does: `https`, by a name, on the internet. Left out or empty
    /// for a webhook that is there already, it stays the address it has.
    #[serde(default)]
    url: String,
    /// Every line of the Activity page.
    #[serde(default)]
    everything: bool,
    /// What to tell it of, where that is not everything.
    #[serde(default)]
    events: Vec<String>,
}

fn view(webhook: Webhook) -> WebhookView {
    WebhookView {
        id: webhook.id,
        name: webhook.name,
        address: notify::shortened(&webhook.url),
        everything: webhook.everything,
        events: webhook.events,
        last: webhook.last,
    }
}

/// What was asked for, as it is kept.
struct Checked {
    name: String,
    /// Nothing where the address is to stay as it is.
    url: Option<String>,
    events: Vec<String>,
}

fn check(asked: &WebhookSettings, new: bool) -> Result<Checked, Problem> {
    let invalid = |sentence: &'static str| Err(Problem::Invalid(sentence.into()));
    let name = asked.name.trim();
    if name.is_empty() {
        return invalid("Give the webhook a name.");
    }
    if name.chars().count() > LONGEST_NAME {
        return invalid("A webhook's name is 60 characters at the most.");
    }
    let url = asked.url.trim();
    if url.is_empty() && new {
        return invalid("Give the address that is to be told.");
    }
    if url.len() > LONGEST_ADDRESS {
        return invalid("That address is too long to be one.");
    }
    if !url.is_empty() {
        fetch::site(url).map_err(|why| Problem::Invalid(why.into()))?;
    }
    if let Some(unknown) = asked.events.iter().find(|event| !notify::known(event)) {
        return Err(Problem::Invalid(
            format!("{unknown} is not something Homewarp writes down.").into(),
        ));
    }
    // Each once, in the order they are listed in.
    let events: Vec<String> = notify::EVENTS
        .iter()
        .filter(|(name, _)| asked.events.iter().any(|event| event == name))
        .map(|(name, _)| (*name).to_owned())
        .collect();
    if events.is_empty() && !asked.everything {
        return invalid("Choose what it is to be told of.");
    }
    Ok(Checked {
        name: name.to_owned(),
        url: (!url.is_empty()).then(|| url.to_owned()),
        events,
    })
}

/// A webhook as it is written down: without its address, which is a secret.
fn said(webhook: &Webhook) -> String {
    let of = match (webhook.everything, webhook.events.len()) {
        (true, _) => "everything".to_owned(),
        (false, 1) => "1 thing".to_owned(),
        (false, many) => format!("{many} things"),
    };
    format!(
        "{}: to {}, of {of}",
        webhook.name,
        notify::shortened(&webhook.url)
    )
}

/// The webhooks, by name, and everything one can be told of. The one request
/// the Webhooks page needs.
#[utoipa::path(
    get,
    path = "/api/v1/webhooks",
    responses(
        (status = OK, body = Webhooks),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn list_webhooks(State(state): State<AppState>, _: Owner) -> Result<Json<Webhooks>, Problem> {
    Ok(Json(Webhooks {
        webhooks: notify::all(&state.db)
            .await?
            .into_iter()
            .map(view)
            .collect(),
        events: notify::EVENTS
            .iter()
            .map(|(name, by_itself)| Happening {
                name: (*name).to_owned(),
                by_itself: *by_itself,
            })
            .collect(),
    }))
}

/// Has this Homewarp tell an address what happens to it: every line of the
/// Activity page, or the ones that are named. The address is one that takes a
/// message as JSON.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks",
    request_body = WebhookSettings,
    responses(
        (status = CREATED, body = WebhookView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "There are as many webhooks as there may be."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not an address that can be told, or nothing it could be told of."),
    )
)]
async fn create_webhook(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<WebhookSettings>,
) -> Result<(StatusCode, Json<WebhookView>), Problem> {
    let checked = check(&asked, true)?;
    if notify::all(&state.db).await?.len() >= MOST_WEBHOOKS {
        return Err(Problem::Conflict(
            "There are 20 webhooks at the most.".into(),
        ));
    }
    let url = checked.url.unwrap_or_default();
    let id = notify::add(
        &state.db,
        &checked.name,
        &url,
        asked.everything,
        &checked.events,
    )
    .await?;
    let made = notify::one(&state.db, id).await?.ok_or(NO_WEBHOOK)?;
    audit::record(&state.db, &who, None, "webhook.create", &said(&made)).await;
    Ok((StatusCode::CREATED, Json(view(made))))
}

/// Changes a webhook: its name, what it is told of and, where one is given,
/// its address. How the last thing it was sent went is forgotten with the
/// address it was sent to.
#[utoipa::path(
    put,
    path = "/api/v1/webhooks/{id}",
    params(("id" = i64, Path, description = "The webhook's id.")),
    request_body = WebhookSettings,
    responses(
        (status = OK, body = WebhookView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such webhook."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not an address that can be told, or nothing it could be told of."),
    )
)]
async fn change_webhook(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
    Json(asked): Json<WebhookSettings>,
) -> Result<Json<WebhookView>, Problem> {
    let checked = check(&asked, false)?;
    let changed = notify::change(
        &state.db,
        id,
        &checked.name,
        checked.url.as_deref(),
        asked.everything,
        &checked.events,
    )
    .await?;
    if !changed {
        return Err(NO_WEBHOOK);
    }
    let kept = notify::one(&state.db, id).await?.ok_or(NO_WEBHOOK)?;
    audit::record(&state.db, &who, None, "webhook.change", &said(&kept)).await;
    Ok(Json(view(kept)))
}

/// Has this Homewarp tell an address no more.
#[utoipa::path(
    delete,
    path = "/api/v1/webhooks/{id}",
    params(("id" = i64, Path, description = "The webhook's id.")),
    responses(
        (status = NO_CONTENT, description = "The webhook is gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such webhook."),
    )
)]
async fn remove_webhook(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let name = notify::remove(&state.db, id).await?.ok_or(NO_WEBHOOK)?;
    audit::record(&state.db, &who, None, "webhook.remove", &name).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Sends a webhook a line that says only that it is reached, and waits for
/// the site to take it.
#[utoipa::path(
    post,
    path = "/api/v1/webhooks/{id}/test",
    params(("id" = i64, Path, description = "The webhook's id.")),
    responses(
        (status = OK, body = WebhookView, description = "The site took it."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such webhook."),
        (status = CONFLICT, body = ProblemBody, description = "The site did not take it."),
    )
)]
async fn test_webhook(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
) -> Result<Json<WebhookView>, Problem> {
    let webhook = notify::one(&state.db, id).await?.ok_or(NO_WEBHOOK)?;
    notify::test(&state.db, &webhook, &who.username)
        .await
        .map_err(|why| Problem::Conflict(why.into()))?;
    let sent = notify::one(&state.db, id).await?.ok_or(NO_WEBHOOK)?;
    Ok(Json(view(sent)))
}
