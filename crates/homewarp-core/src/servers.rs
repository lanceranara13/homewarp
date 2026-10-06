//! Servers: making them, asking after them, starting and stopping them
//! (PLAN.md §5.6). What a server is made of is in the database and read here;
//! what it is doing comes from its task in `runtime`.

use std::{collections::BTreeMap, sync::Arc};

use anyhow::Context;
use axum::{
    Json,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::Response,
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use homewarp_template::{Parser, rules};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::broadcast::{self, error::RecvError};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, FromHere, Problem, ProblemBody, SignedIn},
    auth,
    runtime::{self, Definition, Event, Power, Runtime},
    templates,
};

const LONGEST_NAME: usize = 60;
/// Less than this and nothing an egg installs will start.
const LEAST_MEMORY: u32 = 128;
const MOST_MEMORY: u32 = 1024 * 1024;
/// Ports below this belong to the system.
const LOWEST_PORT: u16 = 1024;
const MOST_CPU: u32 = 25_600;
const LONGEST_COMMAND: usize = 1000;

const MISSING: Problem = Problem::NotFound("There is no such server.");
const NO_DOCKER: Problem = Problem::Unavailable(
    "This Homewarp cannot reach Docker, so it has nothing to run servers with.",
);

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_servers, create_server))
        .routes(routes!(get_server, change_server, remove_server))
        .routes(routes!(power_server))
        .routes(routes!(command_server))
        // A WebSocket, which the API's description has no way to describe.
        .route("/api/v1/servers/{id}/console", get(follow_server))
}

/// A server as the Servers page shows it.
#[derive(Serialize, ToSchema)]
struct ServerSummary {
    id: i64,
    name: String,
    /// The name of the template it was made from.
    template: String,
    state: runtime::State,
    /// Where players reach it on the home machine.
    port: i64,
    memory_mb: i64,
}

/// A server in full, with the end of its console.
#[derive(Serialize, ToSchema)]
struct Server {
    id: i64,
    /// When it was made, in Unix seconds.
    created_at: i64,
    name: String,
    template_id: i64,
    /// The name of the template it was made from.
    template: String,
    image: String,
    memory_mb: i64,
    /// 100 is one core. 0 is no limit.
    cpu_percent: i64,
    /// Where players reach it on the home machine, over TCP and UDP.
    port: i64,
    state: runtime::State,
    /// The last lines of its console: the install first, then what the server prints.
    console: Vec<String>,
    /// The value of each of the template's variables, by the name the server sees.
    variables: BTreeMap<String, String>,
    /// Whether the game's EULA was agreed to for it.
    eula: bool,
}

/// What a server is made of that can still be changed once it is made.
#[derive(Deserialize, ToSchema)]
struct ServerSettings {
    name: String,
    /// One of the template's images. Its first, if none is named.
    image: Option<String>,
    memory_mb: u32,
    /// 100 is one core. Nothing, or 0, is no limit.
    cpu_percent: Option<u32>,
    /// Published on the home machine, for TCP and UDP.
    port: u16,
    /// Values for the template's variables, by the name the server sees. One
    /// that is not given takes the template's default.
    #[serde(default)]
    variables: BTreeMap<String, String>,
    /// Whoever asks agrees to the EULA of the game, for a game that has one.
    #[serde(default)]
    eula: bool,
}

#[derive(Deserialize, ToSchema)]
struct NewServer {
    template_id: i64,
    #[serde(flatten)]
    settings: ServerSettings,
}

/// Settings that have been found sound for a template.
struct Checked {
    name: String,
    image: String,
    memory_mb: u32,
    cpu_percent: u32,
    port: u16,
    variables: Vec<(String, String)>,
    eula: bool,
}

impl Checked {
    fn definition(
        self,
        id: i64,
        uuid: String,
        template: homewarp_template::Template,
        installed: bool,
    ) -> Definition {
        Definition {
            id,
            uuid,
            template,
            image: self.image,
            memory_mb: self.memory_mb,
            cpu_percent: self.cpu_percent,
            port: self.port,
            variables: self.variables,
            eula: self.eula,
            installed,
        }
    }
}

