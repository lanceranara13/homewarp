//! Passkeys, as accounts have them (PLAN.md §11, Phase 5).
//!
//! An account makes a passkey in Settings, on a device that then holds it, and
//! from then on signs in with that device and nothing typed. The device checks
//! who is holding it, which is the second step, and proves it has the key,
//! which is the first. The password stays, for a browser that has no passkey.
//!
//! A passkey is bound to a name, and a browser makes one only on a page it
//! trusts. So there are passkeys where the panel is reached by its name over
//! TLS, and on the machine itself, by the name every machine has for itself.
//! On the home network, by an address and without TLS, there are none.
//!
//! What a browser sends is checked in `webauthn`. Here is what is kept: the
//! keys, and the challenges that are out.

use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Mutex, PoisonError},
};

use axum::{
    Json,
    extract::{FromRequestParts, Path, State},
    http::{HeaderMap, StatusCode, header::HOST, request::Parts},
    response::Response,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{self, AppState, Problem, ProblemBody, SignedIn, Trying, User},
    audit, auth,
    door::Client,
    panel,
    tls::Secured,
    webauthn::{self, Expected},
};

/// How long a challenge counts: long enough to find the device and touch it.
const CHALLENGE_SECONDS: i64 = 5 * 60;
/// How many challenges are out at once, at the most. Anyone may ask for one
/// to sign in with, so one more than this has the oldest forgotten.
const MOST_CHALLENGES: usize = 1024;
/// How many passkeys an account has, at the most.
const MOST_PASSKEYS: i64 = 10;
const LONGEST_NAME: usize = 40;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_passkeys, add_passkey))
        .routes(routes!(begin_passkey))
        .routes(routes!(remove_passkey))
        .routes(routes!(begin_sign_in))
        .routes(routes!(sign_in))
}

/// What a challenge was handed out for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// For this account to make a passkey with.
    Make(i64),
    /// For whoever has a passkey to sign in with.
    SignIn,
}

/// The challenges that are out, and until when each counts. Kept in memory: a
/// Core that is started again has none out, and whoever was in the middle of
/// it starts again.
#[derive(Default)]
pub(crate) struct Challenges(Mutex<HashMap<String, (Purpose, i64)>>);

impl Challenges {
    /// Makes a new challenge, in the base64 that uses neither `+` nor `/`.
    fn hand_out(&self, purpose: Purpose) -> String {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the operating system has a source of randomness");
        let challenge = URL_SAFE_NO_PAD.encode(bytes);
        let now = auth::now();
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        out.retain(|_, (_, until)| *until > now);
        if out.len() >= MOST_CHALLENGES {
            let oldest = out
                .iter()
                .min_by_key(|(_, (_, until))| *until)
                .map(|(challenge, _)| challenge.clone());
            if let Some(oldest) = oldest {
                out.remove(&oldest);
            }
        }
        out.insert(challenge.clone(), (purpose, now + CHALLENGE_SECONDS));
        challenge
    }

    /// Takes a challenge back, and says what it was for. It counts once.
    fn take(&self, challenge: &str) -> Option<Purpose> {
        let mut out = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (purpose, until) = out.remove(challenge)?;
        (until > auth::now()).then_some(purpose)
    }
}

/// Whether a request came in over TLS.
struct OverTls(bool);

impl<S: Send + Sync> FromRequestParts<S> for OverTls {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Infallible> {
        Ok(Self(parts.extensions.get::<Secured>().is_some()))
    }
}

/// The site a request was made on, as a passkey knows it.
struct Site {
    /// Such as `https://panel.example.com:8443`.
    origin: String,
    /// The name a key made here is bound to.
    rp_id: String,
}

/// The site a request was made on, if passkeys can be had there: the panel's
/// own name over TLS, or this machine as it calls itself.
///
/// What the request says of where it was sent is not taken on trust. It is
/// held against the name the owner gave the panel, and a browser holds the
/// same against the page it is on: a page that only looks like the panel is
/// on another name, and gets nothing a key made here will sign.
async fn site(state: &AppState, headers: &HeaderMap, secured: bool) -> Result<Site, Problem> {
    let host = headers
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        .unwrap_or_default();
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };
    if name == "localhost" {
        let scheme = if secured { "https" } else { "http" };
        return Ok(Site {
            origin: format!("{scheme}://{host}"),
            rp_id: name.to_owned(),
        });
    }
    if secured && panel::name(&state.db).await?.as_deref() == Some(name) {
        return Ok(Site {
            origin: format!("https://{host}"),
            rp_id: name.to_owned(),
        });
    }
    Err(Problem::Conflict(
        "Passkeys are made and used where the panel is reached by its name, over TLS.".into(),
    ))
}

