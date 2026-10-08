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
    http::{HeaderMap, StatusCode},
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
    accounts::{self, Permission},
    api::{AppState, FromHere, Owner, Problem, ProblemBody, SignedIn},
    audit, auth,
    minecraft::Players,
    mods,
    runtime::{self, Definition, Event, Power, Runtime},
    templates, tunnel,
};

const LONGEST_NAME: usize = 60;
/// Less than this and nothing an egg installs will start.
const LEAST_MEMORY: u32 = 128;
const MOST_MEMORY: u32 = 1024 * 1024;
/// Ports below this belong to the system.
const LOWEST_PORT: u16 = 1024;
const MOST_CPU: u32 = 25_600;
/// More than any game asks for, and few enough to refuse a mistake.
const MOST_PORTS: usize = 16;
const LONGEST_COMMAND: usize = 1000;
/// A week, in minutes.
const MOST_SLEEP: u32 = 7 * 24 * 60;

pub(crate) const MISSING: Problem = Problem::NotFound("There is no such server.");
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

/// Which protocols a port is published for at home and forwarded for by the Gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PortProtocol {
    Tcp,
    Udp,
    /// An egg does not say which its port speaks, so this is what a port starts as.
    #[default]
    Both,
}

impl PortProtocol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Both => "both",
        }
    }

    fn read(text: &str) -> Self {
        match text {
            "tcp" => Self::Tcp,
            "udp" => Self::Udp,
            _ => Self::Both,
        }
    }

    /// The one protocol or the two, as the Gate is told them.
    pub(crate) fn each(self) -> &'static [homewarp_proto::Protocol] {
        use homewarp_proto::Protocol::{Tcp, Udp};
        match self {
            Self::Tcp => &[Tcp],
            Self::Udp => &[Udp],
            Self::Both => &[Tcp, Udp],
        }
    }
}

/// A further port of a server: voice chat, say, or the port it is queried on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub(crate) struct ExtraPort {
    pub(crate) port: u16,
    #[serde(default)]
    pub(crate) protocol: PortProtocol,
}

/// A port some server has, its first or a further one.
pub(crate) struct Published {
    pub(crate) server_id: i64,
    pub(crate) server: String,
    pub(crate) port: u16,
    pub(crate) protocol: PortProtocol,
    /// The VPS the server is reached through, whose Gate forwards the port.
    pub(crate) gate_id: Option<i64>,
}

/// Every port of every server, lowest first: what is published at home, and
/// what the Gate of each server's VPS forwards.
pub(crate) async fn published(db: &SqlitePool) -> anyhow::Result<Vec<Published>> {
    let rows: Vec<(i64, String, i64, String, String, Option<i64>)> =
        sqlx::query_as("SELECT id, name, port, protocol, ports, gate_id FROM servers")
            .fetch_all(db)
            .await?;
    let mut all = Vec::with_capacity(rows.len());
    for (server_id, server, port, protocol, further, gate_id) in rows {
        let further: Vec<ExtraPort> =
            serde_json::from_str(&further).context("reading a server's ports")?;
        let first = ExtraPort {
            port: port.try_into()?,
            protocol: PortProtocol::read(&protocol),
        };
        for one in std::iter::once(first).chain(further) {
            all.push(Published {
                server_id,
                server: server.clone(),
                port: one.port,
                protocol: one.protocol,
                gate_id,
            });
        }
    }
    all.sort_by_key(|published| published.port);
    Ok(all)
}

