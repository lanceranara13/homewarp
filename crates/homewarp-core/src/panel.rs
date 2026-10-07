//! The panel online (PLAN.md §5.9; §11, Phase 5): the name it is reached by
//! from the internet, and the certificate for that name.
//!
//! The name leads to the VPS, which forwards the panel's port home as it
//! forwards a server's, and TLS is ended here (`tls`). So the certificate is
//! asked for from here as well, and its key is made here and stays here. The
//! authority asks its question of the name, which is to say of the VPS's port
//! 80; Core has the answer, and has the Gate put it there for as long as the
//! asking takes. Nothing the VPS is given is a secret, and nothing of the
//! panel is reached through it but the port that speaks TLS.
//!
//! What is kept is three rows of `settings`: the name, the certificate with
//! its key, and the account with the authority.

use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::{Context, bail};
use axum::{Json, extract::State, http::StatusCode};
use homewarp_proto::{AnsweredBy, Answering};
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, NewAccount, NewOrder, Order,
    OrderStatus, RetryPolicy,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::SqlitePool;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Notify,
    time::timeout,
};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody},
    audit, auth, tls,
};

/// Who certificates are asked of unless Homewarp is told otherwise.
const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
/// The rows of `settings` this keeps.
const NAME: &str = "panel_name";
const CERTIFICATE: &str = "panel_certificate";
const ACCOUNT: &str = "panel_account";
/// How often the certificate is looked at, to see whether it is due.
const LOOK: Duration = Duration::from_secs(3600);
/// How long after an asking failed the next is made by itself. An authority
/// counts failures, and stops listening to whoever goes on failing.
const AGAIN_AFTER: i64 = 3600;
/// How long the authority is given to ask its question and make up its mind.
const PATIENT: RetryPolicy = RetryPolicy::new().timeout(Duration::from_secs(90));
/// How long the VPS's port 80 is given to answer Core's own asking.
const ASKING: Duration = Duration::from_secs(8);
/// A certificate with no dates that could be read is taken to last this long.
const UNREAD_DAYS: i64 = 60;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_panel, change_panel))
        .routes(routes!(ask_again))
}

/// Who certificates are asked of.
#[derive(Debug, Clone)]
pub struct Authority {
    /// Its ACME directory.
    pub directory: String,
    /// The root its own address is trusted by, as a PEM file, where that is
    /// none of the roots this machine trusts already: an authority of one's own.
    pub root: Option<PathBuf>,
}

impl Default for Authority {
    fn default() -> Self {
        Self {
            directory: LETS_ENCRYPT.to_owned(),
            root: None,
        }
    }
}

impl Authority {
    /// Let's Encrypt, unless a directory is given.
    pub fn at(directory: Option<String>, root: Option<PathBuf>) -> Self {
        Self {
            directory: directory.unwrap_or_else(|| LETS_ENCRYPT.to_owned()),
            root,
        }
    }

    /// What it is called where it is shown: the machine its directory is on.
    fn name(&self) -> &str {
        host(&self.directory)
    }
}

/// The machine a URL names.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or(rest);
    host.split(':').next().unwrap_or(host)
}

/// The certificate, as it is kept.
#[derive(Clone, Serialize, Deserialize)]
struct Certificate {
    name: String,
    /// The chain and its key, both in PEM.
    chain: String,
    key: String,
    issued_at: i64,
    /// From when and until when it counts, where that could be read off it.
    not_before: Option<i64>,
    not_after: Option<i64>,
    /// The directory of the authority that issued it.
    authority: String,
}

impl Certificate {
    fn until(&self) -> i64 {
        self.not_after
            .unwrap_or(self.issued_at + UNREAD_DAYS * 86_400)
    }

    /// When a newer one is asked for: with a third of its time left, which
    /// for one of ninety days is a month before it ends.
    fn renew_at(&self) -> i64 {
        let from = self.not_before.unwrap_or(self.issued_at);
        let until = self.until();
        until - (until - from) / 3
    }
}