/// Reads what a browser sent in the base64 that uses neither `+` nor `/`.
fn decoded(text: &str) -> Result<Vec<u8>, Problem> {
    URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| Problem::Invalid("What the browser sent could not be read.".into()))
}

/// A passkey, as its owner sees it.
#[derive(Serialize, ToSchema)]
struct PasskeyView {
    id: i64,
    /// What its owner called it.
    name: String,
    /// In Unix seconds, as every time here.
    created_at: i64,
    /// When it last signed in. Nothing if it never has.
    last_used_at: Option<i64>,
}

/// An account's passkeys.
#[derive(Serialize, ToSchema)]
struct Passkeys {
    /// Whether one can be made, or used, where this was asked from.
    available: bool,
    passkeys: Vec<PasskeyView>,
}

/// What making a passkey begins with.
#[derive(Deserialize, ToSchema)]
struct BeginPasskey {
    /// The account's password: a passkey is another way in, and whoever adds
    /// one has to be the account's owner and not only someone at its browser.
    password: String,
}

/// What a browser is asked to make a passkey with.
#[derive(Serialize, ToSchema)]
struct MakeOptions {
    /// What the device is to sign, in the base64 that uses neither `+` nor `/`.
    challenge: String,
    /// The name the key is bound to.
    rp_id: String,
    /// What the device files the key under: the account, in that base64.
    user_handle: String,
    username: String,
    /// The keys the account has here already, by what their devices call
    /// them, so that a device is not given a second.
    exclude: Vec<String>,
}

/// A passkey a browser has just had made.
#[derive(Deserialize, ToSchema)]
struct NewPasskey {
    /// What to call it: "Laptop", "Phone".
    name: String,
    /// What the browser says it was asked, and what the device answered, each
    /// in the base64 that uses neither `+` nor `/`.
    client_data: String,
    attestation: String,
}

/// What a browser is asked to sign in with.
#[derive(Serialize, ToSchema)]
struct SignInOptions {
    challenge: String,
    rp_id: String,
}

/// A sign-in made with a passkey.
#[derive(Deserialize, ToSchema)]
struct PasskeySignIn {
    /// What the device calls the key that signed.
    id: String,
    client_data: String,
    authenticator_data: String,
    signature: String,
}

/// The passkeys of whoever asks.
#[utoipa::path(
    get,
    path = "/api/v1/account/passkeys",
    responses(
        (status = OK, body = Passkeys),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn list_passkeys(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    OverTls(secured): OverTls,
    headers: HeaderMap,
) -> Result<Json<Passkeys>, Problem> {
    let rows: Vec<(i64, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ? ORDER BY id",
    )
    .bind(who.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Passkeys {
        available: site(&state, &headers, secured).await.is_ok(),
        passkeys: rows
            .into_iter()
            .map(|(id, name, created_at, last_used_at)| PasskeyView {
                id,
                name,
                created_at,
                last_used_at,
            })
            .collect(),
    }))
}