/// The port among `ports` that a server other than `except` has, if any does.
async fn clash(db: &SqlitePool, except: Option<i64>, ports: &[u16]) -> anyhow::Result<Option<u16>> {
    Ok(published(db)
        .await?
        .into_iter()
        .find(|published| Some(published.server_id) != except && ports.contains(&published.port))
        .map(|published| published.port))
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
    /// The VPS it is reached through. None while it is reached at home only.
    gate_id: Option<i64>,
    /// Who is on it, for a running server that says.
    players: Option<Players>,
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
    /// Where players reach it, on the home machine and on a connected VPS.
    port: i64,
    /// Which protocols that port is open for.
    protocol: PortProtocol,
    /// The further ports it has.
    ports: Vec<ExtraPort>,
    /// The VPS it is reached through, whose address is the one players type.
    /// None while no VPS is connected: it is then reached at home only.
    gate_id: Option<i64>,
    state: runtime::State,
    /// The last lines of its console: the install first, then what the server prints.
    console: Vec<String>,
    /// The value of each of the template's variables, by the name the server sees.
    variables: BTreeMap<String, String>,
    /// Whether the game's EULA was agreed to for it.
    eula: bool,
    /// How many minutes it may run with nobody on it before it is put to
    /// sleep, to be woken when a player joins. 0 is never.
    sleep_minutes: i64,
    /// Whether players are told that it is offline while it is stopped.
    says_offline: bool,
    /// Who is on it, for a running server that says.
    players: Option<Players>,
    /// Whether it is one of Minecraft's, which mods or plugins can be found for.
    mods: bool,
    /// What the account that asks may do with it, beyond looking at it.
    permissions: Vec<Permission>,
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
    /// Published on the home machine, and forwarded by a connected VPS.
    port: u16,
    /// Which protocols that port is open for. Both, if none is named.
    #[serde(default)]
    protocol: PortProtocol,
    /// Further ports, each published and forwarded as the first is.
    #[serde(default)]
    ports: Vec<ExtraPort>,
    /// The VPS to reach it through, of those that are connected. With none
    /// named, a new server takes the one Homewarp recommends, and a server
    /// that is changed keeps the one it has.
    #[serde(default)]
    gate_id: Option<i64>,
    /// Values for the template's variables, by the name the server sees. One
    /// that is not given takes the template's default.
    #[serde(default)]
    variables: BTreeMap<String, String>,
    /// Whoever asks agrees to the EULA of the game, for a game that has one.
    #[serde(default)]
    eula: bool,
    /// Put to sleep after this many minutes with nobody on it, and woken when
    /// a player joins. Nothing, or 0, is never. It needs a server that says
    /// who is on it, which is one of Minecraft's.
    #[serde(default)]
    sleep_minutes: u32,
    /// While it is stopped, have something small listen on its port and tell
    /// the game's list, and a player who joins, that it is offline. For a
    /// server of Minecraft's; it is nothing to any other.
    #[serde(default)]
    says_offline: bool,
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
    protocol: PortProtocol,
    ports: Vec<ExtraPort>,
    /// The VPS asked for, if one was.
    gate_id: Option<i64>,
    variables: Vec<(String, String)>,
    eula: bool,
    sleep_minutes: u32,
    says_offline: bool,
}

impl Checked {
    /// The VPS the server is to be reached through: the one asked for, which
    /// has to be connected, or else the one it has, or else the one Homewarp
    /// would choose.
    async fn through(&self, state: &AppState, has: Option<i64>) -> Result<Option<i64>, Problem> {
        match self.gate_id {
            Some(asked) if tunnel::is_connected(state, asked).await? => Ok(Some(asked)),
            Some(_) => Err(Problem::Invalid("There is no such VPS connected.".into())),
            None => match has {
                Some(has) => Ok(Some(has)),
                None => Ok(tunnel::chosen(state).await?),
            },
        }
    }

    /// Refuses a port that is Homewarp's own: the one the panel is served on
    /// over TLS, which a VPS forwards as it forwards a server's.
    fn apart(self, state: &AppState) -> Result<Self, Problem> {
        match state.tls_port.filter(|port| self.numbers().contains(port)) {
            Some(port) => Err(Problem::Invalid(
                format!("Port {port} is the panel's own.").into(),
            )),
            None => Ok(self),
        }
    }

    /// Every port number asked for, the first one first.
    fn numbers(&self) -> Vec<u16> {
        std::iter::once(self.port)
            .chain(self.ports.iter().map(|further| further.port))
            .collect()
    }