/// The account with the authority, as it is kept.
#[derive(Serialize, Deserialize)]
struct KeptAccount {
    directory: String,
    /// The account's own address at the authority.
    id: String,
    credentials: serde_json::Value,
}

/// What is known of the asking, from one look at the page to the next.
#[derive(Default)]
pub(crate) struct Panel {
    /// Rung when the name has changed, or the owner has asked for another try.
    wake: Notify,
    said: Mutex<Said>,
}

#[derive(Default, Clone)]
struct Said {
    asking: bool,
    /// Why the last asking failed, and when.
    problem: Option<(String, i64)>,
    /// Where a web server on the VPS has to serve the answers from, when the
    /// Gate last said that one has port 80.
    answers: Option<String>,
    /// The owner asked: the hour after a failure is not waited out.
    now: bool,
}

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn kept<T: DeserializeOwned>(db: &SqlitePool, key: &str) -> anyhow::Result<Option<T>> {
    let json: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(db)
        .await?;
    json.map(|json| serde_json::from_str(&json).with_context(|| format!("reading {key}")))
        .transpose()
}

async fn keep<T: Serialize>(db: &SqlitePool, key: &str, value: &T) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(serde_json::to_string(value)?)
    .execute(db)
    .await?;
    Ok(())
}

/// The name the panel has been given, if it has been given one.
pub(crate) async fn name(db: &SqlitePool) -> anyhow::Result<Option<String>> {
    kept(db, NAME).await
}

/// Whether text is a name a certificate can be had for: lower-case labels with
/// dots between, and not an address.
fn is_name(text: &str) -> bool {
    let labels: Vec<&str> = text.split('.').collect();
    let label = |label: &&str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    };
    text.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(label)
        && labels
            .last()
            .is_some_and(|last| !last.bytes().all(|c| c.is_ascii_digit()))
}

/// Keeps the certificate: shows the one there is, and asks for a newer one
/// when it is due or there is none. For as long as Core runs.
pub(crate) async fn keep_certificate(state: AppState) {
    loop {
        if let Err(error) = round(&state).await {
            tracing::warn!("The panel's certificate: {error:#}");
            lock(&state.panel.said).problem = Some((format!("{error:#}"), auth::now()));
        }
        lock(&state.panel.said).asking = false;
        tokio::select! {
            () = tokio::time::sleep(LOOK) => {}
            () = state.panel.wake.notified() => {}
        }
    }
}

async fn round(state: &AppState) -> anyhow::Result<()> {
    let Some(name) = name(&state.db).await? else {
        state.shown.hide();
        return Ok(());
    };
    let now = auth::now();
    let certificate = kept::<Certificate>(&state.db, CERTIFICATE)
        .await?
        .filter(|kept| kept.name == name && kept.authority == state.authority.directory);
    match &certificate {
        Some(certificate) => {
            state.shown.show(&certificate.chain, &certificate.key)?;
            if now < certificate.renew_at() {
                return Ok(());
            }
        }
        None => state.shown.hide(),
    }
    {
        let mut said = lock(&state.panel.said);
        let asked = std::mem::take(&mut said.now);
        let failed_lately = said
            .problem
            .as_ref()
            .is_some_and(|(_, at)| now - at < AGAIN_AFTER);
        if failed_lately && !asked {
            return Ok(());
        }
        said.asking = true;
    }
    let made = ask(state, &name).await?;
    keep(&state.db, CERTIFICATE, &made).await?;
    state.shown.show(&made.chain, &made.key)?;
    lock(&state.panel.said).problem = None;
    audit::record_by_homewarp(&state.db, None, "panel.certificate", &name).await;
    tracing::info!("The panel has a certificate for {name}.");
    Ok(())
}

