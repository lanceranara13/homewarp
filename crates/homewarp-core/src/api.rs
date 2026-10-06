//! The HTTP API under `/api/v1` (PLAN.md §5.8). The OpenAPI document is built
//! from the handlers here, and the web client's types are generated from it.

use std::{
    borrow::Cow,
    path::Path,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{FromRequestParts, Request, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{HOST, ORIGIN, RETRY_AFTER, SET_COOKIE},
        request::Parts,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    accounts, audit, auth, backups, door::Client, files, limits::Limiter, runtime::Runtime,
    schedules, servers, settings, templates, totp, tunnel, tunnel::Tunnel, ui,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub(crate) db: SqlitePool,
    /// The directory everything is kept in: servers' files among it.
    pub(crate) data: Arc<Path>,
    /// What runs servers. There is none where Docker cannot be reached.
    pub(crate) runtime: Option<Arc<Runtime>>,
    /// Home's end of the tunnel to the Gate, if there is a Gate.
    pub(crate) tunnel: Arc<Tunnel>,
    setup_code: Option<Arc<str>>,
    sftp_port: Option<u16>,
    /// How often a sign-in has failed, from where and at which account.
    pub(crate) limits: Arc<Limiter>,
}

impl AppState {
    /// Looks for an account and, if there is none yet, makes the code that
    /// creating the first one will ask for.
    pub async fn start(
        db: SqlitePool,
        data: &Path,
        runtime: Option<Arc<Runtime>>,
    ) -> Result<Self, sqlx::Error> {
        let setup_code = if has_users(&db).await? {
            None
        } else {
            Some(auth::new_setup_code().into())
        };
        audit::forget_old(&db).await?;
        backups::settle(&db, data).await?;
        schedules::settle(&db).await?;
        Ok(Self {
            tunnel: Tunnel::new(db.clone(), runtime.clone()),
            db,
            data: data.into(),
            runtime,
            setup_code,
            sftp_port: None,
            limits: Arc::default(),
        })
    }

    /// Starts keeping the tunnel as the database says it should be. Left to the
    /// caller, because it changes this machine's network and tests must not.
    pub fn keep_tunnel(&self) {
        self.tunnel.keep();
    }

    /// Starts doing what servers are scheduled to do, when its time comes.
    /// Left to the caller as well: a test has no use for a clock of its own.
    pub fn keep_schedules(&self) {
        tokio::spawn(schedules::keep(self.clone()));
    }

    /// Says which port SFTP is reached on, for the pages that tell people.
    /// Nothing, where it is not served.
    pub fn sftp_at(mut self, port: Option<u16>) -> Self {
        self.sftp_port = port;
        self
    }

    /// The setup code, if this process started without an account. Whoever can
    /// read it can read this machine's logs, which is the proof setup asks for.
    pub fn setup_code(&self) -> Option<&str> {
        self.setup_code.as_deref()
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Homewarp", description = "The Homewarp panel's API."),
    // What a server's socket sends. The socket is no part of this description,
    // but the web client's types come from here all the same.
    components(schemas(crate::runtime::Event))
)]
struct Document;

fn api() -> OpenApiRouter<AppState> {
    let mut document = Document::openapi();
    // The licence is not decided yet (PLAN.md §13): say nothing rather than an empty name.
    document.info.license = None;
    OpenApiRouter::with_openapi(document)
        .routes(routes!(health))
        .routes(routes!(session))
        .routes(routes!(setup))
        .routes(routes!(login))
        .routes(routes!(logout))
        .merge(templates::routes())
        .merge(servers::routes())
        .merge(files::routes())
        .merge(tunnel::routes())
        .merge(accounts::routes())
        .merge(audit::routes())
        .merge(backups::routes())
        .merge(schedules::routes())
        .merge(settings::routes())
}

/// The whole application: the API, and the web interface for every other path.
pub fn app(state: AppState) -> Router {
    let (api, _) = api().split_for_parts();
    api.layer(middleware::from_fn(same_origin))
        .fallback(ui::serve)
        .with_state(state)
}

/// The OpenAPI document for the API.
pub fn openapi() -> utoipa::openapi::OpenApi {
    api().into_openapi()
}

/// Refuses a state-changing request that a page on another site made the browser
/// send. `SameSite=Strict` on the cookie already covers this; browsers have got
/// such things wrong before.
async fn same_origin(request: Request, next: Next) -> Response {
    if crosses_sites(&request) {
        return Problem::Forbidden("This request came from another site.").into_response();
    }
    next.run(request).await
}

fn crosses_sites(request: &Request) -> bool {
    !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) && from_elsewhere(request.headers())
}