/// Holds what is asked for against the limits and against the template.
fn check(
    template: &homewarp_template::Template,
    settings: ServerSettings,
) -> Result<Checked, Problem> {
    let invalid = |sentence: String| Err(Problem::Invalid(sentence.into()));
    let name = settings.name.trim().to_owned();
    if !(1..=LONGEST_NAME).contains(&name.chars().count()) {
        return invalid("A server's name is 1 to 60 characters.".to_owned());
    }
    if !(LEAST_MEMORY..=MOST_MEMORY).contains(&settings.memory_mb) {
        return invalid("A server needs 128 MB of memory or more.".to_owned());
    }
    if settings.port < LOWEST_PORT {
        return invalid("A port is a number from 1024 to 65535.".to_owned());
    }
    let cpu_percent = settings.cpu_percent.unwrap_or(0);
    if cpu_percent > MOST_CPU {
        return invalid("A processor limit is 25600 % at the most.".to_owned());
    }
    let image = match settings.image {
        Some(asked) if template.images.iter().any(|image| image.image == asked) => asked,
        Some(_) => return invalid("This template has no such image.".to_owned()),
        None => template
            .images
            .first()
            .map(|image| image.image.clone())
            .unwrap_or_default(),
    };
    // Said now, and not by a server that installs for minutes and then will not start.
    if let Some(file) = template
        .config_files
        .iter()
        .find(|file| file.parser == Parser::Xml)
    {
        return invalid(format!(
            "This template sets up {} in a way Homewarp cannot yet, so a server made from it would not work.",
            file.path
        ));
    }

    let mut given = settings.variables;
    let mut variables = Vec::with_capacity(template.variables.len());
    for variable in &template.variables {
        let value = given
            .remove(&variable.env)
            .unwrap_or_else(|| variable.default.clone());
        if let Err(reason) = rules::check(&variable.rules, &value) {
            return invalid(format!("{} {reason}.", variable.name));
        }
        variables.push((variable.env.clone(), value));
    }
    if let Some(unknown) = given.keys().next() {
        return invalid(format!("This template has no variable called {unknown}."));
    }
    Ok(Checked {
        name,
        image,
        memory_mb: settings.memory_mb,
        cpu_percent,
        port: settings.port,
        variables,
        eula: settings.eula,
    })
}

/// What the database said to a name or a port that another server has, in words.
fn taken(error: sqlx::Error, name: &str, port: u16) -> Problem {
    match error {
        sqlx::Error::Database(error) if error.is_unique_violation() => {
            let said = if error.message().contains("servers.port") {
                format!("Port {port} belongs to another server.")
            } else {
                format!("There is already a server called {name}.")
            };
            Problem::Conflict(said.into())
        }
        other => other.into(),
    }
}

#[derive(Deserialize, ToSchema)]
struct PowerRequest {
    action: Power,
}

#[derive(Deserialize, ToSchema)]
struct CommandRequest {
    /// One line, as it would be typed into the server's console.
    command: String,
}

/// Every server as the database has it, for the tasks that run them.
pub(crate) async fn definitions(db: &SqlitePool) -> anyhow::Result<Vec<Definition>> {
    type Row = (i64, String, String, String, i64, i64, i64, String, i64, i64);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT servers.id, servers.uuid, templates.definition, servers.image, servers.memory_mb,
                servers.cpu_percent, servers.port, servers.variables, servers.eula, servers.installed
         FROM servers JOIN templates ON templates.id = servers.template_id",
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(
            |(
                id,
                uuid,
                template,
                image,
                memory_mb,
                cpu_percent,
                port,
                variables,
                eula,
                installed,
            )| {
                Ok(Definition {
                    id,
                    uuid,
                    template: serde_json::from_str(&template)
                        .context("reading a stored template")?,
                    image,
                    memory_mb: memory_mb.try_into()?,
                    cpu_percent: cpu_percent.try_into()?,
                    port: port.try_into()?,
                    variables: serde_json::from_str(&variables)
                        .context("reading a server's variables")?,
                    eula: eula != 0,
                    installed: installed != 0,
                })
            },
        )
        .collect()
}

/// Without Docker nothing runs, so every server is offline.
fn state_of(state: &AppState, id: i64) -> runtime::State {
    state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.state(id))
        .unwrap_or(runtime::State::Offline)
}

