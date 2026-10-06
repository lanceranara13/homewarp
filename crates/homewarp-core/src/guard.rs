//! The guard on the VPS itself, as the panel asks for it (PLAN.md §6):
//! "harden this VPS".
//!
//! The guard is the Gate's own (`homewarp-gate`, `guard`): what arrives at the
//! VPS itself from the internet is dropped, but for what was listening there
//! when its owner asked. Core passes the asking on through the tunnel and says
//! what came of it. It is put in place on trial, and stays only if the owner
//! says within a minute that it is to: by then they have seen that they can
//! still get in.

use axum::{Json, extract::State};
use homewarp_proto::{Guard, GuardState, Open};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody},
    audit,
};

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_guard, harden, unharden))
        .routes(routes!(keep_guard))
}

/// Where the guard stands.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum VpsGuardState {
    /// There is none. What the VPS lets in is the VPS's own affair.
    Off,
    /// In place for a minute, and undone by itself unless it is kept.
    Trial,
    /// In place, and written down on the VPS to stay.
    Kept,
}

/// Ports, by the two kinds there are.
#[derive(Serialize, ToSchema)]
struct Ports {
    tcp: Vec<u16>,
    udp: Vec<u16>,
}

impl From<Open> for Ports {
    fn from(open: Open) -> Self {
        Self {
            tcp: open.tcp,
            udp: open.udp,
        }
    }
}

/// The guard on the VPS itself.
#[derive(Serialize, ToSchema)]
struct VpsGuard {
    state: VpsGuardState,
    /// What it keeps open on the VPS itself. Nothing where there is no guard.
    open: Ports,
    /// On trial: how many seconds are left before it is undone by itself.
    seconds_left: Option<u64>,
    /// What is listening on the VPS now, where the internet can reach it.
    /// This is what hardening would keep open.
    listening: Ports,
    /// What is listening now and is shut all the same: it began to listen
    /// after the guard was put in place.
    shut: Ports,
    /// How many packets the guard has dropped since it was put in place.
    dropped: u64,
}

fn view(guard: Guard) -> VpsGuard {
    let guarded = guard.state != GuardState::Off;
    let shut = |listening: &[u16], open: &[u16]| -> Vec<u16> {
        listening
            .iter()
            .copied()
            .filter(|port| guarded && !open.contains(port))
            .collect()
    };
    VpsGuard {
        shut: Ports {
            tcp: shut(&guard.listening.tcp, &guard.open.tcp),
            udp: shut(&guard.listening.udp, &guard.open.udp),
        },
        state: match guard.state {
            GuardState::Off => VpsGuardState::Off,
            GuardState::Trial => VpsGuardState::Trial,
            GuardState::Kept => VpsGuardState::Kept,
        },
        open: guard.open.into(),
        seconds_left: guard.seconds_left,
        listening: guard.listening.into(),
        dropped: guard.dropped,
    }
}

/// Asks the Gate, and says in words why it could not be asked.
async fn asked(
    state: &AppState,
    method: &str,
    path: &str,
    open: Option<&Open>,
) -> Result<Guard, Problem> {
    state
        .tunnel
        .guard(method, path, open)
        .await
        .map_err(|error| Problem::Conflict(format!("{error:#}.").into()))
}

/// Ports as a line of the activity log says them.
fn said(open: &Open) -> String {
    let list = |ports: &[u16]| {
        let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
        ports.join(", ")
    };
    format!("TCP {}; UDP {}", list(&open.tcp), list(&open.udp))
}

/// The guard on the VPS itself, and what is listening there.
#[utoipa::path(
    get,
    path = "/api/v1/gate/guard",
    responses(
        (status = OK, body = VpsGuard),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "No VPS is connected, or its Gate cannot be reached."),
    )
)]
async fn get_guard(State(state): State<AppState>, _: Owner) -> Result<Json<VpsGuard>, Problem> {
    Ok(Json(view(asked(&state, "GET", "/v1/guard", None).await?)))
}

/// Hardens the VPS: from now on, what arrives at the VPS itself from the
/// internet is dropped, but for what is listening there at this moment, and
/// new SSH connections from one address are held to a few a minute. What is
/// forwarded to servers is not touched.
///
/// It is on trial. In a minute the VPS undoes it by itself, unless it has been
/// kept by then.
#[utoipa::path(
    put,
    path = "/api/v1/gate/guard",
    responses(
        (status = OK, body = VpsGuard, description = "In place, on trial."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "No VPS is connected, or its Gate cannot be reached or would not."),
    )
)]
async fn harden(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<Json<VpsGuard>, Problem> {
    // What is open is what is listening, as the Gate sees it now: the same
    // list the owner was shown, give or take the seconds since.
    let listening = asked(&state, "GET", "/v1/guard", None).await?.listening;
    let guard = asked(&state, "PUT", "/v1/guard", Some(&listening)).await?;
    audit::record(&state.db, &who, None, "gate.harden", &said(&guard.open)).await;
    Ok(Json(view(guard)))
}

/// Keeps a guard that is on trial. The VPS writes it down, and comes back
/// with it after a restart.
#[utoipa::path(
    post,
    path = "/api/v1/gate/guard/keep",
    responses(
        (status = OK, body = VpsGuard, description = "It stays."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "There is no guard to keep: its minute was over. Or the Gate cannot be reached."),
    )
)]
async fn keep_guard(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<Json<VpsGuard>, Problem> {
    let guard = asked(&state, "POST", "/v1/guard/keep", None).await?;
    audit::record(
        &state.db,
        &who,
        None,
        "gate.harden_keep",
        &said(&guard.open),
    )
    .await;
    Ok(Json(view(guard)))
}

/// Takes the guard away, kept or on trial.
#[utoipa::path(
    delete,
    path = "/api/v1/gate/guard",
    responses(
        (status = OK, body = VpsGuard, description = "There is no guard now."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "No VPS is connected, or its Gate cannot be reached."),
    )
)]
async fn unharden(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<Json<VpsGuard>, Problem> {
    let guard = asked(&state, "DELETE", "/v1/guard", None).await?;
    audit::record(&state.db, &who, None, "gate.unharden", "").await;
    Ok(Json(view(guard)))
}

#[cfg(test)]
mod tests {
    use homewarp_proto::{Guard, GuardState, Open};

    use super::view;

    #[test]
    fn what_began_to_listen_after_the_guard_is_said_to_be_shut() {
        let guard = |state| Guard {
            state,
            open: Open {
                tcp: vec![22, 80],
                udp: vec![51820],
            },
            seconds_left: None,
            listening: Open {
                tcp: vec![22, 80, 8080],
                udp: vec![51820, 9987],
            },
            dropped: 3,
        };
        let kept = view(guard(GuardState::Kept));
        assert_eq!((kept.shut.tcp, kept.shut.udp), (vec![8080], vec![9987]));
        // With no guard, nothing is shut by one.
        let off = view(guard(GuardState::Off));
        assert!(off.shut.tcp.is_empty() && off.shut.udp.is_empty());
    }
}
