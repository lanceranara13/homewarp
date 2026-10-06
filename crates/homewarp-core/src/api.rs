//! The HTTP API under `/api/v1` (PLAN.md §5.8). The OpenAPI document is built
//! from the handlers here, and the web client's types are generated from it.

use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

use axum::{
    Json, Router,
    extract::{FromRequestParts, Request, State},
    http::{
        HeaderMap, Method, StatusCode,
        header::{HOST, ORIGIN, SET_COOKIE},
        request::Parts,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{auth, runtime::Runtime, servers, templates, tunnel, tunnel::Tunnel, ui};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub(crate) db: SqlitePool,
    /// What runs servers. There is none where Docker cannot be reached.
    pub(crate) runtime: Option<Arc<Runtime>>,
    /// Home's end of the tunnel to the Gate, if there is a Gate.
    pub(crate) tunnel: Arc<Tunnel>,
    setup_code: Option<Arc<str>>,
}

impl AppState {
    /// Looks for an account and, if there is none yet, makes the code that
    /// creating the first one will ask for.
    pub async fn start(db: SqlitePool, runtime: Option<Arc<Runtime>>) -> Result<Self, sqlx::Error> {
        let setup_code = if has_users(&db).await? {
            None
        } else {
            Some(auth::new_setup_code().into())
        };
        Ok(Self {
            tunnel: Tunnel::new(db.clone(), runtime.clone()),
            db,
            runtime,
            setup_code,
        })
    }

    /// Starts keeping the tunnel as the database says it should be. Left to the
    /// caller, because it changes this machine's network and tests must not.
    pub fn keep_tunnel(&self) {
        self.tunnel.keep();
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
        .merge(tunnel::routes())
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
}

#[derive(Serialize, ToSchema)]
struct User {
    id: i64,
    username: String,
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
}

/// The body of every error response.
#[derive(Serialize, ToSchema)]
pub(crate) struct ProblemBody {
    /// A sentence fit to show to the person using the panel.
    error: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Problem {
    #[error("{0}")]
    Invalid(Cow<'static, str>),
    #[error("{0}")]
    Unauthorized(&'static str),
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(Cow<'static, str>),
    #[error("{0}")]
    Unavailable(&'static str),
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
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(error) => {
                tracing::error!("{error:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (
            status,
            Json(ProblemBody {
                error: self.to_string(),
            }),
        )
            .into_response()
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
    Json(request): Json<SetupRequest>,
) -> Result<Response, Problem> {
    const DONE: Problem = Problem::Conflict(Cow::Borrowed(
        "This Homewarp already has its account. Sign in instead.",
    ));
    // Asked first, so that a finished setup says so whatever code is sent.
    let (false, Some(code)) = (has_users(&state.db).await?, state.setup_code()) else {
        return Err(DONE);
    };
    if !auth::setup_code_matches(&request.code, code) {
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
        "INSERT INTO users (username, password_hash, created_at)
         SELECT ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM users)",
    )
    .bind(&username)
    .bind(hash)
    .bind(auth::now())
    .execute(&state.db)
    .await?;
    if created.rows_affected() == 0 {
        return Err(DONE);
    }
    sign_in(
        &state.db,
        User {
            id: created.last_insert_rowid(),
            username,
        },
    )
    .await
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
    Json(request): Json<LoginRequest>,
) -> Result<Response, Problem> {
    let found: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, username, password_hash FROM users WHERE username = ?")
            .bind(request.username.trim())
            .fetch_optional(&state.db)
            .await?;
    let (user, hash) = match found {
        Some((id, username, hash)) => (Some(User { id, username }), Some(hash)),
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
    let Some(user) = user.filter(|_| correct) else {
        return Err(Problem::Unauthorized("Wrong username or password."));
    };
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(auth::now())
        .execute(&state.db)
        .await?;
    sign_in(&state.db, user).await
}

#[utoipa::path(post, path = "/api/v1/logout", responses((status = NO_CONTENT, description = "Signed out.")))]
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Problem> {
    if let Some(token) = auth::token_from(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(auth::token_hash(token))
            .execute(&state.db)
            .await?;
    }
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, auth::cookie("", 0))]).into_response())
}

/// Starts a session for `user` and answers with its cookie.
async fn sign_in(db: &SqlitePool, user: User) -> Result<Response, Problem> {
    let token = auth::new_token();
    let now = auth::now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(auth::token_hash(&token))
    .bind(user.id)
    .bind(now)
    .bind(now + auth::SESSION_SECONDS)
    .execute(db)
    .await?;
    let session = Session {
        setup_required: false,
        user: Some(user),
        version: VERSION,
    };
    Ok((
        [(SET_COOKIE, auth::cookie(&token, auth::SESSION_SECONDS))],
        Json(session),
    )
        .into_response())
}

async fn has_users(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    let any: i64 = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(db)
        .await?;
    Ok(any != 0)
}

async fn current_user(db: &SqlitePool, headers: &HeaderMap) -> Result<Option<User>, sqlx::Error> {
    let Some(token) = auth::token_from(headers) else {
        return Ok(None);
    };
    let found: Option<(i64, String)> = sqlx::query_as(
        "SELECT users.id, users.username FROM sessions JOIN users ON users.id = sessions.user_id
         WHERE sessions.token_hash = ? AND sessions.expires_at > ?",
    )
    .bind(auth::token_hash(token))
    .bind(auth::now())
    .fetch_optional(db)
    .await?;
    Ok(found.map(|(id, username)| User { id, username }))
}

/// A request from someone signed in. An endpoint is private by asking for this.
pub(crate) struct SignedIn;

impl FromRequestParts<AppState> for SignedIn {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        match current_user(&state.db, &parts.headers).await? {
            Some(_) => Ok(Self),
            None => Err(Problem::Unauthorized("Sign in first.")),
        }
    }
}

/// Runs slow, CPU-bound work (password hashing) off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Problem> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Problem::Internal(error.into()))
}

fn valid_username(typed: &str) -> Result<&str, Problem> {
    let name = typed.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    if name.is_empty() || name.len() > 32 || !name.chars().all(allowed) {
        return Err(Problem::Invalid(
            "A username is 1 to 32 letters, digits, dots, dashes or underscores.".into(),
        ));
    }
    Ok(name)
}

fn valid_password(password: &str) -> Result<(), Problem> {
    match password.chars().count() {
        10..=256 => Ok(()),
        _ => Err(Problem::Invalid(
            "A password is 10 to 256 characters.".into(),
        )),
    }
}