/// Begins making a passkey: checks the account's password, and answers with
/// what the browser is to ask its device for. The challenge counts for five
/// minutes, and once.
#[utoipa::path(
    post,
    path = "/api/v1/account/passkeys/begin",
    request_body = BeginPasskey,
    responses(
        (status = OK, body = MakeOptions),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The password is wrong."),
        (status = CONFLICT, body = ProblemBody, description = "There are no passkeys where this was asked from, or the account has as many as it can."),
        (status = TOO_MANY_REQUESTS, body = ProblemBody, description = "Too many wrong passwords."),
    )
)]
async fn begin_passkey(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    client: Client,
    OverTls(secured): OverTls,
    headers: HeaderMap,
    Json(asked): Json<BeginPasskey>,
) -> Result<Json<MakeOptions>, Problem> {
    let site = site(&state, &headers, secured).await?;
    let trying = Trying::new(client, &who.username);
    trying.may(&state.limits)?;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(who.id)
        .fetch_one(&state.db)
        .await?;
    let right = tokio::task::spawn_blocking(move || auth::verify_password(&asked.password, &hash))
        .await
        .map_err(anyhow::Error::new)?;
    if !right {
        trying.failed(&state.limits);
        return Err(Problem::Forbidden("That is not your password."));
    }
    trying.passed(&state.limits);
    let kept: Vec<(Vec<u8>,)> =
        sqlx::query_as("SELECT credential_id FROM passkeys WHERE user_id = ?")
            .bind(who.id)
            .fetch_all(&state.db)
            .await?;
    if kept.len() >= MOST_PASSKEYS as usize {
        return Err(Problem::Conflict(
            "An account has ten passkeys at the most. Remove one first.".into(),
        ));
    }
    Ok(Json(MakeOptions {
        challenge: state.challenges.hand_out(Purpose::Make(who.id)),
        rp_id: site.rp_id,
        user_handle: URL_SAFE_NO_PAD.encode(who.id.to_be_bytes()),
        username: who.username,
        exclude: kept
            .into_iter()
            .map(|(id,)| URL_SAFE_NO_PAD.encode(id))
            .collect(),
    }))
}

/// Keeps a passkey that the browser has just had made with the challenge
/// `begin` handed out.
#[utoipa::path(
    post,
    path = "/api/v1/account/passkeys",
    request_body = NewPasskey,
    responses(
        (status = CREATED, body = PasskeyView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "The challenge is no longer out, the key is here already, or there are no passkeys where this was asked from."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "What the device answered does not hold."),
    )
)]
async fn add_passkey(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    OverTls(secured): OverTls,
    headers: HeaderMap,
    Json(new): Json<NewPasskey>,
) -> Result<(StatusCode, Json<PasskeyView>), Problem> {
    let site = site(&state, &headers, secured).await?;
    let name = new.name.trim().to_owned();
    if !(1..=LONGEST_NAME).contains(&name.chars().count()) {
        return Err(Problem::Invalid(
            "A passkey's name is 1 to 40 characters.".into(),
        ));
    }
    let client_data = decoded(&new.client_data)?;
    let attestation = decoded(&new.attestation)?;
    let challenge = webauthn::challenge(&client_data)
        .filter(|challenge| state.challenges.take(challenge) == Some(Purpose::Make(who.id)))
        .ok_or_else(|| Problem::Conflict("That took too long. Start again.".into()))?;
    let expected = Expected {
        challenge: &challenge,
        origin: &site.origin,
        rp_id: &site.rp_id,
    };
    let made = webauthn::made(&client_data, &attestation, &expected)
        .map_err(|why| Problem::Invalid(why.into()))?;
    let now = auth::now();
    let id = sqlx::query(
        "INSERT INTO passkeys
             (user_id, credential_id, public_key, sign_count, rp_id, name, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(who.id)
    .bind(&made.id)
    .bind(&made.public_key)
    .bind(made.count)
    .bind(&site.rp_id)
    .bind(&name)
    .bind(now)
    .execute(&state.db)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(error) if error.is_unique_violation() => {
            Problem::Conflict("That passkey is here already.".into())
        }
        other => other.into(),
    })?
    .last_insert_rowid();
    audit::record(&state.db, &who, None, "account.passkey_add", &name).await;
    Ok((
        StatusCode::CREATED,
        Json(PasskeyView {
            id,
            name,
            created_at: now,
            last_used_at: None,
        }),
    ))
}

