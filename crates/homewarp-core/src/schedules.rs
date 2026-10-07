//! What a server does by the clock (PLAN.md §11, Phase 4): a line typed into
//! its console, a start, a stop, a backup, or several of those one after
//! another, at times written as cron writes them.
//!
//! The times are read on the clock of whoever set them, which their browser
//! says as so many minutes east of UTC. That is a fixed distance: where clocks
//! are put forward in summer, a schedule runs an hour off for half the year.

use std::time::Duration;

use anyhow::Context;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    accounts::{self, Permission},
    api::{AppState, Problem, ProblemBody, SignedIn, User},
    audit, auth, backups,
    clock::Cron,
    runtime::{self, Power},
    servers::MISSING,
};

const LONGEST_NAME: usize = 60;
const MOST_SCHEDULES: i64 = 20;
const MOST_TASKS: usize = 10;
/// The longest a task waits for the one before it: an hour.
const LONGEST_WAIT: u32 = 3600;
const LONGEST_COMMAND: usize = 1000;
/// How often the clock is looked at. A schedule is that much late at the most.
const LOOK_EVERY: Duration = Duration::from_secs(15);
/// How long a restart waits for the server to have stopped before it gives up.
const LONGEST_STOP: Duration = Duration::from_secs(120);

const NO_SCHEDULE: Problem = Problem::NotFound("There is no such schedule.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_schedules, create_schedule))
        .routes(routes!(change_schedule, remove_schedule))
        .routes(routes!(run_schedule))
}

/// What a task does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    /// Types `command` into the console.
    Command,
    Start,
    Stop,
    /// Stops it, waits for it to have stopped, and starts it.
    Restart,
    Kill,
    /// Begins a backup, and goes on without waiting for it.
    Backup,
}

/// One thing a schedule does. They are done in the order they are given in.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
struct Task {
    action: Action,
    /// The line to type, for a command. Nothing for anything else.
    #[serde(default)]
    command: String,
    /// How long to wait before it, in seconds: after the task before it, or
    /// after the time came for the first.
    #[serde(default)]
    wait_seconds: u32,
}

#[derive(Serialize, ToSchema)]
struct Schedule {
    id: i64,
    name: String,
    /// When it runs, as cron writes it: minute, hour, day, month, weekday.
    cron: String,
    /// The clock those are read on: minutes east of UTC.
    utc_offset: i32,
    enabled: bool,
    /// Whether a time at which the server is not running is passed over.
    only_running: bool,
    tasks: Vec<Task>,
    /// When it runs next, in Unix seconds. Never, while it is not enabled.
    next_run_at: Option<i64>,
    last_run_at: Option<i64>,
    /// What the last run came to, in a sentence.
    last_result: Option<String>,
}

#[derive(Deserialize, ToSchema)]
struct ScheduleSettings {
    name: String,
    cron: String,
    /// The clock of whoever sets it: minutes east of UTC.
    utc_offset: i32,
    enabled: bool,
    #[serde(default)]
    only_running: bool,
    tasks: Vec<Task>,
}