/// A request that no other site's page made. For what `same_origin` lets pass
/// and should not: a WebSocket is opened with a GET, and a page anywhere may
/// try to open one here.
pub(crate) struct FromHere;

impl<S: Send + Sync> FromRequestParts<S> for FromHere {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Problem> {
        if from_elsewhere(&parts.headers) {
            return Err(Problem::Forbidden("This request came from another site."));
        }
        Ok(Self)
    }
}

/// Whether the browser says that the page which made this request is another
/// site's. A request that names no origin is not a page's.
fn from_elsewhere(headers: &HeaderMap) -> bool {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    header(ORIGIN).is_some_and(|origin| {
        let from = origin.split_once("://").map_or(origin, |(_, host)| host);
        Some(from) != header(HOST)
    })
}

#[derive(Serialize, ToSchema)]
struct Health {
    status: &'static str,
    version: &'static str,
}

/// Everything the web interface needs to decide what to show first.
#[derive(Serialize, ToSchema)]
struct Session {
    /// True until the first account has been created.
    setup_required: bool,
    /// Who is signed in, if anyone.
    user: Option<User>,
    version: &'static str,
    /// The port SFTP is reached on at this machine, where it is served.
    sftp_port: Option<u16>,
}

/// An account.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub(crate) struct User {
    pub(crate) id: i64,
    pub(crate) username: String,
    /// The account made at setup. It may do everything; any other account
    /// only what it has been let do, with the servers it has been let into.
    pub(crate) owner: bool,
}

#[derive(Deserialize, ToSchema)]
struct SetupRequest {
    /// The code Homewarp wrote to its log when it started.
    code: String,
    username: String,
    password: String,
}

#[derive(Deserialize, ToSchema)]
struct LoginRequest {
    username: String,
    password: String,
    /// The code of the second step, for an account that has one: from its
    /// authenticator app, or one of its recovery codes.
    #[serde(default)]
    code: String,
}