/// Removes one of the passkeys of whoever asks. The device goes on holding
/// its half, which opens nothing here any more.
#[utoipa::path(
    delete,
    path = "/api/v1/account/passkeys/{id}",
    params(("id" = i64, Path, description = "The passkey.")),
    responses(
        (status = NO_CONTENT, description = "It is gone."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "The account has no such passkey."),
    )
)]
async fn remove_passkey(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let name: Option<String> =
        sqlx::query_scalar("DELETE FROM passkeys WHERE id = ? AND user_id = ? RETURNING name")
            .bind(id)
            .bind(who.id)
            .fetch_optional(&state.db)
            .await?;
    let Some(name) = name else {
        return Err(Problem::NotFound("There is no such passkey."));
    };
    audit::record(&state.db, &who, None, "account.passkey_remove", &name).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Begins a sign-in with a passkey: answers with a challenge for the device
/// to sign. Nobody is named: the device says which key it has.
#[utoipa::path(
    post,
    path = "/api/v1/login/passkey/begin",
    responses(
        (status = OK, body = SignInOptions),
        (status = CONFLICT, body = ProblemBody, description = "There are no passkeys where this was asked from."),
    )
)]
async fn begin_sign_in(
    State(state): State<AppState>,
    OverTls(secured): OverTls,
    headers: HeaderMap,
) -> Result<Json<SignInOptions>, Problem> {
    let site = site(&state, &headers, secured).await?;
    Ok(Json(SignInOptions {
        challenge: state.challenges.hand_out(Purpose::SignIn),
        rp_id: site.rp_id,
    }))
}

