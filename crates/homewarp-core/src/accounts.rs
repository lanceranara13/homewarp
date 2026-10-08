//! Accounts besides the owner's, and what each may do with which server
//! (PLAN.md §11, Phase 4).
//!
//! The account made at setup owns this Homewarp. It makes the others, and lets
//! each into the servers it should have. An account that has been let into a
//! server may look at it: its state, its console, what it uses. What it may do
//! there beyond looking is said one [`Permission`] at a time.

use anyhow::Context;
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody, SignedIn, User, valid_password, valid_username},
    audit, auth, schedules,
    servers::MISSING,
    totp,
};

const NO_ACCOUNT: Problem = Problem::NotFound("There is no such account.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_accounts, create_account))
        .routes(routes!(remove_account))
        .routes(routes!(set_password))
        .routes(routes!(change_own_password))
        .routes(routes!(two_steps, begin_two_steps))
        .routes(routes!(confirm_two_steps))
        .routes(routes!(end_two_steps))
        .routes(routes!(end_two_steps_of))
        .routes(routes!(list_server_users))
        .routes(routes!(let_in, turn_out))
}

/// Something an account may be let do with a server, beyond looking at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Permission {
    /// Type commands into its console.
    Console,
    /// Start it, stop it and kill it.
    Power,
    /// Read, change, upload and delete its files.
    Files,
    /// Make backups of it, and put one back. Taking one away from here is
    /// taking every file of the server, and asks for `Files` as well.
    Backups,
    /// Set what it does by the clock.
    Schedules,
    /// Change what it is made of: its name, its image, when it sleeps, and what
    /// its template leaves to a user to set. Its memory and processor, its
    /// ports and the rest of what its template asks are the owner's.
    Settings,
}

impl Permission {
    pub(crate) const ALL: [Self; 6] = [
        Self::Console,
        Self::Power,
        Self::Files,
        Self::Backups,
        Self::Schedules,
        Self::Settings,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Console => "console",
            Self::Power => "power",
            Self::Files => "files",
            Self::Backups => "backups",
            Self::Schedules => "schedules",
            Self::Settings => "settings",
        }
    }
}

fn read(permissions: &str) -> Result<Vec<Permission>, Problem> {
    serde_json::from_str(permissions)
        .context("reading what an account may do")
        .map_err(Problem::Internal)
}

/// What an account may do with a server beyond looking at it, or nothing at
/// all if it has not been let into it. The owner may do everything.
pub(crate) async fn permissions_of(
    db: &SqlitePool,
    who: &User,
    server_id: i64,
) -> Result<Option<Vec<Permission>>, Problem> {
    if who.owner {
        return Ok(Some(Permission::ALL.to_vec()));
    }
    let granted: Option<String> = sqlx::query_scalar(
        "SELECT permissions FROM server_users WHERE server_id = ? AND user_id = ?",
    )
    .bind(server_id)
    .bind(who.id)
    .fetch_optional(db)
    .await?;
    granted.as_deref().map(read).transpose()
}

/// Lets a request through if the account may do this with the server: what is
/// `needed`, or with nothing named, look at it. To an account that has not
/// been let into a server, that server is not there.
pub(crate) async fn may(
    db: &SqlitePool,
    who: &User,
    server_id: i64,
    needed: Option<Permission>,
) -> Result<(), Problem> {
    match (permissions_of(db, who, server_id).await?, needed) {
        (None, _) => Err(MISSING),
        (Some(granted), Some(needed)) if !granted.contains(&needed) => Err(Problem::Forbidden(
            "Your account has not been let do that with this server.",
        )),
        _ => Ok(()),
    }
}

/// An account, as the owner sees it among the others.
#[derive(Serialize, ToSchema)]
struct Account {
    id: i64,
    username: String,
    owner: bool,
    /// When it was made, in Unix seconds.
    created_at: i64,
    /// How many servers it has been let into. The owner is in all of them, and this says nothing of it.
    servers: i64,
}

#[derive(Deserialize, ToSchema)]
struct NewAccount {
    username: String,
    /// What it signs in with at first. Whoever it is for can change it.
    password: String,
}