/// The account certificates are asked for under: the one that is kept, or a
/// new one where there is none with this authority.
async fn account(state: &AppState) -> anyhow::Result<Account> {
    let authority = &state.authority;
    let builder = || match &authority.root {
        Some(root) => Account::builder_with_root(root),
        None => Account::builder(),
    };
    let kept_account = kept::<KeptAccount>(&state.db, ACCOUNT)
        .await?
        .filter(|kept| kept.directory == authority.directory);
    if let Some(kept) = kept_account {
        let credentials = serde_json::from_value(kept.credentials)?;
        return Ok(builder()?.from_credentials(credentials).await?);
    }
    let new = NewAccount {
        contact: &[],
        // The owner agreed, where the name was given: `change_panel` asks.
        terms_of_service_agreed: true,
        only_return_existing: false,
    };
    let (account, credentials) = builder()?
        .create(&new, authority.directory.clone(), None)
        .await
        .with_context(|| format!("{} could not be reached", authority.name()))?;
    let made = KeptAccount {
        directory: authority.directory.clone(),
        id: account.id().to_owned(),
        credentials: serde_json::to_value(&credentials)?,
    };
    keep(&state.db, ACCOUNT, &made).await?;
    Ok(account)
}

/// Asks the authority for a certificate for `name`, and sees it through.
async fn ask(state: &AppState, name: &str) -> anyhow::Result<Certificate> {
    let account = account(state).await?;
    let identifiers = [Identifier::Dns(name.to_owned())];
    let mut order = account
        .new_order(&NewOrder::new(&identifiers))
        .await
        .with_context(|| format!("{} would not take the request", state.authority.name()))?;
    let mut tokens = Vec::new();
    let answered = answer(state, name, &mut order, &mut tokens).await;
    // Whatever came of it, the answers are taken away again.
    for token in &tokens {
        if let Err(error) = state.tunnel.unanswer(token).await {
            tracing::warn!("An answer was left on the VPS: {error:#}");
        }
    }
    answered?;
    let key = order.finalize().await?;
    let chain = order.poll_certificate(&PATIENT).await?;
    let (not_before, not_after) = tls::validity(&chain).unzip();
    Ok(Certificate {
        name: name.to_owned(),
        chain,
        key,
        issued_at: auth::now(),
        not_before,
        not_after,
        authority: state.authority.directory.clone(),
    })
}

/// Answers what the authority asks about `name`, and waits for it to be
/// satisfied. `tokens` is every answer that was put on the VPS, for taking
/// away again.
async fn answer(
    state: &AppState,
    name: &str,
    order: &mut Order,
    tokens: &mut Vec<String>,
) -> anyhow::Result<()> {
    // Whether the Gate serves the answers itself, to a port 80 that home did not get through to.
    let mut shut = false;
    {
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization?;
            match authorization.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => bail!("the authority will not ask about {name}: {other:?}"),
            }
            let mut challenge = authorization
                .challenge(ChallengeType::Http01)
                .context("the authority asks nothing that port 80 can answer")?;
            let token = challenge.token.clone();
            let answer = challenge.key_authorization();
            let answering = state.tunnel.answer(name, &token, answer.as_str()).await?;
            tokens.push(token.clone());
            lock(&state.panel.said).answers =
                (answering.by == AnsweredBy::WebServer).then(|| answering.directory.clone());
            let seen = served(name, &token, answer.as_str(), &answering).await?;
            shut |= !seen && answering.by == AnsweredBy::Gate;
            challenge.set_ready().await?;
        }
    }
    let refused = match order.poll_ready(&PATIENT).await {
        Ok(OrderStatus::Ready) => return Ok(()),
        Ok(_) => refusal(order).await,
        // The same refusal, where the authority gives its reason with the order.
        Err(instant_acme::Error::Api(problem)) => match &problem.detail {
            Some(detail) => detail.clone(),
            None => problem.to_string(),
        },
        Err(other) => return Err(other.into()),
    };
    // The likeliest reason, where neither the authority nor home got an answer
    // from a port that the Gate does serve: a firewall of the VPS's own.
    let advice = match shut {
        true => {
            " Home did not get through to port 80 of the VPS either. If the VPS has a firewall of its own, open the port in it: `ufw allow 80/tcp`, or `firewall-cmd --permanent --add-service=http` and `firewall-cmd --reload`."
        }
        false => "",
    };
    // The authority ends its sentence as it pleases, and this one ends with a full stop.
    bail!(
        "{} was not satisfied that this is {name}: {}.{advice}",
        state.authority.name(),
        refused.trim_end().trim_end_matches('.')
    )
}