type Row = (
    i64,
    String,
    String,
    i32,
    bool,
    bool,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn read(row: Row) -> Result<Schedule, Problem> {
    let (
        id,
        name,
        cron,
        utc_offset,
        enabled,
        only_running,
        tasks,
        next_run_at,
        last_run_at,
        last_result,
    ) = row;
    Ok(Schedule {
        id,
        name,
        cron,
        utc_offset,
        enabled,
        only_running,
        tasks: serde_json::from_str(&tasks)
            .context("reading a schedule's tasks")
            .map_err(Problem::Internal)?,
        next_run_at,
        last_run_at,
        last_result,
    })
}

/// Settings that have been found sound, and when they first come due.
struct Checked {
    name: String,
    cron: String,
    tasks: String,
    next_run_at: Option<i64>,
}

/// What an account has to be let do with a server for a schedule of its own to
/// do this there: what the same thing asks when it is done by hand.
fn needs(action: Action) -> Permission {
    match action {
        Action::Command => Permission::Console,
        Action::Start | Action::Stop | Action::Restart | Action::Kill => Permission::Power,
        Action::Backup => Permission::Backups,
    }
}

/// Refuses tasks that do what whoever asks may not do with the server by
/// hand. A schedule runs with nobody at the panel, and what it does, it does
/// with the leave of the account that made it or set it off.
async fn within(
    db: &SqlitePool,
    who: &User,
    server_id: i64,
    tasks: &[Task],
) -> Result<(), Problem> {
    let granted = accounts::permissions_of(db, who, server_id)
        .await?
        .unwrap_or_default();
    match tasks
        .iter()
        .all(|task| granted.contains(&needs(task.action)))
    {
        true => Ok(()),
        false => Err(Problem::Forbidden(
            "Your account has not been let do, with this server, what that schedule does.",
        )),
    }
}

/// Switches off the schedules an account made that it could not make now: all
/// of them where `granted` is nothing, which is an account taken out of a
/// server (or, with no server named, removed), and otherwise those whose tasks
/// need what it is no longer let do. A schedule that is switched off says why.
pub(crate) async fn withdraw(
    db: &SqlitePool,
    server_id: Option<i64>,
    user_id: i64,
    granted: &[Permission],
) -> Result<(), sqlx::Error> {
    let theirs: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, tasks FROM schedules
         WHERE user_id = ?1 AND enabled = 1 AND (?2 IS NULL OR server_id = ?2)",
    )
    .bind(user_id)
    .bind(server_id)
    .fetch_all(db)
    .await?;
    for (id, tasks) in theirs {
        let tasks: Vec<Task> = serde_json::from_str(&tasks).unwrap_or_default();
        let may = granted.contains(&Permission::Schedules)
            && tasks
                .iter()
                .all(|task| granted.contains(&needs(task.action)));
        if !may {
            sqlx::query(
                "UPDATE schedules SET enabled = 0, next_run_at = NULL, last_result = ? WHERE id = ?",
            )
            .bind("Switched off: the account that set it may no longer do this with this server.")
            .bind(id)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

fn check(settings: &ScheduleSettings) -> Result<Checked, Problem> {
    let invalid = |sentence: String| Err(Problem::Invalid(sentence.into()));
    let name = settings.name.trim().to_owned();
    if !(1..=LONGEST_NAME).contains(&name.chars().count()) {
        return invalid("A schedule's name is 1 to 60 characters.".to_owned());
    }
    let cron = match Cron::parse(&settings.cron) {
        Ok(cron) => cron,
        Err(reason) => return invalid(format!("{reason}.")),
    };
    // No clock is further from UTC than fourteen hours.
    if !(-840..=840).contains(&settings.utc_offset) {
        return invalid("That is not a clock's distance from UTC.".to_owned());
    }
    if !(1..=MOST_TASKS).contains(&settings.tasks.len()) {
        return invalid("A schedule does 1 to 10 things.".to_owned());
    }
    for task in &settings.tasks {
        if task.wait_seconds > LONGEST_WAIT {
            return invalid("A task waits an hour at the most.".to_owned());
        }
        let line = task.command.as_str();
        // As for one typed by hand: a line break in it would be a second command.
        let sound =
            !line.is_empty() && line.len() <= LONGEST_COMMAND && !line.contains(char::is_control);
        if task.action == Action::Command && !sound {
            return invalid("A command is one line of up to 1000 characters.".to_owned());
        }
    }
    let next_run_at = match settings.enabled {
        true => match cron.next(auth::now(), settings.utc_offset) {
            Some(next) => Some(next),
            None => return invalid("That time never comes.".to_owned()),
        },
        false => None,
    };
    Ok(Checked {
        name,
        cron: settings
            .cron
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        tasks: serde_json::to_string(&settings.tasks).map_err(anyhow::Error::new)?,
        next_run_at,
    })
}

async fn one(db: &SqlitePool, server_id: i64, id: i64) -> Result<Schedule, Problem> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, name, cron, utc_offset, enabled, only_running, tasks, next_run_at,
                last_run_at, last_result
         FROM schedules WHERE id = ? AND server_id = ?",
    )
    .bind(id)
    .bind(server_id)
    .fetch_optional(db)
    .await?;
    read(row.ok_or(NO_SCHEDULE)?)
}

/// Works out afresh when each schedule comes due, from now. Done when
/// Homewarp starts: what came due while it was not running is passed over,
/// rather than all done at once on its return.
pub(crate) async fn settle(db: &SqlitePool) -> Result<(), sqlx::Error> {
    let enabled: Vec<(i64, String, i32)> =
        sqlx::query_as("SELECT id, cron, utc_offset FROM schedules WHERE enabled = 1")
            .fetch_all(db)
            .await?;
    let now = auth::now();
    for (id, cron, utc_offset) in enabled {
        let next = Cron::parse(&cron)
            .ok()
            .and_then(|cron| cron.next(now, utc_offset));
        sqlx::query("UPDATE schedules SET next_run_at = ? WHERE id = ?")
            .bind(next)
            .bind(id)
            .execute(db)
            .await?;
    }
    Ok(())
}

/// A schedule whose time has come, or that was run by hand.
struct Due {
    id: i64,
    server_id: i64,
    name: String,
    only_running: bool,
    tasks: Vec<Task>,
}

/// Looks at the clock for as long as Homewarp runs, and sets off each
/// schedule whose time has come.
pub(crate) async fn keep(state: AppState) {
    loop {
        tokio::time::sleep(LOOK_EVERY).await;
        if let Err(error) = set_off_what_is_due(&state).await {
            tracing::error!("the schedules could not be looked at: {error:#}");
        }
    }
}

async fn set_off_what_is_due(state: &AppState) -> anyhow::Result<()> {
    let now = auth::now();
    let due: Vec<(i64, i64, String, String, i32, bool, String)> = sqlx::query_as(
        "SELECT id, server_id, name, cron, utc_offset, only_running, tasks FROM schedules
         WHERE enabled = 1 AND next_run_at <= ?",
    )
    .bind(now)
    .fetch_all(&state.db)
    .await?;
    for (id, server_id, name, cron, utc_offset, only_running, tasks) in due {
        // When it comes due next is written down before it is set off, so
        // that a run which goes wrong is not a run that is tried for ever.
        let next = Cron::parse(&cron)
            .ok()
            .and_then(|cron| cron.next(now, utc_offset));
        sqlx::query("UPDATE schedules SET next_run_at = ? WHERE id = ?")
            .bind(next)
            .bind(id)
            .execute(&state.db)
            .await?;
        let tasks = serde_json::from_str(&tasks).context("reading a schedule's tasks")?;
        let due = Due {
            id,
            server_id,
            name,
            only_running,
            tasks,
        };
        tokio::spawn(run(state.clone(), due));
    }
    Ok(())
}

/// Does what a schedule says, and writes down what that came to.
async fn run(state: AppState, due: Due) {
    let result = carry_out(&state, &due).await;
    let written = sqlx::query("UPDATE schedules SET last_run_at = ?, last_result = ? WHERE id = ?")
        .bind(auth::now())
        .bind(&result)
        .bind(due.id)
        .execute(&state.db)
        .await;
    if let Err(error) = written {
        tracing::error!(
            "what schedule {} came to could not be written down: {error}",
            due.id
        );
    }
    let detail = format!("{}: {result}", due.name);
    audit::record_by_homewarp(&state.db, Some(due.server_id), "schedule.ran", &detail).await;
}

/// The sentence a schedule's run comes to.
async fn carry_out(state: &AppState, due: &Due) -> String {
    let Some(runtime) = &state.runtime else {
        return "Not run: Homewarp cannot reach Docker.".to_owned();
    };
    let id = due.server_id;
    if due.only_running && runtime.state(id) != Some(runtime::State::Running) {
        return "Passed over: the server was not running.".to_owned();
    }
    // What a server answers when its state rules a thing out.
    let refused = |now: Option<runtime::State>| match now {
        Some(now) => format!("the server was {}", now.in_words()),
        None => "the server is no more".to_owned(),
    };
    for (index, task) in due.tasks.iter().enumerate() {
        tokio::time::sleep(Duration::from_secs(task.wait_seconds.into())).await;
        let done = match task.action {
            Action::Command => runtime.type_in(id, task.command.clone()).map_err(refused),
            Action::Start => runtime.ask(id, Power::Start).map_err(refused),
            Action::Stop => runtime.ask(id, Power::Stop).map_err(refused),
            Action::Kill => runtime.ask(id, Power::Kill).map_err(refused),
            Action::Restart => restart(runtime, id).await.map_err(refused),
            Action::Backup => {
                let name = format!("{} (scheduled)", due.name);
                let begun = backups::begin(state, id, &name).await;
                begun.map(|_| ()).map_err(|problem| {
                    // The sentence a person would have been shown, without its full stop.
                    let said = problem.to_string();
                    said.trim_end_matches('.').to_owned()
                })
            }
        };
        if let Err(why) = done {
            return match due.tasks.len() {
                1 => format!("Not done: {why}."),
                _ => format!("Stopped at step {}: {why}.", index + 1),
            };
        }
    }
    "Done.".to_owned()
}

/// Stops a server that is running, waits for it to have stopped, and starts
/// it. One that is not running is only started.
async fn restart(runtime: &runtime::Runtime, id: i64) -> Result<(), Option<runtime::State>> {
    let idle = |now: Option<runtime::State>| now.is_none_or(runtime::State::is_idle);
    if !idle(runtime.state(id)) {
        runtime.ask(id, Power::Stop)?;
        let gave_up = tokio::time::Instant::now() + LONGEST_STOP;
        while !idle(runtime.state(id)) {
            if tokio::time::Instant::now() > gave_up {
                return Err(runtime.state(id));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    runtime.ask(id, Power::Start)
}

/// A server's schedules, by name. The one request the Schedules tab needs.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/schedules",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = OK, body = Vec<Schedule>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
    )
)]
async fn list_schedules(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Schedule>>, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Schedules)).await?;
    let there: Option<i64> = sqlx::query_scalar("SELECT id FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    there.ok_or(MISSING)?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, cron, utc_offset, enabled, only_running, tasks, next_run_at,
                last_run_at, last_result
         FROM schedules WHERE server_id = ? ORDER BY name COLLATE NOCASE, id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(read).collect::<Result<_, _>>()?))
}