#[derive(Deserialize, ToSchema)]
struct NewPassword {
    password: String,
}

#[derive(Deserialize, ToSchema)]
struct OwnPassword {
    /// The password as it is now.
    current: String,
    /// The one to have from now on.
    password: String,
}

/// An account that has been let into a server.
#[derive(Serialize, ToSchema)]
struct ServerUser {
    user_id: i64,
    username: String,
    /// What it may do there beyond looking.
    permissions: Vec<Permission>,
}

#[derive(Deserialize, ToSchema)]
struct Grant {
    /// What the account may do with the server beyond looking at it. None is looking only.
    permissions: Vec<Permission>,
}

/// Every account, the owner's first. The one request the Users page needs.
#[utoipa::path(
    get,
    path = "/api/v1/users",
    responses(
        (status = OK, body = Vec<Account>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn list_accounts(
    State(state): State<AppState>,
    _: Owner,
) -> Result<Json<Vec<Account>>, Problem> {
    let rows: Vec<(i64, String, bool, i64, i64)> = sqlx::query_as(
        "SELECT id, username, owner, created_at,
                (SELECT COUNT(*) FROM server_users WHERE user_id = users.id)
         FROM users ORDER BY owner DESC, username",
    )
    .fetch_all(&state.db)
    .await?;
    let accounts = rows
        .into_iter()
        .map(|(id, username, owner, created_at, servers)| Account {
            id,
            username,
            owner,
            created_at,
            servers,
        })
        .collect();
    Ok(Json(accounts))
}

/// Makes an account. It can sign in at once, and sees no server until it is let into one.
#[utoipa::path(
    post,
    path = "/api/v1/users",
    request_body = NewAccount,
    responses(
        (status = CREATED, body = Account),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "There is an account by that name already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The username or password will not do."),
    )
)]
async fn create_account(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(new): Json<NewAccount>,
) -> Result<(StatusCode, Json<Account>), Problem> {
    let username = valid_username(&new.username)?.to_owned();
    valid_password(&new.password)?;
    let hash = auth::hash(new.password).await?;
    let created_at = auth::now();
    let inserted =
        sqlx::query("INSERT INTO users (username, password_hash, created_at) VALUES (?, ?, ?)")
            .bind(&username)
            .bind(hash)
            .bind(created_at)
            .execute(&state.db)
            .await;
    let id = match inserted {
        Ok(inserted) => inserted.last_insert_rowid(),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(Problem::Conflict(
                format!("There is already an account called {username}.").into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    audit::record(&state.db, &who, None, "account.create", &username).await;
    Ok((
        StatusCode::CREATED,
        Json(Account {
            id,
            username,
            owner: false,
            created_at,
            servers: 0,
        }),
    ))
}

/// Removes an account, signs it out everywhere and takes it out of every server.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}",
    params(("id" = i64, Path, description = "The account's id.")),
    responses(
        (status = NO_CONTENT, description = "The account is gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account asking is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such account."),
        (status = CONFLICT, body = ProblemBody, description = "It is the owner's account."),
    )
)]
async fn remove_account(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let found: Option<(String, bool)> =
        sqlx::query_as("SELECT username, owner FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (username, owner) = found.ok_or(NO_ACCOUNT)?;
    if owner {
        return Err(Problem::Conflict(
            "The owner's account cannot be removed.".into(),
        ));
    }
    // What it set servers to do by the clock is not done in its name once it is gone.
    schedules::withdraw(&state.db, None, id, &[]).await?;
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    state.taken_away();
    audit::record(&state.db, &who, None, "account.remove", &username).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Gives an account another password, for whoever has forgotten theirs. The
/// account is signed out wherever it was signed in.
#[utoipa::path(
    put,
    path = "/api/v1/users/{id}/password",
    params(("id" = i64, Path, description = "The account's id.")),
    request_body = NewPassword,
    responses(
        (status = NO_CONTENT, description = "The account has the new password."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account asking is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such account."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The password will not do."),
    )
)]
async fn set_password(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
    Json(new): Json<NewPassword>,
) -> Result<StatusCode, Problem> {
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let username = username.ok_or(NO_ACCOUNT)?;
    valid_password(&new.password)?;
    let hash = auth::hash(new.password).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id)
        .execute(&state.db)
        .await?;
    // The browsers it was known by knew the password that is no more.
    sqlx::query("DELETE FROM known_devices WHERE user_id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;
    // The owner's own sessions stay: this one among them.
    if id != who.id {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    state.taken_away();
    audit::record(&state.db, &who, None, "account.password", &username).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Changes the password of whoever asks. Every other session of the account
/// ends; the one that asked goes on.
#[utoipa::path(
    post,
    path = "/api/v1/account/password",
    request_body = OwnPassword,
    responses(
        (status = NO_CONTENT, description = "The account has the new password."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The current password is wrong."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The new password will not do."),
    )
)]
async fn change_own_password(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    headers: HeaderMap,
    Json(asked): Json<OwnPassword>,
) -> Result<StatusCode, Problem> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(who.id)
        .fetch_one(&state.db)
        .await?;
    if !auth::verify(asked.current, Some(hash)).await? {
        return Err(Problem::Forbidden(
            "That is not your password as it is now.",
        ));
    }
    valid_password(&asked.password)?;
    let hash = auth::hash(asked.password).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(who.id)
        .execute(&state.db)
        .await?;
    // The browsers it was known by knew the password that is no more: one
    // that signs in with the new one is known again from then.
    sqlx::query("DELETE FROM known_devices WHERE user_id = ?")
        .bind(who.id)
        .execute(&state.db)
        .await?;
    let this = auth::token_from(&headers).map(auth::token_hash);
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash IS NOT ?")
        .bind(who.id)
        .bind(this.map(|hash| hash.to_vec()))
        .execute(&state.db)
        .await?;
    state.taken_away();
    audit::record(&state.db, &who, None, "account.password", &who.username).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn server_exists(db: &SqlitePool, id: i64) -> Result<(), Problem> {
    let there: Option<i64> = sqlx::query_scalar("SELECT id FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?;
    there.map(|_| ()).ok_or(MISSING)
}

/// The accounts that have been let into a server, by name. The owner is not
/// among them: it is in every server.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/users",
    params(("id" = i64, Path, description = "The server's id.")),
    responses(
        (status = OK, body = Vec<ServerUser>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
    )
)]
async fn list_server_users(
    State(state): State<AppState>,
    _: Owner,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ServerUser>>, Problem> {
    server_exists(&state.db, id).await?;
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT users.id, users.username, server_users.permissions
         FROM server_users JOIN users ON users.id = server_users.user_id
         WHERE server_users.server_id = ? ORDER BY users.username",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let users = rows
        .into_iter()
        .map(|(user_id, username, permissions)| {
            Ok(ServerUser {
                user_id,
                username,
                permissions: read(&permissions)?,
            })
        })
        .collect::<Result<_, Problem>>()?;
    Ok(Json(users))
}

/// Lets an account into a server, or changes what it may do there. With no
/// permission named it may look, and no more.
#[utoipa::path(
    put,
    path = "/api/v1/servers/{id}/users/{user_id}",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("user_id" = i64, Path, description = "The account's id."),
    ),
    request_body = Grant,
    responses(
        (status = NO_CONTENT, description = "The account may do what was named, and no more."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account asking is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server, or no such account."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "It is the owner's account."),
    )
)]
async fn let_in(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path((id, user_id)): Path<(i64, i64)>,
    Json(grant): Json<Grant>,
) -> Result<StatusCode, Problem> {
    server_exists(&state.db, id).await?;
    let found: Option<(String, bool)> =
        sqlx::query_as("SELECT username, owner FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    let (username, owner) = found.ok_or(NO_ACCOUNT)?;
    if owner {
        return Err(Problem::Invalid(
            "The owner is in every server already.".into(),
        ));
    }
    // Each once, and in the order they are always told in.
    let permissions: Vec<Permission> = Permission::ALL
        .into_iter()
        .filter(|one| grant.permissions.contains(one))
        .collect();
    sqlx::query(
        "INSERT INTO server_users (server_id, user_id, permissions) VALUES (?, ?, ?)
         ON CONFLICT (server_id, user_id) DO UPDATE SET permissions = excluded.permissions",
    )
    .bind(id)
    .bind(user_id)
    .bind(serde_json::to_string(&permissions).map_err(anyhow::Error::new)?)
    .execute(&state.db)
    .await?;
    // What it may no longer do, it no longer does: by the clock, or over a
    // connection it opened while it still might.
    schedules::withdraw(&state.db, Some(id), user_id, &permissions).await?;
    state.taken_away();
    let named: Vec<&str> = permissions.iter().map(|one| one.as_str()).collect();
    let may = if named.is_empty() {
        "looking only".to_owned()
    } else {
        named.join(", ")
    };
    let detail = format!("{username}: {may}");
    audit::record(&state.db, &who, Some(id), "server.let_in", &detail).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes an account out of a server. To that account the server is then not there.
#[utoipa::path(
    delete,
    path = "/api/v1/servers/{id}/users/{user_id}",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("user_id" = i64, Path, description = "The account's id."),
    ),
    responses(
        (status = NO_CONTENT, description = "The account is out of the server."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account asking is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "That account was not in that server."),
    )
)]
async fn turn_out(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path((id, user_id)): Path<(i64, i64)>,
) -> Result<StatusCode, Problem> {
    let username: Option<String> = sqlx::query_scalar(
        "SELECT users.username FROM server_users JOIN users ON users.id = server_users.user_id
         WHERE server_users.server_id = ? AND server_users.user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(username) = username else {
        return Err(Problem::NotFound(
            "That account has not been let into that server.",
        ));
    };
    sqlx::query("DELETE FROM server_users WHERE server_id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;
    schedules::withdraw(&state.db, Some(id), user_id, &[]).await?;
    state.taken_away();
    audit::record(&state.db, &who, Some(id), "server.turn_out", &username).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Whether the account that asks has a second step to its sign-in.
#[derive(Serialize, ToSchema)]
struct TwoSteps {
    on: bool,
    /// How many of its recovery codes have not been used.
    recovery_codes: usize,
}

/// What an authenticator app is given to make the codes of an account.
#[derive(Serialize, ToSchema)]
struct TwoStepsSetup {
    /// The secret, to type into the app.
    secret: String,
    /// The same as a link, which an app on this device opens.
    uri: String,
}

#[derive(Deserialize, ToSchema)]
struct TypedCode {
    code: String,
}

/// The codes that sign an account in when its app is lost. Each works once,
/// and they are shown this one time.
#[derive(Serialize, ToSchema)]
struct RecoveryCodes {
    recovery_codes: Vec<String>,
}

/// Whether the account signed in here has a second step, and how many
/// recovery codes it has left.
#[utoipa::path(
    get,
    path = "/api/v1/account/two-steps",
    responses(
        (status = OK, body = TwoSteps),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn two_steps(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
) -> Result<Json<TwoSteps>, Problem> {
    let (secret, codes): (Option<String>, String) =
        sqlx::query_as("SELECT totp_secret, recovery_codes FROM users WHERE id = ?")
            .bind(who.id)
            .fetch_one(&state.db)
            .await?;
    let codes: Vec<String> = serde_json::from_str(&codes)
        .context("reading an account's recovery codes")
        .map_err(Problem::Internal)?;
    Ok(Json(TwoSteps {
        on: secret.is_some(),
        recovery_codes: codes.len(),
    }))
}

/// Begins giving the account a second step: makes a secret for an
/// authenticator app. Nothing changes until a code from that app has been
/// typed back, which shows the app has it.
#[utoipa::path(
    post,
    path = "/api/v1/account/two-steps",
    responses(
        (status = OK, body = TwoStepsSetup),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "The account has a second step already."),
    )
)]
async fn begin_two_steps(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
) -> Result<Json<TwoStepsSetup>, Problem> {
    let secret = totp::new_secret()?;
    let begun =
        sqlx::query("UPDATE users SET totp_pending = ? WHERE id = ? AND totp_secret IS NULL")
            .bind(&secret)
            .bind(who.id)
            .execute(&state.db)
            .await?;
    if begun.rows_affected() == 0 {
        return Err(Problem::Conflict(
            "This account has a second step already. Turn it off to set up another app.".into(),
        ));
    }
    Ok(Json(TwoStepsSetup {
        uri: totp::uri(&secret, &who.username),
        secret,
    }))
}

/// Turns the second step on, given a code from the app that was just set up.
/// The answer is the account's recovery codes, which are not shown again.
#[utoipa::path(
    post,
    path = "/api/v1/account/two-steps/confirm",
    request_body = TypedCode,
    responses(
        (status = OK, body = RecoveryCodes),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "No app is being set up."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The code is not the app's."),
    )
)]
async fn confirm_two_steps(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Json(typed): Json<TypedCode>,
) -> Result<Json<RecoveryCodes>, Problem> {
    let pending: Option<Option<String>> =
        sqlx::query_scalar("SELECT totp_pending FROM users WHERE id = ? AND totp_secret IS NULL")
            .bind(who.id)
            .fetch_optional(&state.db)
            .await?;
    let Some(secret) = pending.flatten() else {
        return Err(Problem::Conflict(
            "No authenticator app is being set up for this account.".into(),
        ));
    };
    let Some(step) = totp::passes(&secret, &typed.code, auth::now(), 0) else {
        return Err(Problem::Invalid(
            "That is not the code the app shows now. Check that the secret was typed as it is, and that this device's clock is right.".into(),
        ));
    };
    let (codes, kept) = totp::new_recovery_codes();
    sqlx::query(
        "UPDATE users SET totp_secret = ?, totp_pending = NULL, totp_step = ?, recovery_codes = ?
         WHERE id = ?",
    )
    .bind(secret)
    .bind(step)
    .bind(kept)
    .bind(who.id)
    .execute(&state.db)
    .await?;
    audit::record(&state.db, &who, None, "account.two_steps_on", &who.username).await;
    Ok(Json(RecoveryCodes {
        recovery_codes: codes,
    }))
}