/// Why the authority was not satisfied, in its own words.
async fn refusal(order: &mut Order) -> String {
    let mut authorizations = order.authorizations();
    while let Some(Ok(authorization)) = authorizations.next().await {
        let said = authorization
            .challenges
            .iter()
            .find_map(|challenge| challenge.error.as_ref()?.detail.clone());
        if let Some(said) = said {
            return said;
        }
    }
    "it did not say why".to_owned()
}

/// Asks the VPS's port 80 for the answer as the authority will, before the
/// authority is told to: it counts the times it was sent to ask in vain.
///
/// Only an answer that is the wrong one stops the asking. A home that cannot
/// look the name up, or cannot reach its own VPS that way, says nothing of
/// what the authority will find: that is `false`, and the asking goes on.
async fn served(
    name: &str,
    token: &str,
    answer: &str,
    answering: &Answering,
) -> anyhow::Result<bool> {
    let asked = async {
        let mut stream = TcpStream::connect((name, 80)).await?;
        let request = format!(
            "GET /.well-known/acme-challenge/{token} HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await?;
        let mut reply = Vec::new();
        stream.take(1 << 16).read_to_end(&mut reply).await?;
        std::io::Result::Ok(reply)
    };
    let Ok(Ok(reply)) = timeout(ASKING, asked).await else {
        return Ok(false);
    };
    let reply = String::from_utf8_lossy(&reply);
    if reply.contains(answer) {
        return Ok(true);
    }
    let status = reply
        .split_whitespace()
        .nth(1)
        .unwrap_or("with nothing that is HTTP");
    match answering.by {
        AnsweredBy::WebServer => bail!(
            "The VPS has a web server on port 80, and it does not give the answer to the authority's question: it answered {status}. Have it serve /.well-known/acme-challenge/ for {name} from {}.",
            answering.directory
        ),
        AnsweredBy::Gate => bail!(
            "Port 80 of {name} does not give the answer the Gate serves there: it answered {status}. The name may lead somewhere other than the VPS."
        ),
    }
}

/// Where the panel stands with the internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum PanelState {
    /// It has no name. It is reached on the home network only.
    Off,
    /// It has a name and no certificate yet, and none has been asked for.
    Waiting,
    /// A certificate is being asked for just now.
    Asking,
    /// It has a certificate for its name, and is served with it.
    On,
    /// It has a name, and the certificate could not be had. `problem` says why.
    Failed,
}

/// The certificate the panel is served with.
#[derive(Serialize, ToSchema)]
struct CertificateView {
    name: String,
    /// When it was issued, when it ends and when a newer one is asked for, in
    /// Unix seconds.
    issued_at: i64,
    expires_at: i64,
    renews_at: i64,
    /// Who issued it: the machine of the authority's directory.
    authority: String,
}

/// The panel's way in from the internet.
#[derive(Serialize, ToSchema)]
struct PanelView {
    /// Whether this Homewarp can be reached by a name at all: whether it was
    /// started with a door for TLS.
    available: bool,
    /// The port the panel is reached on by its name, on the VPS and at home alike.
    port: Option<u16>,
    /// The name it has been given.
    name: Option<String>,
    /// Where the panel is, once it has a certificate for its name.
    address: Option<String>,
    state: PanelState,
    /// Why the last asking for a certificate failed, if it did.
    problem: Option<String>,
    certificate: Option<CertificateView>,
    /// Who certificates are asked of.
    authority: String,
    /// A DNS record for the name, of type CAA, that has the authority give a
    /// certificate for it to this Homewarp and to nobody else: not to
    /// whoever has the VPS, which is where the name leads. Nothing where the
    /// authority is not known to honour such a record.
    caa: Option<String>,
    /// The directory a web server on the VPS has to serve at
    /// `/.well-known/acme-challenge/` for the name, where the VPS has one on
    /// port 80. Nothing where the Gate serves the answers itself.
    answers: Option<String>,
}