/// Makes a schedule.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/schedules",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = ScheduleSettings,
    responses(
        (status = CREATED, body = Schedule),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The server has as many schedules as it may."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Something asked for will not do."),
    )
)]
async fn create_schedule(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(settings): Json<ScheduleSettings>,
) -> Result<(StatusCode, Json<Schedule>), Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Schedules)).await?;
    let checked = check(&settings)?;
    within(&state.db, &who, id, &settings.tasks).await?;
    let had: Option<i64> = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM schedules WHERE server_id = servers.id) FROM servers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    if had.ok_or(MISSING)? >= MOST_SCHEDULES {
        return Err(Problem::Conflict(
            "A server has 20 schedules at the most.".into(),
        ));
    }
    let made = sqlx::query(
        "INSERT INTO schedules
             (server_id, name, cron, utc_offset, enabled, only_running, tasks, next_run_at,
              created_at, user_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(&checked.name)
    .bind(&checked.cron)
    .bind(settings.utc_offset)
    .bind(settings.enabled)
    .bind(settings.only_running)
    .bind(&checked.tasks)
    .bind(checked.next_run_at)
    .bind(auth::now())
    .bind(who.id)
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    audit::record(&state.db, &who, Some(id), "schedule.create", &checked.name).await;
    Ok((StatusCode::CREATED, Json(one(&state.db, id, made).await?)))
}

/// Changes a schedule. When it runs next is worked out afresh.
#[utoipa::path(
    put,
    path = "/api/v1/servers/{id}/schedules/{schedule_id}",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("schedule_id" = i64, Path, description = "The schedule's id."),
    ),
    request_body = ScheduleSettings,
    responses(
        (status = OK, body = Schedule),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such schedule."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Something asked for will not do."),
    )
)]
async fn change_schedule(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path((id, schedule_id)): Path<(i64, i64)>,
    Json(settings): Json<ScheduleSettings>,
) -> Result<Json<Schedule>, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Schedules)).await?;
    let checked = check(&settings)?;
    within(&state.db, &who, id, &settings.tasks).await?;
    let changed = sqlx::query(
        "UPDATE schedules
         SET name = ?, cron = ?, utc_offset = ?, enabled = ?, only_running = ?, tasks = ?,
             next_run_at = ?, user_id = ?
         WHERE id = ? AND server_id = ?",
    )
    .bind(&checked.name)
    .bind(&checked.cron)
    .bind(settings.utc_offset)
    .bind(settings.enabled)
    .bind(settings.only_running)
    .bind(&checked.tasks)
    .bind(checked.next_run_at)
    // It is whoever changed it last that it now runs with the leave of.
    .bind(who.id)
    .bind(schedule_id)
    .bind(id)
    .execute(&state.db)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(NO_SCHEDULE);
    }
    audit::record(&state.db, &who, Some(id), "schedule.change", &checked.name).await;
    Ok(Json(one(&state.db, id, schedule_id).await?))
}