/// Signs in with a passkey: the device signed the challenge, and checked who
/// was holding it, so neither a password nor a code is asked for.
#[utoipa::path(
    post,
    path = "/api/v1/login/passkey",
    request_body = PasskeySignIn,
    responses(
        (status = OK, body = api::Session, description = "Signed in. The session cookie is set."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "The passkey did not sign in."),
        (status = CONFLICT, body = ProblemBody, description = "There are no passkeys where this was asked from."),
        (status = TOO_MANY_REQUESTS, body = ProblemBody, description = "Too many sign-ins that failed."),
    )
)]
async fn sign_in(
    State(state): State<AppState>,
    client: Client,
    OverTls(secured): OverTls,
    headers: HeaderMap,
    Json(asked): Json<PasskeySignIn>,
) -> Result<Response, Problem> {
    let site = site(&state, &headers, secured).await?;
    let id = decoded(&asked.id)?;
    let client_data = decoded(&asked.client_data)?;
    let authenticator_data = decoded(&asked.authenticator_data)?;
    let signature = decoded(&asked.signature)?;

    type Row = (i64, i64, String, bool, Vec<u8>, i64, String);
    let found: Option<Row> = sqlx::query_as(
        "SELECT passkeys.id, users.id, users.username, users.owner, passkeys.public_key,
                passkeys.sign_count, passkeys.rp_id
         FROM passkeys JOIN users ON users.id = passkeys.user_id
         WHERE passkeys.credential_id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    // A key nobody has is counted against where it came from, and against no account.
    let named = found
        .as_ref()
        .map_or("(no such passkey)", |found| found.2.as_str());
    let trying = Trying::new(client, named);
    trying.may(&state.limits)?;
    let from = client.0.to_string();

    // An answer to a challenge that is not out is no guess at anything.
    let challenge = webauthn::challenge(&client_data)
        .filter(|challenge| state.challenges.take(challenge) == Some(Purpose::SignIn))
        .ok_or(Problem::Unauthorized(
            "That sign-in took too long. Try again.",
        ))?;
    let refused = Problem::Unauthorized("That passkey does not open this panel.");
    let Some((passkey, user_id, username, owner, public_key, kept, made_for)) = found else {
        trying.failed(&state.limits);
        return Err(refused);
    };
    let user = User {
        id: user_id,
        username,
        owner,
    };
    let expected = Expected {
        challenge: &challenge,
        origin: &site.origin,
        rp_id: &site.rp_id,
    };
    let signed = match made_for == site.rp_id {
        true => webauthn::signed(
            &client_data,
            &authenticator_data,
            &signature,
            &public_key,
            &expected,
        ),
        false => Err("That passkey is for another site."),
    };
    let count = match signed {
        Ok(count) => i64::from(count),
        Err(why) => {
            trying.failed(&state.limits);
            audit::record(&state.db, &user, None, "account.sign_in_failed", &from).await;
            return Err(Problem::Unauthorized(why));
        }
    };
    // A device that counts its uses counts up. One that has not is a second
    // holder of the key, and which of the two is the owner's cannot be told.
    if (count != 0 || kept != 0) && count <= kept {
        trying.failed(&state.limits);
        audit::record(&state.db, &user, None, "account.sign_in_failed", &from).await;
        return Err(Problem::Unauthorized(
            "That passkey has been used more often than its device says. It may have been copied, so it is not taken. Sign in with your password and remove it.",
        ));
    }
    let now = auth::now();
    sqlx::query("UPDATE passkeys SET sign_count = ?, last_used_at = ? WHERE id = ?")
        .bind(count)
        .bind(now)
        .bind(passkey)
        .execute(&state.db)
        .await?;
    trying.passed(&state.limits);
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(&state.db)
        .await?;
    let detail = format!("{from}, with a passkey");
    audit::record(&state.db, &user, None, "account.sign_in", &detail).await;
    api::sign_in(&state, user).await
}

#[cfg(test)]
mod tests {
    use super::{Challenges, Purpose};

    #[test]
    fn a_challenge_counts_once_and_for_what_it_was_handed_out_for() {
        let challenges = Challenges::default();
        let (first, second) = (
            challenges.hand_out(Purpose::Make(7)),
            challenges.hand_out(Purpose::SignIn),
        );
        assert_ne!(first, second);
        // 32 bytes, in letters that need no escaping anywhere.
        assert_eq!(first.len(), 43);
        assert!(
            first
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_eq!(challenges.take(&first), Some(Purpose::Make(7)));
        assert_eq!(challenges.take(&first), None);
        assert_eq!(challenges.take(&second), Some(Purpose::SignIn));
        assert_eq!(challenges.take("never handed out"), None);
    }

    #[test]
    fn no_more_challenges_are_kept_than_there_is_room_for() {
        let challenges = Challenges::default();
        let first = challenges.hand_out(Purpose::SignIn);
        for _ in 0..super::MOST_CHALLENGES {
            challenges.hand_out(Purpose::SignIn);
        }
        assert_eq!(challenges.0.lock().unwrap().len(), super::MOST_CHALLENGES);
        // All of one age, so which went is not said: only that one did.
        let last = challenges.hand_out(Purpose::SignIn);
        assert_eq!(challenges.take(&last), Some(Purpose::SignIn));
        let _ = first;
    }
}

/// The whole of it as a browser goes through it, with a device that is a few
/// lines of the tests' own. The panel here has a name and is asked over TLS,
/// which a test inside this crate can say of a request and one outside cannot.
#[cfg(test)]
mod through {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{
            Request, StatusCode,
            header::{CONTENT_TYPE, COOKIE, HOST, SET_COOKIE},
        },
    };
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::{
        api::{self, AppState},
        db,
        panel::Authority,
        tls::Secured,
        webauthn::device::{Device, client_data},
    };

    const NAME: &str = "panel.example.com";
    const HERE: &str = "panel.example.com:8443";
    const ORIGIN: &str = "https://panel.example.com:8443";
    const HOME: &str = "192.168.1.250:3600";
    const PASSWORD: &str = "correct horse battery";
    const KEYS: &str = "/api/v1/account/passkeys";

    struct Panel {
        app: Router,
        _files: tempfile::TempDir,
    }

    struct Answer {
        status: StatusCode,
        set_cookie: Option<String>,
        body: Value,
    }

    impl Answer {
        fn cookie(&self) -> String {
            let set = self
                .set_cookie
                .as_deref()
                .expect("the answer sets a cookie");
            set.split(';').next().unwrap().to_owned()
        }
    }

    impl Panel {
        /// A panel with a name and an owner, and the owner's session.
        async fn new() -> (Self, String) {
            let files = tempfile::tempdir().unwrap();
            let db = db::open(&files.path().join("homewarp.db")).await.unwrap();
            let state = AppState::start(db.clone(), files.path(), None)
                .await
                .unwrap()
                .tls_at(Some(8443), Authority::default());
            let code = state.setup_code().unwrap().to_owned();
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES ('panel_name', '\"panel.example.com\"')",
            )
            .execute(&db)
            .await
            .unwrap();
            let panel = Self {
                app: api::app(state),
                _files: files,
            };
            let account = json!({ "code": code, "username": "lance", "password": PASSWORD });
            let made = panel
                .ask("POST", "/api/v1/setup", None, (HERE, true), account)
                .await;
            assert_eq!(made.status, StatusCode::OK, "{}", made.body);
            let cookie = made.cookie();
            (panel, cookie)
        }

        /// One request, to the site it names, over TLS or not.
        async fn ask(
            &self,
            method: &str,
            path: &str,
            cookie: Option<&str>,
            (host, secured): (&str, bool),
            body: Value,
        ) -> Answer {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header(HOST, host)
                .header(CONTENT_TYPE, "application/json")
                .header(COOKIE, cookie.unwrap_or_default());
            if secured {
                request = request.extension(Secured);
            }
            let request = request.body(Body::from(body.to_string())).unwrap();
            let response = self.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let set_cookie = response
                .headers()
                .get(SET_COOKIE)
                .map(|value| value.to_str().unwrap().to_owned());
            let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
            Answer {
                status,
                set_cookie,
                body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            }
        }

        /// A challenge to sign in with, as anyone may ask for one.
        async fn challenge(&self) -> String {
            let begun = self
                .ask(
                    "POST",
                    "/api/v1/login/passkey/begin",
                    None,
                    (HERE, true),
                    Value::Null,
                )
                .await;
            assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
            assert_eq!(begun.body["rp_id"], NAME);
            begun.body["challenge"].as_str().unwrap().to_owned()
        }

        /// What comes of a device signing a challenge on the page at `origin`.
        async fn sign_in(&self, device: &mut Device, challenge: &str, origin: &str) -> Answer {
            let asked = client_data("webauthn.get", challenge, origin);
            let (data, signature) = device.sign(NAME, &asked);
            let signed = json!({
                "id": text(&device.id),
                "client_data": text(&asked),
                "authenticator_data": text(&data),
                "signature": text(&signature),
            });
            self.ask("POST", "/api/v1/login/passkey", None, (HERE, true), signed)
                .await
        }
    }

    fn text(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    #[tokio::test]
    async fn there_are_passkeys_where_the_panel_has_its_name_and_tls_and_nowhere_else() {
        let (panel, cookie) = Panel::new().await;
        let cookie = Some(cookie.as_str());
        let begin = format!("{KEYS}/begin");
        let password = json!({ "password": PASSWORD });

        for (site, there) in [
            // By its name, over TLS.
            ((HERE, true), true),
            // On the machine itself, which a browser trusts as it is.
            (("localhost:3600", false), true),
            // By an address on the home network.
            ((HOME, false), false),
            // By the name without TLS, and by another name with it.
            ((HERE, false), false),
            (("panel.example.net:8443", true), false),
            (("localhost.example.net", true), false),
        ] {
            let listed = panel.ask("GET", KEYS, cookie, site, Value::Null).await;
            assert_eq!(
                listed.body,
                json!({ "available": there, "passkeys": [] }),
                "{site:?}"
            );
            let begun = panel
                .ask("POST", &begin, cookie, site, password.clone())
                .await;
            let sign_in = panel
                .ask(
                    "POST",
                    "/api/v1/login/passkey/begin",
                    None,
                    site,
                    Value::Null,
                )
                .await;
            let expected = match there {
                true => StatusCode::OK,
                false => StatusCode::CONFLICT,
            };
            assert_eq!(
                (begun.status, sign_in.status),
                (expected, expected),
                "{site:?}"
            );
        }
        // And for nobody who is not signed in.
        let nobody = panel
            .ask("GET", KEYS, None, (HERE, true), Value::Null)
            .await;
        assert_eq!(nobody.status, StatusCode::UNAUTHORIZED);
        let nobody = panel
            .ask("POST", &begin, None, (HERE, true), password)
            .await;
        assert_eq!(nobody.status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_passkey_is_made_with_the_password_and_then_signs_in_without_one() {
        let (panel, cookie) = Panel::new().await;
        let cookie = Some(cookie.as_str());
        let here = (HERE, true);
        let begin = format!("{KEYS}/begin");

        // Adding a way in takes the password, and not only the session.
        let wrong = json!({ "password": "not the password" });
        let refused = panel.ask("POST", &begin, cookie, here, wrong).await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        let begun = panel
            .ask(
                "POST",
                &begin,
                cookie,
                here,
                json!({ "password": PASSWORD }),
            )
            .await;
        assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
        assert_eq!(begun.body["rp_id"], NAME);
        assert_eq!(begun.body["username"], "lance");
        assert_eq!(begun.body["exclude"], json!([]));
        let challenge = begun.body["challenge"].as_str().unwrap();

        let mut device = Device::new();
        let new = json!({
            "name": " Laptop ",
            "client_data": text(&client_data("webauthn.create", challenge, ORIGIN)),
            "attestation": text(&device.make(NAME)),
        });
        // A name is asked for, and what the browser sent has to be readable.
        for (change, status) in [
            (json!({ "name": "  " }), StatusCode::UNPROCESSABLE_ENTITY),
            (
                json!({ "attestation": "not base64 !" }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let mut odd = new.clone();
            for (key, value) in change.as_object().unwrap() {
                odd[key] = value.clone();
            }
            assert_eq!(
                panel.ask("POST", KEYS, cookie, here, odd).await.status,
                status
            );
        }
        let added = panel.ask("POST", KEYS, cookie, here, new.clone()).await;
        assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
        assert_eq!(added.body["name"], "Laptop");
        // The challenge counted once: the same answer again is to one that is not out.
        let again = panel.ask("POST", KEYS, cookie, here, new).await;
        assert_eq!(again.status, StatusCode::CONFLICT);

        let listed = panel.ask("GET", KEYS, cookie, here, Value::Null).await;
        let kept = listed.body["passkeys"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["last_used_at"], Value::Null);
        let id = kept[0]["id"].as_i64().unwrap();
        // The device is not asked to make a second: it is told which it has.
        let begun = panel
            .ask(
                "POST",
                &begin,
                cookie,
                here,
                json!({ "password": PASSWORD }),
            )
            .await;
        assert_eq!(begun.body["exclude"], json!([text(&device.id)]));

        // Signing in: nobody is named and nothing is typed.
        let challenge = panel.challenge().await;
        let signed = panel.sign_in(&mut device, &challenge, ORIGIN).await;
        assert_eq!(signed.status, StatusCode::OK, "{}", signed.body);
        assert_eq!(signed.body["user"]["username"], "lance");
        assert_eq!(signed.body["user"]["owner"], true);
        // Over TLS, with a cookie that is sent back over TLS only.
        assert!(signed.set_cookie.as_deref().unwrap().ends_with("; Secure"));
        let session = signed.cookie();
        let whose = panel
            .ask("GET", "/api/v1/session", Some(&session), here, Value::Null)
            .await;
        assert_eq!(whose.body["user"]["username"], "lance");
        let listed = panel.ask("GET", KEYS, cookie, here, Value::Null).await;
        assert!(listed.body["passkeys"][0]["last_used_at"].is_i64());

        // The same challenge a second time is one that is not out.
        let replayed = panel.sign_in(&mut device, &challenge, ORIGIN).await;
        assert_eq!(replayed.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            replayed.body["error"],
            "That sign-in took too long. Try again."
        );

        // A page that only looks like the panel, on a name that is not its.
        let challenge = panel.challenge().await;
        let phished = panel
            .sign_in(&mut device, &challenge, "https://panel.example.net:8443")
            .await;
        assert_eq!(phished.status, StatusCode::UNAUTHORIZED);
        assert_eq!(phished.body["error"], "That was made on another site.");

        // A copy of the key, on a device that has counted fewer uses.
        let challenge = panel.challenge().await;
        device.count = 0;
        let copied = panel.sign_in(&mut device, &challenge, ORIGIN).await;
        assert_eq!(copied.status, StatusCode::UNAUTHORIZED);
        assert!(copied.body["error"].as_str().unwrap().contains("copied"));

        // A key that was never added here.
        let challenge = panel.challenge().await;
        let unknown = panel.sign_in(&mut Device::new(), &challenge, ORIGIN).await;
        assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            unknown.body["error"],
            "That passkey does not open this panel."
        );

        // Removed, it opens nothing, and there is nothing to remove twice.
        let path = format!("{KEYS}/{id}");
        let removed = panel.ask("DELETE", &path, cookie, here, Value::Null).await;
        assert_eq!(removed.status, StatusCode::NO_CONTENT);
        let removed = panel.ask("DELETE", &path, cookie, here, Value::Null).await;
        assert_eq!(removed.status, StatusCode::NOT_FOUND);
        device.count = 100;
        let challenge = panel.challenge().await;
        let gone = panel.sign_in(&mut device, &challenge, ORIGIN).await;
        assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
    }
}