/// The body of every error response.
#[derive(Serialize, ToSchema)]
pub(crate) struct ProblemBody {
    /// A sentence fit to show to the person using the panel.
    error: String,
    /// True when what is missing is the code of the second step of a sign-in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    code_required: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Problem {
    #[error("{0}")]
    Invalid(Cow<'static, str>),
    #[error("{0}")]
    Unauthorized(&'static str),
    /// The password was right, and the second step of the sign-in is still to do.
    #[error("{0}")]
    CodeNeeded(&'static str),
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(Cow<'static, str>),
    #[error("{0}")]
    Unavailable(&'static str),
    /// Too many wrong tries: how long there is to wait.
    #[error("Too many wrong tries. Wait {} and try again.", in_minutes(.0))]
    TooMany(Duration),
    #[error("Something went wrong on the server. Its log has the details.")]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for Problem {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthorized(_) | Self::CodeNeeded(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::TooMany(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal(error) => {
                tracing::error!("{error:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let wait = match &self {
            Self::TooMany(wait) => Some(wait.as_secs().max(1)),
            _ => None,
        };
        let body = Json(ProblemBody {
            error: self.to_string(),
            code_required: matches!(self, Self::CodeNeeded(_)),
        });
        let mut response = (status, body).into_response();
        // Said to a program as it is said to a person.
        if let Some(seconds) = wait {
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

/// How long a wait is, as it is said to whoever has to wait it.
fn in_minutes(wait: &Duration) -> String {
    match wait.as_secs().div_ceil(60) {
        0 | 1 => "a minute".to_owned(),
        minutes => format!("{minutes} minutes"),
    }
}

/// How many times an address may get a sign-in wrong before it has to wait.
const TRIES_FROM_AN_ADDRESS: u32 = 5;
/// The same for one account, from wherever: more, so that someone who keeps
/// guessing at an account's password does not lock its owner out with them.
const TRIES_AT_AN_ACCOUNT: u32 = 20;

/// Who is trying to sign in, as the limits know them: the address they come
/// from and the account they name.
pub(crate) struct Trying {
    from: String,
    account: String,
}

impl Trying {
    pub(crate) fn new(client: Client, account: &str) -> Self {
        Self {
            from: format!("from {}", client.0),
            account: format!("account {}", account.trim().to_lowercase()),
        }
    }

    /// Refuses a try that comes after too many wrong ones, before anything is
    /// looked up or worked out for it.
    pub(crate) fn may(&self, limits: &Limiter) -> Result<(), Problem> {
        let waits = [
            limits.wait(&self.from, TRIES_FROM_AN_ADDRESS),
            limits.wait(&self.account, TRIES_AT_AN_ACCOUNT),
        ];
        match waits.into_iter().flatten().max() {
            Some(wait) => Err(Problem::TooMany(wait)),
            None => Ok(()),
        }
    }

    pub(crate) fn failed(&self, limits: &Limiter) {
        limits.failed(&self.from);
        limits.failed(&self.account);
    }

    /// The address has got it right, and starts afresh. What the account has
    /// been failed at by others still counts.
    pub(crate) fn passed(&self, limits: &Limiter) {
        limits.passed(&self.from);
    }
}

#[utoipa::path(get, path = "/api/v1/health", responses((status = OK, body = Health)))]
async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        version: VERSION,
    })
}

/// Who is signed in, and whether setup is still to do. The one request a page
/// needs before it can paint.
#[utoipa::path(get, path = "/api/v1/session", responses((status = OK, body = Session)))]
async fn session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Session>, Problem> {
    // Neither read depends on the other, so neither waits for the other.
    let (has_users, user) =
        tokio::try_join!(has_users(&state.db), current_user(&state.db, &headers))?;
    Ok(Json(Session {
        setup_required: !has_users,
        user,
        version: VERSION,
        sftp_port: state.sftp_port,
    }))
}

/// Creates the first account and signs it in. Works once.
#[utoipa::path(
    post,
    path = "/api/v1/setup",
    request_body = SetupRequest,
    responses(
        (status = OK, body = Session, description = "The account exists and is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The setup code is wrong."),
        (status = CONFLICT, body = ProblemBody, description = "There is an account already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The username or password will not do."),
    )
)]
async fn setup(
    State(state): State<AppState>,
    client: Client,
    Json(request): Json<SetupRequest>,
) -> Result<Response, Problem> {
    // The code is long enough that guessing it is hopeless. It is limited all
    // the same, as every door that takes a secret is.
    let trying = Trying::new(client, "the setup code");
    trying.may(&state.limits)?;
    const DONE: Problem = Problem::Conflict(Cow::Borrowed(
        "This Homewarp already has its account. Sign in instead.",
    ));
    // Asked first, so that a finished setup says so whatever code is sent.
    let (false, Some(code)) = (has_users(&state.db).await?, state.setup_code()) else {
        return Err(DONE);
    };
    if !auth::setup_code_matches(&request.code, code) {
        trying.failed(&state.limits);
        return Err(Problem::Forbidden(
            "That is not the setup code. It is in Homewarp's log.",
        ));
    }
    let username = valid_username(&request.username)?.to_owned();
    valid_password(&request.password)?;
    let hash = blocking(move || auth::hash_password(&request.password)).await??;

    // One statement, so that two people finishing setup at the same moment
    // cannot both come away with an account.
    let created = sqlx::query(
        "INSERT INTO users (username, password_hash, created_at, owner)
         SELECT ?, ?, ?, 1 WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(&username)
    .bind(hash)
    .bind(auth::now())
    .execute(&state.db)
    .await?;
    if created.rows_affected() == 0 {
        return Err(DONE);
    }
    let user = User {
        id: created.last_insert_rowid(),
        username,
        owner: true,
    };
    audit::record(&state.db, &user, None, "account.setup", "").await;
    sign_in(&state, user).await
}

#[utoipa::path(
    post,
    path = "/api/v1/login",
    request_body = LoginRequest,
    responses(
        (status = OK, body = Session),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Wrong username or password."),
    )
)]
async fn login(
    State(state): State<AppState>,
    client: Client,
    Json(request): Json<LoginRequest>,
) -> Result<Response, Problem> {
    let trying = Trying::new(client, &request.username);
    trying.may(&state.limits)?;
    let found: Option<(i64, String, String, bool)> =
        sqlx::query_as("SELECT id, username, password_hash, owner FROM users WHERE username = ?")
            .bind(request.username.trim())
            .fetch_optional(&state.db)
            .await?;
    let (user, hash) = match found {
        Some((id, username, hash, owner)) => (
            Some(User {
                id,
                username,
                owner,
            }),
            Some(hash),
        ),
        None => (None, None),
    };
    // An unknown name costs the same work as a wrong password, so how long the
    // answer takes does not say which of the two it was.
    let correct = blocking(move || {
        static NOBODY: OnceLock<String> = OnceLock::new();
        let hash = hash
            .as_ref()
            .unwrap_or_else(|| NOBODY.get_or_init(|| auth::hash_password("").unwrap_or_default()));
        auth::verify_password(&request.password, hash)
    })
    .await?;
    let from = client.0.to_string();
    let user = match user {
        Some(user) if correct => user,
        tried => {
            trying.failed(&state.limits);
            // Against an account that is there, it is written down for its
            // owner to see, with where it came from. What was typed where no
            // account is, is not: it is as likely a password in the wrong
            // field as a name.
            if let Some(user) = &tried {
                audit::record(&state.db, user, None, "account.sign_in_failed", &from).await;
            }
            return Err(Problem::Unauthorized("Wrong username or password."));
        }
    };
    // The second step, for an account that has one. A wrong code counts as a
    // wrong password does.
    match totp::second_step(&state.db, user.id, &request.code).await? {
        totp::Step::Passed => {}
        totp::Step::Wanted => {
            return Err(Problem::CodeNeeded(
                "Enter the code from your authenticator app.",
            ));
        }
        totp::Step::Wrong => {
            trying.failed(&state.limits);
            audit::record(&state.db, &user, None, "account.sign_in_failed", &from).await;
            return Err(Problem::CodeNeeded(
                "That code is not right. A code changes every half minute, and works once.",
            ));
        }
    }
    trying.passed(&state.limits);
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(auth::now())
        .execute(&state.db)
        .await?;
    audit::record(&state.db, &user, None, "account.sign_in", &from).await;
    sign_in(&state, user).await
}

#[utoipa::path(post, path = "/api/v1/logout", responses((status = NO_CONTENT, description = "Signed out.")))]
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Problem> {
    if let Some(token) = auth::token_from(&headers) {
        if let Some(user) = current_user(&state.db, &headers).await? {
            audit::record(&state.db, &user, None, "account.sign_out", "").await;
        }
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(auth::token_hash(token))
            .execute(&state.db)
            .await?;
    }
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, auth::cookie("", 0))]).into_response())
}