#[utoipa::path(
    delete,
    path = "/api/v1/servers/{id}/schedules/{schedule_id}",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("schedule_id" = i64, Path, description = "The schedule's id."),
    ),
    responses(
        (status = NO_CONTENT, description = "The schedule is gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such schedule."),
    )
)]
async fn remove_schedule(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path((id, schedule_id)): Path<(i64, i64)>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Schedules)).await?;
    let name: Option<String> =
        sqlx::query_scalar("DELETE FROM schedules WHERE id = ? AND server_id = ? RETURNING name")
            .bind(schedule_id)
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let name = name.ok_or(NO_SCHEDULE)?;
    audit::record(&state.db, &who, Some(id), "schedule.remove", &name).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Sets a schedule off now, whatever its times and whether or not it is
/// enabled. The answer comes at once; the schedule says what the run came to.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/schedules/{schedule_id}/run",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("schedule_id" = i64, Path, description = "The schedule's id."),
    ),
    responses(
        (status = ACCEPTED, description = "It has been set off."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let do this."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such schedule."),
    )
)]
async fn run_schedule(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path((id, schedule_id)): Path<(i64, i64)>,
) -> Result<StatusCode, Problem> {
    accounts::may(&state.db, &who, id, Some(Permission::Schedules)).await?;
    let schedule = one(&state.db, id, schedule_id).await?;
    within(&state.db, &who, id, &schedule.tasks).await?;
    audit::record(&state.db, &who, Some(id), "schedule.run", &schedule.name).await;
    let due = Due {
        id: schedule_id,
        server_id: id,
        name: schedule.name,
        only_running: schedule.only_running,
        tasks: schedule.tasks,
    };
    tokio::spawn(run(state, due));
    Ok(StatusCode::ACCEPTED)
}