/// Takes an account's second step away, and its recovery codes with it.
async fn without_two_steps(db: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE users
         SET totp_secret = NULL, totp_pending = NULL, totp_step = 0, recovery_codes = '[]'
         WHERE id = ?",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

/// Turns an account's second step off by its name: the way back in for whoever
/// has lost both their app and their recovery codes, run on the machine itself
/// (`homewarp two-steps-off <username>`). Whoever can run that can read the
/// database, so it asks for nothing. False if there is no such account.
pub async fn two_steps_off(db: &SqlitePool, username: &str) -> anyhow::Result<bool> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(db)
        .await?;
    let Some(id) = id else {
        return Ok(false);
    };
    without_two_steps(db, id).await?;
    audit::record_by_homewarp(db, None, "account.two_steps_off", username).await;
    Ok(true)
}

/// Turns the second step of the account that asks off, given its password.
#[utoipa::path(
    post,
    path = "/api/v1/account/two-steps/off",
    request_body = NewPassword,
    responses(
        (status = NO_CONTENT, description = "The account signs in with its password alone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The password is wrong."),
    )
)]
async fn end_two_steps(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Json(asked): Json<NewPassword>,
) -> Result<StatusCode, Problem> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(who.id)
        .fetch_one(&state.db)
        .await?;
    if !auth::verify(asked.password, Some(hash)).await? {
        return Err(Problem::Forbidden("That is not your password."));
    }
    without_two_steps(&state.db, who.id).await?;
    audit::record(
        &state.db,
        &who,
        None,
        "account.two_steps_off",
        &who.username,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Turns another account's second step off: for whoever has lost both their
/// app and their recovery codes.
#[utoipa::path(
    delete,
    path = "/api/v1/users/{id}/two-steps",
    params(("id" = i64, Path, description = "The account's id.")),
    responses(
        (status = NO_CONTENT, description = "The account signs in with its password alone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account asking is not the owner's."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such account."),
    )
)]
async fn end_two_steps_of(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let username = username.ok_or(NO_ACCOUNT)?;
    without_two_steps(&state.db, id).await?;
    audit::record(&state.db, &who, None, "account.two_steps_off", &username).await;
    Ok(StatusCode::NO_CONTENT)
}