/// What the owner sets.
#[derive(Deserialize, ToSchema)]
struct PanelChange {
    /// The name the panel is to be reached by, which has to lead to the VPS
    /// already. Nothing takes the panel off the internet again.
    name: Option<String>,
    /// That the owner agrees to the terms of the authority certificates are
    /// asked of. Asked for with a name.
    #[serde(default)]
    agreed: bool,
}

async fn view(state: &AppState) -> Result<PanelView, Problem> {
    let (name, certificate, account) = tokio::try_join!(
        name(&state.db),
        kept::<Certificate>(&state.db, CERTIFICATE),
        kept::<KeptAccount>(&state.db, ACCOUNT)
    )?;
    let authority = &state.authority;
    let said = lock(&state.panel.said).clone();
    let now = auth::now();
    let certificate = certificate
        .filter(|kept| name.as_ref() == Some(&kept.name) && kept.authority == authority.directory);
    let good = certificate.as_ref().is_some_and(|kept| kept.until() > now);
    let panel_state = match (&name, said.asking, good, &said.problem) {
        (None, ..) => PanelState::Off,
        (_, true, ..) => PanelState::Asking,
        (_, _, true, _) => PanelState::On,
        (_, _, _, Some(_)) => PanelState::Failed,
        _ => PanelState::Waiting,
    };
    // Let's Encrypt honours a record that names an account. Of another
    // authority that is not known, and what it calls itself there is not either.
    let caa = account
        .filter(|kept| {
            kept.directory == authority.directory && authority.name().ends_with("letsencrypt.org")
        })
        .map(|kept| {
            format!(
                "0 issue \"letsencrypt.org; accounturi={}; validationmethods=http-01\"",
                kept.id
            )
        });
    Ok(PanelView {
        available: state.tls_port.is_some(),
        port: state.tls_port,
        address: name.as_ref().zip(state.tls_port).filter(|_| good).map(
            |(name, port)| match port {
                443 => format!("https://{name}"),
                port => format!("https://{name}:{port}"),
            },
        ),
        state: panel_state,
        problem: said.problem.map(|(problem, _)| problem),
        certificate: certificate.map(|kept| CertificateView {
            issued_at: kept.issued_at,
            expires_at: kept.until(),
            renews_at: kept.renew_at(),
            authority: host(&kept.authority).to_owned(),
            name: kept.name,
        }),
        authority: authority.name().to_owned(),
        caa: name.as_ref().and(caa),
        answers: name.as_ref().and(said.answers),
        name,
    })
}