/// Every server, by name. The one request the Servers page needs.
#[utoipa::path(
    get,
    path = "/api/v1/servers",
    responses(
        (status = OK, body = Vec<ServerSummary>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn list_servers(
    State(state): State<AppState>,
    _: SignedIn,
) -> Result<Json<Vec<ServerSummary>>, Problem> {
    let rows: Vec<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT servers.id, servers.name, templates.name, servers.port, servers.memory_mb
         FROM servers JOIN templates ON templates.id = servers.template_id
         ORDER BY servers.name",
    )
    .fetch_all(&state.db)
    .await?;
    let servers = rows
        .into_iter()
        .map(|(id, name, template, port, memory_mb)| ServerSummary {
            id,
            name,
            template,
            state: state_of(&state, id),
            port,
            memory_mb,
        })
        .collect();
    Ok(Json(servers))
}

/// Makes a server from a template, then installs and starts it. The answer
/// comes at once; the install goes on, and the server's console shows it.
#[utoipa::path(
    post,
    path = "/api/v1/servers",
    request_body = NewServer,
    responses(
        (status = CREATED, body = Server),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "The name or the port belongs to another server."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Something asked for will not do."),
        (status = SERVICE_UNAVAILABLE, body = ProblemBody, description = "Homewarp cannot reach Docker."),
    )
)]
async fn create_server(
    State(state): State<AppState>,
    _: SignedIn,
    Json(new): Json<NewServer>,
) -> Result<(StatusCode, Json<Server>), Problem> {
    let found: Option<(String, String)> =
        sqlx::query_as("SELECT name, definition FROM templates WHERE id = ?")
            .bind(new.template_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((template_name, definition)) = found else {
        return Err(Problem::Invalid("There is no such template.".into()));
    };
    let template = templates::read(&definition)?;
    let checked = check(&template, new.settings)?;

    let runtime = state.runtime.as_ref().ok_or(NO_DOCKER)?;
    let uuid = auth::new_uuid();
    let created_at = auth::now();
    let id = sqlx::query(
        "INSERT INTO servers
             (uuid, name, template_id, image, memory_mb, cpu_percent, port, variables, eula, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&uuid)
    .bind(&checked.name)
    .bind(new.template_id)
    .bind(&checked.image)
    .bind(checked.memory_mb)
    .bind(checked.cpu_percent)
    .bind(checked.port)
    .bind(serde_json::to_string(&checked.variables).map_err(anyhow::Error::new)?)
    .bind(checked.eula)
    .bind(created_at)
    .execute(&state.db)
    .await
    .map_err(|error| taken(error, &checked.name, checked.port))?
    .last_insert_rowid();

    let server = Server {
        id,
        created_at,
        name: checked.name.clone(),
        template_id: new.template_id,
        template: template_name,
        image: checked.image.clone(),
        memory_mb: checked.memory_mb.into(),
        cpu_percent: checked.cpu_percent.into(),
        port: checked.port.into(),
        state: runtime::State::Installing,
        console: Vec::new(),
        variables: checked.variables.iter().cloned().collect(),
        eula: checked.eula,
    };
    runtime.add(checked.definition(id, uuid, template, false));
    Ok((StatusCode::CREATED, Json(server)))
}

/// Changes what a server is made of. It has to be stopped first, and what was
/// changed counts from its next start.
#[utoipa::path(
    put,
    path = "/api/v1/servers/{id}",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = ServerSettings,
    responses(
        (status = OK, body = Server),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The server is running, or the name or the port belongs to another."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Something asked for will not do."),
    )
)]
async fn change_server(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
    Json(settings): Json<ServerSettings>,
) -> Result<Json<Server>, Problem> {
    const RUNNING: Problem = Problem::Conflict(std::borrow::Cow::Borrowed(
        "Stop this server before changing it.",
    ));
    let found: Option<(String, String, i64)> = sqlx::query_as(
        "SELECT servers.uuid, templates.definition, servers.installed
         FROM servers JOIN templates ON templates.id = servers.template_id
         WHERE servers.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (uuid, definition, installed) = found.ok_or(MISSING)?;
    let template = templates::read(&definition)?;
    let checked = check(&template, settings)?;
    // A running server was started as it was. Changed under itself, it would
    // no longer be what is written down for it.
    if state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.state(id))
        .is_some_and(|now| !now.is_idle())
    {
        return Err(RUNNING);
    }

    sqlx::query(
        "UPDATE servers
         SET name = ?, image = ?, memory_mb = ?, cpu_percent = ?, port = ?, variables = ?, eula = ?
         WHERE id = ?",
    )
    .bind(&checked.name)
    .bind(&checked.image)
    .bind(checked.memory_mb)
    .bind(checked.cpu_percent)
    .bind(checked.port)
    .bind(serde_json::to_string(&checked.variables).map_err(anyhow::Error::new)?)
    .bind(checked.eula)
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(|error| taken(error, &checked.name, checked.port))?;
    if let Some(runtime) = &state.runtime
        && runtime
            .change(id, checked.definition(id, uuid, template, installed != 0))
            .is_err()
    {
        return Err(RUNNING);
    }
    get_server(State(state), SignedIn, Path(id)).await
}