    /// The further ports as the database keeps them.
    fn ports_json(&self) -> Result<String, Problem> {
        Ok(serde_json::to_string(&self.ports).map_err(anyhow::Error::new)?)
    }

    /// Refuses a port that another server has. The database would refuse the
    /// first port by itself; the further ones it does not know one by one.
    async fn free(&self, db: &SqlitePool, except: Option<i64>) -> Result<(), Problem> {
        // Nor the port a VPS keeps its tunnel on. Forwarded, it would send the
        // tunnel into itself, and the VPS refuses all it is asked to forward
        // rather than that.
        let tunnels: Vec<i64> = sqlx::query_scalar("SELECT wg_port FROM gates")
            .fetch_all(db)
            .await?;
        let numbers = self.numbers();
        if let Some(port) = numbers
            .iter()
            .find(|port| tunnels.contains(&i64::from(**port)))
        {
            return Err(Problem::Invalid(
                format!("Port {port} is the one a VPS keeps its tunnel to this Homewarp on.")
                    .into(),
            ));
        }
        match clash(db, except, &numbers).await? {
            Some(port) => Err(Problem::Conflict(
                format!("Port {port} belongs to another server.").into(),
            )),
            None => Ok(()),
        }
    }

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
            name: self.name,
            template,
            image: self.image,
            memory_mb: self.memory_mb,
            cpu_percent: self.cpu_percent,
            port: self.port,
            protocol: self.protocol,
            ports: self.ports,
            variables: self.variables,
            eula: self.eula,
            installed,
            sleep_minutes: self.sleep_minutes,
            says_offline: self.says_offline,
            // What is made or changed is not running, and not asleep either.
            asleep: false,
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
    if settings.ports.len() > MOST_PORTS {
        return invalid("A server has 16 further ports at the most.".to_owned());
    }
    let mut numbers: Vec<u16> = std::iter::once(settings.port)
        .chain(settings.ports.iter().map(|further| further.port))
        .collect();
    if numbers.iter().any(|port| *port < LOWEST_PORT) {
        return invalid("A port is a number from 1024 to 65535.".to_owned());
    }
    numbers.sort_unstable();
    if let Some(twice) = numbers.windows(2).find(|pair| pair[0] == pair[1]) {
        return invalid(format!("Port {} is given twice.", twice[0]));
    }
    let cpu_percent = settings.cpu_percent.unwrap_or(0);
    if cpu_percent > MOST_CPU {
        return invalid("A processor limit is 25600 % at the most.".to_owned());
    }
    if settings.sleep_minutes > MOST_SLEEP {
        return invalid(
            "A server is put to sleep after a week with nobody on it at the latest: 10080 minutes."
                .to_owned(),
        );
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
        protocol: settings.protocol,
        ports: settings.ports,
        gate_id: settings.gate_id,
        variables,
        eula: settings.eula,
        sleep_minutes: settings.sleep_minutes,
        says_offline: settings.says_offline,
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
    type Row = (
        i64,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        String,
        i64,
        i64,
        String,
        i64,
        i64,
        i64,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT servers.id, servers.uuid, templates.definition, servers.image, servers.memory_mb,
                servers.cpu_percent, servers.port, servers.protocol, servers.ports,
                servers.variables, servers.eula, servers.installed, servers.name,
                servers.sleep_minutes, servers.asleep, servers.says_offline
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
                protocol,
                ports,
                variables,
                eula,
                installed,
                name,
                sleep_minutes,
                asleep,
                says_offline,
            )| {
                Ok(Definition {
                    id,
                    uuid,
                    name,
                    sleep_minutes: sleep_minutes.try_into()?,
                    asleep: asleep != 0,
                    says_offline: says_offline != 0,
                    template: serde_json::from_str(&template)
                        .context("reading a stored template")?,
                    image,
                    memory_mb: memory_mb.try_into()?,
                    cpu_percent: cpu_percent.try_into()?,
                    port: port.try_into()?,
                    protocol: PortProtocol::read(&protocol),
                    ports: serde_json::from_str(&ports).context("reading a server's ports")?,
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

fn players_of(state: &AppState, id: i64) -> Option<Players> {
    state.runtime.as_ref()?.players(id)
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
    SignedIn(who): SignedIn,
) -> Result<Json<Vec<ServerSummary>>, Problem> {
    // All of them for the owner, and for anyone else those they have been let into.
    let rows: Vec<(i64, String, String, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT servers.id, servers.name, templates.name, servers.port, servers.memory_mb,
                servers.gate_id
         FROM servers JOIN templates ON templates.id = servers.template_id
         WHERE ?1 OR servers.id IN (SELECT server_id FROM server_users WHERE user_id = ?2)
         ORDER BY servers.name",
    )
    .bind(who.owner)
    .bind(who.id)
    .fetch_all(&state.db)
    .await?;
    let servers = rows
        .into_iter()
        .map(
            |(id, name, template, port, memory_mb, gate_id)| ServerSummary {
                id,
                name,
                template,
                state: state_of(&state, id),
                port,
                memory_mb,
                gate_id,
                players: players_of(&state, id),
            },
        )
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
    Owner(who): Owner,
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
    let checked = check(&template, new.settings)?.apart(&state)?;

    let runtime = state.runtime.as_ref().ok_or(NO_DOCKER)?;
    checked.free(&state.db, None).await?;
    let gate_id = checked.through(&state, None).await?;
    let uuid = auth::new_uuid();
    let created_at = auth::now();
    let id = sqlx::query(
        "INSERT INTO servers
             (uuid, name, template_id, image, memory_mb, cpu_percent, port, protocol, ports,
              variables, eula, sleep_minutes, says_offline, gate_id, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&uuid)
    .bind(&checked.name)
    .bind(new.template_id)
    .bind(&checked.image)
    .bind(checked.memory_mb)
    .bind(checked.cpu_percent)
    .bind(checked.port)
    .bind(checked.protocol.as_str())
    .bind(checked.ports_json()?)
    .bind(serde_json::to_string(&checked.variables).map_err(anyhow::Error::new)?)
    .bind(checked.eula)
    .bind(checked.sleep_minutes)
    .bind(checked.says_offline)
    .bind(gate_id)
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
        protocol: checked.protocol,
        ports: checked.ports.clone(),
        gate_id,
        state: runtime::State::Installing,
        console: Vec::new(),
        variables: checked.variables.iter().cloned().collect(),
        eula: checked.eula,
        sleep_minutes: checked.sleep_minutes.into(),
        says_offline: checked.says_offline,
        players: None,
        mods: mods::fits(&template),
        permissions: Permission::ALL.to_vec(),
    };
    runtime.add(checked.definition(id, uuid, template, false));
    // A connected Gate is told of the new ports now, and not in ten seconds.
    state.tunnel.wake();
    audit::record(&state.db, &who, Some(id), "server.create", &server.template).await;
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
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(settings): Json<ServerSettings>,
) -> Result<Json<Server>, Problem> {
    const RUNNING: Problem = Problem::Conflict(std::borrow::Cow::Borrowed(
        "Stop this server before changing it.",
    ));
    accounts::may(&state.db, &who, id, Some(Permission::Settings)).await?;
    type Row = (
        String,
        String,
        i64,
        Option<i64>,
        i64,
        String,
        String,
        i64,
        i64,
        String,
    );
    let found: Option<Row> = sqlx::query_as(
        "SELECT servers.uuid, templates.definition, servers.installed, servers.gate_id,
                servers.port, servers.protocol, servers.ports, servers.memory_mb,
                servers.cpu_percent, servers.variables
         FROM servers JOIN templates ON templates.id = servers.template_id
         WHERE servers.id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (uuid, definition, installed, has, port, protocol, ports, memory_mb, cpu_percent, set) =
        found.ok_or(MISSING)?;
    let template = templates::read(&definition)?;
    let checked = check(&template, settings)?.apart(&state)?;
    // Where it is reached is the owner's to say. A port is opened on this
    // machine and forwarded by a VPS, where it takes the place of whatever
    // else that machine has on it: an account that may change the rest of a
    // server leaves its ports, and the VPS it is reached through, as they are.
    let as_it_is = i64::from(checked.port) == port
        && checked.protocol.as_str() == protocol
        && serde_json::from_str::<Vec<ExtraPort>>(&ports)
            .ok()
            .as_deref()
            == Some(checked.ports.as_slice())
        && checked.gate_id.is_none_or(|asked| Some(asked) == has);
    if !who.owner && !as_it_is {
        return Err(Problem::Forbidden(
            "Only the owner of this Homewarp changes a server's ports, or the VPS it is reached through.",
        ));
    }
    // How much of this machine a server may use is the owner's to say as
    // well: it is taken from every other server, and from the machine.
    let within =
        i64::from(checked.memory_mb) == memory_mb && i64::from(checked.cpu_percent) == cpu_percent;
    if !who.owner && !within {
        return Err(Problem::Forbidden(
            "Only the owner of this Homewarp changes how much memory or processor a server may use.",
        ));
    }
    // And so is what its template does not leave to a user: an egg says of
    // each of its variables whether a user may change it, and writes the
    // rules of the others for whoever runs the panel.
    let set: Vec<(String, String)> =
        serde_json::from_str(&set).context("reading a server's variables")?;
    let left_alone = template
        .variables
        .iter()
        .filter(|variable| !variable.user_editable)
        .all(|variable| {
            let of = |values: &[(String, String)]| {
                let found = values.iter().find(|(env, _)| *env == variable.env);
                found.map_or(variable.default.clone(), |(_, value)| value.clone())
            };
            of(&set) == of(&checked.variables)
        });
    if !who.owner && !left_alone {
        return Err(Problem::Forbidden(
            "Only the owner of this Homewarp changes what a template does not leave to its users to set.",
        ));
    }
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
    checked.free(&state.db, Some(id)).await?;
    let gate_id = checked.through(&state, has).await?;

    sqlx::query(
        "UPDATE servers
         SET name = ?, image = ?, memory_mb = ?, cpu_percent = ?, port = ?, protocol = ?,
             ports = ?, variables = ?, eula = ?, sleep_minutes = ?, says_offline = ?, gate_id = ?
         WHERE id = ?",
    )
    .bind(&checked.name)
    .bind(&checked.image)
    .bind(checked.memory_mb)
    .bind(checked.cpu_percent)
    .bind(checked.port)
    .bind(checked.protocol.as_str())
    .bind(checked.ports_json()?)
    .bind(serde_json::to_string(&checked.variables).map_err(anyhow::Error::new)?)
    .bind(checked.eula)
    .bind(checked.sleep_minutes)
    .bind(checked.says_offline)
    .bind(gate_id)
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
    state.tunnel.wake();
    audit::record(&state.db, &who, Some(id), "server.change", "").await;
    get_server(State(state), SignedIn(who), Path(id)).await
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
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
) -> Result<Json<Server>, Problem> {
    let permissions = accounts::permissions_of(&state.db, &who, id)
        .await?
        .ok_or(MISSING)?;
    type Row = (
        String,
        i64,
        String,
        String,
        i64,
        i64,
        i64,
        String,
        String,
        i64,
        String,
        i64,
        i64,
        String,
        i64,
        Option<i64>,
    );
    let found: Option<Row> = sqlx::query_as(
        "SELECT servers.name, servers.template_id, templates.name, servers.image,
                servers.memory_mb, servers.cpu_percent, servers.port, servers.protocol,
                servers.ports, servers.created_at, servers.variables, servers.eula,
                servers.sleep_minutes, templates.definition, servers.says_offline,
                servers.gate_id
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
        protocol,
        ports,
        created_at,
        variables,
        eula,
        sleep_minutes,
        definition,
        says_offline,
        gate_id,
    ) = found.ok_or(MISSING)?;
    let variables: Vec<(String, String)> = serde_json::from_str(&variables)
        .context("reading a server's variables")
        .map_err(Problem::Internal)?;
    let ports: Vec<ExtraPort> = serde_json::from_str(&ports)
        .context("reading a server's ports")
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
        protocol: PortProtocol::read(&protocol),
        ports,
        gate_id,
        state: now,
        console,
        variables: variables.into_iter().collect(),
        eula: eula != 0,
        sleep_minutes,
        says_offline: says_offline != 0,
        players: players_of(&state, id),
        // A template that cannot be read is no reason not to show its server.
        mods: templates::read(&definition).is_ok_and(|template| mods::fits(&template)),
        permissions,
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
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<PowerRequest>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Power)).await?;
    let runtime = state.runtime.as_ref().ok_or(NO_DOCKER)?;
    match runtime.ask(id, asked.action) {
        Ok(()) => {
            let action = match asked.action {
                Power::Start => "server.start",
                Power::Stop => "server.stop",
                Power::Kill => "server.kill",
                Power::Install => "server.install",
            };
            audit::record(&state.db, &who, Some(id), action, "").await;
            Ok(StatusCode::NO_CONTENT)
        }
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
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(asked): Json<CommandRequest>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Console)).await?;
    let line = asked.command.trim_end_matches(['\r', '\n']);
    // A line break inside it would be a second command, slipped in behind the first.
    if line.is_empty() || line.len() > LONGEST_COMMAND || line.contains(char::is_control) {
        return Err(Problem::Invalid(
            "A command is one line of up to 1000 characters.".into(),
        ));
    }
    let runtime = state.runtime.as_ref().ok_or(NO_DOCKER)?;
    match runtime.type_in(id, line.to_owned()) {
        Ok(()) => {
            audit::record(&state.db, &who, Some(id), "server.command", line).await;
            Ok(StatusCode::NO_CONTENT)
        }
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
    SignedIn(who): SignedIn,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, Problem> {
    accounts::may(&state.db, &who, id, None).await?;
    let runtime = state.runtime.clone().ok_or(NO_DOCKER)?;
    let (snapshot, events) = runtime.follow(id).ok_or(MISSING)?;
    let following = Following {
        db: state.db.clone(),
        session: crate::auth::token_from(&headers).map(crate::auth::token_hash),
        taken: state.taken.subscribe(),
    };
    Ok(upgrade.on_upgrade(move |socket| follow(socket, runtime, id, snapshot, events, following)))
}

/// What a socket asks again by, whether whoever opened it may still look:
/// their session, and where to hear that something was taken from an account.
struct Following {
    db: SqlitePool,
    session: Option<Vec<u8>>,
    taken: broadcast::Receiver<()>,
}

impl Following {
    /// Whether the session that opened the socket is still one, of an account
    /// that may still look at the server.
    async fn may_still(&self, server_id: i64) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        match crate::api::signed_in_by(&self.db, session).await {
            Ok(Some(who)) => accounts::may(&self.db, &who, server_id, None).await.is_ok(),
            _ => false,
        }
    }
}

async fn follow(
    socket: WebSocket,
    runtime: Arc<Runtime>,
    id: i64,
    snapshot: Event,
    mut events: broadcast::Receiver<Event>,
    mut following: Following,
) {
    let (mut page, mut from_page) = socket.split();
    let mut next = Some(snapshot);
    let mut listening = true;
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
            // Something was taken from some account. If it was from the one
            // this socket was opened by, the socket is closed.
            told = following.taken.recv(), if listening => match told {
                Err(RecvError::Closed) => listening = false,
                _ => {
                    if !following.may_still(id).await {
                        break;
                    }
                }
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
    Owner(who): Owner,
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
    // Written down while the server still has a name to write.
    audit::record(&state.db, &who, Some(id), "server.remove", "").await;
    sqlx::query("DELETE FROM servers WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    state.tunnel.wake();
    Ok(StatusCode::NO_CONTENT)
}