/// The panel's way in from the internet: its name, its certificate, and how
/// the asking for one stands.
#[utoipa::path(
    get,
    path = "/api/v1/panel",
    responses(
        (status = OK, body = PanelView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn get_panel(State(state): State<AppState>, _: Owner) -> Result<Json<PanelView>, Problem> {
    Ok(Json(view(&state).await?))
}

/// Gives the panel a name, or takes its name away. With a name, a connected
/// VPS forwards the panel's port home, and a certificate for the name is
/// asked for at once and kept renewed; asking for the panel again shows how
/// that stands. Without one the port is forwarded no more.
#[utoipa::path(
    put,
    path = "/api/v1/panel",
    request_body = PanelChange,
    responses(
        (status = OK, body = PanelView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "This Homewarp has no door for TLS."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That is not a name, or the authority's terms were not agreed to."),
    )
)]
async fn change_panel(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<PanelChange>,
) -> Result<Json<PanelView>, Problem> {
    let asked_name = asked
        .name
        .map(|name| name.trim().trim_end_matches('.').to_lowercase())
        .filter(|name| !name.is_empty());
    match &asked_name {
        Some(name) => {
            if state.tls_port.is_none() {
                return Err(Problem::Conflict(
                    "This Homewarp was started without a door for TLS, so it cannot be reached by a name.".into(),
                ));
            }
            if !is_name(name) {
                return Err(Problem::Invalid(
                    "A name is like panel.example.com: letters, digits and hyphens, with dots between.".into(),
                ));
            }
            if !asked.agreed {
                return Err(Problem::Invalid(
                    format!(
                        "A certificate is asked of {}, whose terms have to be agreed to first.",
                        state.authority.name()
                    )
                    .into(),
                ));
            }
            keep(&state.db, NAME, name).await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key IN ('panel_name', 'panel_certificate')")
                .execute(&state.db)
                .await?;
            state.shown.hide();
        }
    }
    {
        let mut said = lock(&state.panel.said);
        *said = Said {
            now: true,
            // Said at once, for the page that asks next: it is about to be.
            asking: asked_name.is_some(),
            ..Said::default()
        };
    }
    // The Gate is told to forward the panel's port, or to stop; and the
    // certificate is seen to.
    state.tunnel.wake();
    state.panel.wake.notify_one();
    let detail = asked_name.as_deref().unwrap_or("no name");
    audit::record(&state.db, &who, None, "panel.name", detail).await;
    Ok(Json(view(&state).await?))
}

/// Asks for the certificate again now, without waiting out the hour that
/// follows an asking that failed.
#[utoipa::path(
    post,
    path = "/api/v1/panel/certificate",
    responses(
        (status = ACCEPTED, body = PanelView, description = "It is being asked for."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "The panel has no name."),
    )
)]
async fn ask_again(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<(StatusCode, Json<PanelView>), Problem> {
    if name(&state.db).await?.is_none() {
        return Err(Problem::Conflict(
            "The panel has no name to ask a certificate for.".into(),
        ));
    }
    {
        let mut said = lock(&state.panel.said);
        said.now = true;
        said.asking = true;
        said.problem = None;
    }
    state.panel.wake.notify_one();
    audit::record(&state.db, &who, None, "panel.ask", "").await;
    Ok((StatusCode::ACCEPTED, Json(view(&state).await?)))
}

#[cfg(test)]
mod tests {
    use super::{Certificate, host, is_name};

    #[test]
    fn a_name_is_labels_with_dots_between_and_not_an_address() {
        for name in ["panel.example.com", "homewarp.apixels.net", "a-b.c0.io"] {
            assert!(is_name(name), "{name}");
        }
        let long = format!("{}.example.com", "a".repeat(64));
        for odd in [
            "",
            "localhost",
            "192.168.1.250",
            "Panel.example.com",
            "panel..example.com",
            "-panel.example.com",
            "panel-.example.com",
            "panel.example.com/",
            "panel.example.com:8443",
            "pan el.example.com",
            "*.example.com",
            long.as_str(),
        ] {
            assert!(!is_name(odd), "{odd:?}");
        }
    }

    #[test]
    fn an_authority_is_called_by_the_machine_its_directory_is_on() {
        assert_eq!(
            host("https://acme-v02.api.letsencrypt.org/directory"),
            "acme-v02.api.letsencrypt.org"
        );
        assert_eq!(host("https://pebble:14000/dir"), "pebble");
    }

    #[test]
    fn a_certificate_is_renewed_with_a_third_of_its_time_left() {
        let day = 86_400;
        let mut certificate = Certificate {
            name: "panel.example.com".to_owned(),
            chain: String::new(),
            key: String::new(),
            issued_at: 1_000 * day,
            not_before: Some(1_000 * day),
            not_after: Some(1_090 * day),
            authority: String::new(),
        };
        // Ninety days: a month before the end.
        assert_eq!(certificate.renew_at(), 1_060 * day);
        // Six days: with two to go.
        certificate.not_after = Some(1_006 * day);
        assert_eq!(certificate.renew_at(), 1_004 * day);
        // Dates that could not be read: taken as sixty days from when it was issued.
        (certificate.not_before, certificate.not_after) = (None, None);
        assert_eq!(certificate.until(), 1_060 * day);
        assert_eq!(certificate.renew_at(), 1_040 * day);
    }
}
