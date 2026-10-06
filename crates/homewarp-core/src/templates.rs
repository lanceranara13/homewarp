//! Templates: eggs, read once and kept as the documents the importer makes of
//! them (PLAN.md §5.6).

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Problem, ProblemBody, SignedIn},
    auth,
};

/// An egg is a few kilobytes. This leaves room for a very long install script.
const LARGEST_EGG: usize = 1 << 20;
const LONGEST_NAME: usize = 100;

const MISSING: Problem = Problem::NotFound("There is no such template.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_templates, import_template))
        .routes(routes!(get_template, remove_template))
}

/// A template as the gallery shows it.
#[derive(Serialize, ToSchema)]
struct TemplateSummary {
    id: i64,
    name: String,
    description: String,
    /// The image a new server runs in unless another is picked.
    image: String,
}

/// A template in full: everything Homewarp will do on its word.
#[derive(Serialize, ToSchema)]
struct Template {
    id: i64,
    /// When it was imported, in Unix seconds.
    created_at: i64,
    name: String,
    description: String,
    /// The first is the one a new server runs in unless another is picked.
    images: Vec<TemplateImage>,
    /// The command that starts a server, with `{{NAME}}` where a variable goes.
    startup: String,
    /// A server has started once its console prints any of these.
    done: Vec<String>,
    stop: TemplateStop,
    /// The files patched in a server's directory before every start.
    config_files: Vec<String>,
    /// What runs once, before a server's first start. Not every template has it.
    install: Option<TemplateInstall>,
    variables: Vec<TemplateVariable>,
    features: Vec<String>,
}

#[derive(Serialize, ToSchema)]
struct TemplateImage {
    label: String,
    image: String,
}

/// How a server is asked to shut down.
#[derive(Serialize, ToSchema)]
struct TemplateStop {
    by: StopBy,
    /// The command typed into the console, or the name of the signal sent.
    value: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum StopBy {
    Command,
    Signal,
}

#[derive(Serialize, ToSchema)]
struct TemplateInstall {
    image: String,
    entrypoint: String,
    script: String,
}

#[derive(Serialize, ToSchema)]
struct TemplateVariable {
    name: String,
    description: String,
    /// The environment variable the server sees.
    env: String,
    default: String,
    /// Laravel-style validation rules, one per entry.
    rules: Vec<String>,
    user_viewable: bool,
    user_editable: bool,
}

impl Template {
    fn new(id: i64, created_at: i64, template: homewarp_template::Template) -> Self {
        let (by, value) = match template.stop {
            homewarp_template::Stop::Command(command) => (StopBy::Command, command),
            homewarp_template::Stop::Signal(signal) => (StopBy::Signal, signal),
        };
        Self {
            id,
            created_at,
            name: template.name,
            description: template.description,
            images: template
                .images
                .into_iter()
                .map(|image| TemplateImage {
                    label: image.label,
                    image: image.image,
                })
                .collect(),
            startup: template.startup,
            done: template.done,
            stop: TemplateStop { by, value },
            config_files: template
                .config_files
                .into_iter()
                .map(|file| file.path)
                .collect(),
            install: template.install.map(|install| TemplateInstall {
                image: install.image,
                entrypoint: install.entrypoint,
                script: install.script,
            }),
            variables: template
                .variables
                .into_iter()
                .map(|variable| TemplateVariable {
                    name: variable.name,
                    description: variable.description,
                    env: variable.env,
                    default: variable.default,
                    rules: variable.rules,
                    user_viewable: variable.user_viewable,
                    user_editable: variable.user_editable,
                })
                .collect(),
            features: template.features,
        }
    }
}

#[derive(Deserialize, ToSchema)]
struct ImportRequest {
    /// The egg as a panel exports it: JSON or YAML.
    egg: String,
}

/// A stored document, read back. One that will not read was written by another
/// version of Homewarp: a fault here, and nothing the request did.
fn read(definition: &str) -> Result<homewarp_template::Template, Problem> {
    serde_json::from_str(definition).map_err(|error| {
        Problem::Internal(anyhow::Error::new(error).context("reading a stored template"))
    })
}

/// Every template, by name. The one request the Templates page needs.
#[utoipa::path(
    get,
    path = "/api/v1/templates",
    responses(
        (status = OK, body = Vec<TemplateSummary>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn list_templates(
    State(state): State<AppState>,
    _: SignedIn,
) -> Result<Json<Vec<TemplateSummary>>, Problem> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, definition FROM templates ORDER BY name")
            .fetch_all(&state.db)
            .await?;
    let mut templates = Vec::with_capacity(rows.len());
    for (id, definition) in rows {
        let template = read(&definition)?;
        templates.push(TemplateSummary {
            id,
            name: template.name,
            description: template.description,
            image: template
                .images
                .into_iter()
                .next()
                .map(|image| image.image)
                .unwrap_or_default(),
        });
    }
    Ok(Json(templates))
}

/// Reads an egg and keeps it as a template.
#[utoipa::path(
    post,
    path = "/api/v1/templates",
    request_body = ImportRequest,
    responses(
        (status = CREATED, body = Template),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "There is a template of that name already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "This is not an egg Homewarp can read."),
    )
)]
async fn import_template(
    State(state): State<AppState>,
    _: SignedIn,
    Json(request): Json<ImportRequest>,
) -> Result<(StatusCode, Json<Template>), Problem> {
    if request.egg.len() > LARGEST_EGG {
        return Err(Problem::Invalid("An egg is at most 1 MB.".into()));
    }
    let mut template = homewarp_template::import(&request.egg).map_err(|error| {
        Problem::Invalid(format!("Homewarp cannot read this egg: {error}.").into())
    })?;
    template.name = template.name.trim().to_owned();
    if !(1..=LONGEST_NAME).contains(&template.name.chars().count()) {
        return Err(Problem::Invalid(
            "A template's name is 1 to 100 characters.".into(),
        ));
    }

    let definition = serde_json::to_string(&template).map_err(anyhow::Error::new)?;
    let created_at = auth::now();
    let inserted = sqlx::query(
        "INSERT INTO templates (name, definition, source, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&template.name)
    .bind(definition)
    .bind(&request.egg)
    .bind(created_at)
    .execute(&state.db)
    .await;
    let id = match inserted {
        Ok(inserted) => inserted.last_insert_rowid(),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(Problem::Conflict(
                format!(
                    "There is already a template called {}. Remove that one to import this in its place.",
                    template.name
                )
                .into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    Ok((
        StatusCode::CREATED,
        Json(Template::new(id, created_at, template)),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/templates/{id}",
    params(("id" = i64, Path, description = "The template's id.")),
    responses(
        (status = OK, body = Template),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such template."),
    )
)]
async fn get_template(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
) -> Result<Json<Template>, Problem> {
    let found: Option<(String, i64)> =
        sqlx::query_as("SELECT definition, created_at FROM templates WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (definition, created_at) = found.ok_or(MISSING)?;
    Ok(Json(Template::new(id, created_at, read(&definition)?)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/templates/{id}",
    params(("id" = i64, Path, description = "The template's id.")),
    responses(
        (status = NO_CONTENT, description = "The template is gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such template."),
    )
)]
async fn remove_template(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let removed = sqlx::query("DELETE FROM templates WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(MISSING);
    }
    Ok(StatusCode::NO_CONTENT)
}