/// One server, with its state and the end of its console. A page that stays
/// open asks again to follow both.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = OK, body = Server),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
    )
)]
async fn get_server(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
) -> Result<Json<Server>, Problem> {
    type Row = (String, i64, String, String, i64, i64, i64, i64, String, i64);
    let found: Option<Row> = sqlx::query_as(
        "SELECT servers.name, servers.template_id, templates.name, servers.image,
                servers.memory_mb, servers.cpu_percent, servers.port, servers.created_at,
                servers.variables, servers.eula
         FROM servers JOIN templates ON templates.id = servers.template_id
         WHERE servers.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (
        name,
        template_id,
        template,
        image,
        memory_mb,
        cpu_percent,
        port,
        created_at,
        variables,
        eula,
    ) = found.ok_or(MISSING)?;
    let variables: Vec<(String, String)> = serde_json::from_str(&variables)
        .context("reading a server's variables")
        .map_err(Problem::Internal)?;
    let (now, console) = state
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.seen(id))
        .unwrap_or((runtime::State::Offline, Vec::new()));
    Ok(Json(Server {
        id,
        created_at,
        name,
        template_id,
        template,
        image,
        memory_mb,
        cpu_percent,
        port,
        state: now,
        console,
        variables: variables.into_iter().collect(),
        eula: eula != 0,
    }))
}

/// Starts, stops or kills a server, or installs one again whose install failed.
/// The answer comes once the request is on its way, not once it is carried out.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/power",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = PowerRequest,
    responses(
        (status = NO_CONTENT, description = "The server has been asked."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The state the server is in rules that out."),
        (status = SERVICE_UNAVAILABLE, body = ProblemBody, description = "Homewarp cannot reach Docker."),
    )
)]
async fn power_server(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<PowerRequest>,
) -> Result<StatusCode, Problem> {
    let runtime = state.runtime.as_ref().ok_or(NO_DOCKER)?;
    match runtime.ask(id, asked.action) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(None) => Err(MISSING),
        Err(Some(now)) => Err(Problem::Conflict(
            format!(
                "This server is {}, so that cannot be done now.",
                now.in_words()
            )
            .into(),
        )),
    }
}

/// Types one line into the console of a server that is starting or running.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/command",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = CommandRequest,
    responses(
        (status = NO_CONTENT, description = "The line is on its way to the server."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The server is not running."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not one line."),
        (status = SERVICE_UNAVAILABLE, body = ProblemBody, description = "Homewarp cannot reach Docker."),
    )
)]
async fn command_server(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<CommandRequest>,
) -> Result<StatusCode, Problem> {
    let line = asked.command.trim_end_matches(['\r', '\n']);
    // A line break inside it would be a second command, slipped in behind the first.
    if line.is_empty() || line.len() > LONGEST_COMMAND || line.contains(char::is_control) {
        return Err(Problem::Invalid(
            "A command is one line of up to 1000 characters.".into(),
        ));
    }
    let runtime = state.runtime.as_ref().ok_or(NO_DOCKER)?;
    match runtime.type_in(id, line.to_owned()) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(None) => Err(MISSING),
        Err(Some(now)) => Err(Problem::Conflict(
            format!(
                "This server is {}, so there is nothing to type into.",
                now.in_words()
            )
            .into(),
        )),
    }
}

/// A server as it happens, over a WebSocket: first where things stand, then
/// each line of its console, each change of state and each measure of what it
/// uses, as an [`Event`] in JSON. The page sends nothing back.
async fn follow_server(
    _: FromHere,
    _: SignedIn,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, Problem> {
    let runtime = state.runtime.clone().ok_or(NO_DOCKER)?;
    let (snapshot, events) = runtime.follow(id).ok_or(MISSING)?;
    Ok(upgrade.on_upgrade(move |socket| follow(socket, runtime, id, snapshot, events)))
}

async fn follow(
    socket: WebSocket,
    runtime: Arc<Runtime>,
    id: i64,
    snapshot: Event,
    mut events: broadcast::Receiver<Event>,
) {
    let (mut page, mut from_page) = socket.split();
    let mut next = Some(snapshot);
    loop {
        if let Some(event) = next.take() {
            let Ok(json) = serde_json::to_string(&event) else {
                break;
            };
            if page.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => next = Some(event),
                // The page fell behind what the server prints: start it again
                // from where things stand, rather than send it half a console.
                Err(RecvError::Lagged(_)) => {
                    let Some((snapshot, fresh)) = runtime.follow(id) else { break };
                    events = fresh;
                    next = Some(snapshot);
                }
                // The server has been removed.
                Err(RecvError::Closed) => break,
            },
            // Read only so that the page's leaving is noticed.
            message = from_page.next() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Removes a server that is not running, with its files.
#[utoipa::path(
    delete,
    path = "/api/v1/servers/{id}",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = NO_CONTENT, description = "The server and its files are gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The server is running."),
    )
)]
async fn remove_server(
    State(state): State<AppState>,
    _: SignedIn,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let uuid = uuid.ok_or(MISSING)?;
    if let Some(runtime) = &state.runtime
        && !runtime.remove(id, &uuid).await?
    {
        return Err(Problem::Conflict(
            "Stop this server before removing it.".into(),
        ));
    }
    sqlx::query("DELETE FROM servers WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