/// Starts a session for `user` and answers with its cookie.
async fn sign_in(state: &AppState, user: User) -> Result<Response, Problem> {
    let token = auth::new_token();
    let now = auth::now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(auth::token_hash(&token))
    .bind(user.id)
    .bind(now)
    .bind(now + auth::SESSION_SECONDS)
    .execute(&state.db)
    .await?;
    let session = Session {
        setup_required: false,
        user: Some(user),
        version: VERSION,
        sftp_port: state.sftp_port,
    };
    Ok((
        [(SET_COOKIE, auth::cookie(&token, auth::SESSION_SECONDS))],
        Json(session),
    )
        .into_response())
}

pub(crate) async fn has_users(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    let any: i64 = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(db)
        .await?;
    Ok(any != 0)
}

async fn current_user(db: &SqlitePool, headers: &HeaderMap) -> Result<Option<User>, sqlx::Error> {
    let Some(token) = auth::token_from(headers) else {
        return Ok(None);
    };
    let found: Option<(i64, String, bool)> = sqlx::query_as(
        "SELECT users.id, users.username, users.owner
         FROM sessions JOIN users ON users.id = sessions.user_id
         WHERE sessions.token_hash = ? AND sessions.expires_at > ?",
    )
    .bind(auth::token_hash(token))
    .bind(auth::now())
    .fetch_optional(db)
    .await?;
    Ok(found.map(|(id, username, owner)| User {
        id,
        username,
        owner,
    }))
}

/// A request from someone signed in, and who that is. An endpoint is private
/// by asking for this.
pub(crate) struct SignedIn(pub(crate) User);

impl FromRequestParts<AppState> for SignedIn {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        match current_user(&state.db, &parts.headers).await? {
            Some(user) => Ok(Self(user)),
            None => Err(Problem::Unauthorized("Sign in first.")),
        }
    }
}

/// A request from the owner of this Homewarp. What changes the machine, or
/// who may use it, asks for this.
pub(crate) struct Owner(pub(crate) User);

impl FromRequestParts<AppState> for Owner {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        match SignedIn::from_request_parts(parts, state).await? {
            SignedIn(user) if user.owner => Ok(Self(user)),
            _ => Err(Problem::Forbidden(
                "Only the owner of this Homewarp can do that.",
            )),
        }
    }
}

/// Runs work that would hold up the async threads off them: hashing a
/// password, and whatever reads or writes a server's files.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Problem> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Problem::Internal(error.into()))
}

pub(crate) fn valid_username(typed: &str) -> Result<&str, Problem> {
    let name = typed.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if name.is_empty() || name.len() > 32 || !name.chars().all(allowed) {
        return Err(Problem::Invalid(
            "A username is 1 to 32 letters, digits, dots, dashes or underscores.".into(),
        ));
    }
    Ok(name)
}

pub(crate) fn valid_password(password: &str) -> Result<(), Problem> {
    match password.chars().count() {
        10..=256 => Ok(()),
        _ => Err(Problem::Invalid(
            "A password is 10 to 256 characters.".into(),
        )),
    }
}
