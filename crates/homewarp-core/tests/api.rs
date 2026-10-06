//! The API as a browser meets it: requests in, responses out, a real database
//! file underneath.

use std::{os::unix::fs::symlink, path::PathBuf, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{
        HeaderMap, Request, StatusCode,
        header::{
            CONTENT_DISPOSITION, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
};
use homewarp_core::{AppState, Authority, app, open, openapi};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery";

/// A small egg in the shape Pelican exports.
const EGG: &str = r#"
meta:
  version: PLCN_v3
name: ' Example '
description: 'An example server.'
docker_images:
  'Java 21': 'example.invalid/java:21'
  'Java 17': 'example.invalid/java:17'
startup_commands:
  Default: 'java -jar {{SERVER_JARFILE}}'
config:
  files:
    server.properties:
      parser: properties
      find:
        server-port: '{{server.allocations.default.port}}'
  startup:
    done: 'Done'
  stop: stop
scripts:
  installation:
    script: 'echo hi'
    container: 'example.invalid/installer:alpine'
    entrypoint: ash
variables:
  -
    name: 'Server Jar File'
    description: 'The jar to run.'
    env_variable: SERVER_JARFILE
    default_value: server.jar
    user_viewable: true
    user_editable: true
    rules:
      - required
"#;

struct Panel {
    app: Router,
    setup_code: String,
    db: SqlitePool,
    /// The directory the panel keeps everything in.
    files: tempfile::TempDir,
}

struct Answer {
    status: StatusCode,
    /// The `Set-Cookie` header, whole.
    set_cookie: Option<String>,
    body: Value,
}

impl Answer {
    /// What a browser would send back after this answer.
    fn cookie(&self) -> String {
        let set = self
            .set_cookie
            .as_deref()
            .expect("the answer sets a cookie");
        set.split(';').next().unwrap().to_owned()
    }
}

async fn panel() -> Panel {
    panel_at(None).await
}

/// A panel that is served over TLS on this port as well, as one behind a door
/// for it is. Nothing here listens, and no certificate is asked for.
async fn panel_at(tls: Option<u16>) -> Panel {
    let files = tempfile::tempdir().unwrap();
    let db = open(&files.path().join("homewarp.db")).await.unwrap();
    // No Docker here: what servers do on it is tried on the homelab.
    let state = AppState::start(db.clone(), files.path(), None)
        .await
        .unwrap()
        .tls_at(tls, Authority::default());
    let setup_code = state
        .setup_code()
        .expect("a new database has no account")
        .to_owned();
    Panel {
        app: app(state),
        setup_code,
        db,
        files,
    }
}

impl Panel {
    async fn send(&self, request: Request<Body>) -> Answer {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let set_cookie = response
            .headers()
            .get(SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_owned());
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Answer {
            status,
            set_cookie,
            body,
        }
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Answer {
        let request = Request::get(path).header(COOKIE, cookie.unwrap_or_default());
        self.send(request.body(Body::empty()).unwrap()).await
    }

    async fn post(&self, path: &str, body: Value, cookie: Option<&str>) -> Answer {
        let request = Request::post(path)
            .header(CONTENT_TYPE, "application/json")
            .header(COOKIE, cookie.unwrap_or_default());
        self.send(request.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn put(&self, path: &str, body: Value, cookie: Option<&str>) -> Answer {
        let request = Request::put(path)
            .header(CONTENT_TYPE, "application/json")
            .header(COOKIE, cookie.unwrap_or_default());
        self.send(request.body(Body::from(body.to_string())).unwrap())
            .await
    }

    async fn delete(&self, path: &str, cookie: Option<&str>) -> Answer {
        let request = Request::delete(path).header(COOKIE, cookie.unwrap_or_default());
        self.send(request.body(Body::empty()).unwrap()).await
    }

    /// Sends a file's bytes as an upload does: as the body, as they are.
    async fn upload(&self, path: &str, bytes: &[u8], cookie: Option<&str>) -> Answer {
        let request = Request::put(path).header(COOKIE, cookie.unwrap_or_default());
        self.send(request.body(Body::from(bytes.to_vec())).unwrap())
            .await
    }

    /// Asks for what is not JSON, and returns it with its headers.
    async fn download(&self, path: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let request = Request::get(path).header(COOKIE, cookie.unwrap_or_default());
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, headers, bytes.to_vec())
    }

    /// Puts a server in the database as one made and installed earlier, and
    /// returns where its files are kept. There is no Docker here to make one with.
    async fn a_server(&self) -> PathBuf {
        sqlx::query(
            "INSERT INTO templates (name, definition, source, created_at)
             VALUES ('Example', '{}', '', 0)",
        )
        .execute(&self.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO servers
                 (uuid, name, template_id, image, memory_mb, cpu_percent, port, variables, eula,
                  installed, created_at)
             VALUES ('a-server', 'Survival', 1, 'example.invalid/java:21', 1024, 0, 25565, '[]',
                     0, 1, 0)",
        )
        .execute(&self.db)
        .await
        .unwrap();
        self.files.path().join("servers/a-server")
    }

    /// Finishes setup as `lance` and returns the session cookie.
    async fn set_up(&self) -> String {
        let request = json!({ "code": self.setup_code, "username": "lance", "password": PASSWORD });
        let answer = self.post("/api/v1/setup", request, None).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        answer.cookie()
    }
}

#[tokio::test]
async fn a_new_panel_asks_for_setup() {
    let panel = panel().await;
    let answer = panel.get("/api/v1/session", None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body["setup_required"], true);
    assert_eq!(answer.body["user"], Value::Null);
}

#[tokio::test]
async fn setup_wants_the_code_and_a_sound_account() {
    let panel = panel().await;
    let attempt = |code: &str, username: &str, password: &str| {
        panel.post(
            "/api/v1/setup",
            json!({ "code": code, "username": username, "password": password }),
            None,
        )
    };

    assert_eq!(
        attempt("AAAA-AAAA-AAAA", "lance", PASSWORD).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        attempt(&panel.setup_code, "la nce", PASSWORD).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let short = attempt(&panel.setup_code, "lance", "too short").await;
    assert_eq!(short.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(short.body["error"].as_str().unwrap().contains("10 to 256"));
    assert_eq!(short.set_cookie, None);

    assert_eq!(
        panel.get("/api/v1/session", None).await.body["setup_required"],
        true
    );
}

#[tokio::test]
async fn setup_creates_the_account_once_and_signs_it_in() {
    let panel = panel().await;
    let request = json!({ "code": panel.setup_code.to_lowercase(), "username": " lance ", "password": PASSWORD });
    let created = panel.post("/api/v1/setup", request.clone(), None).await;
    assert_eq!(created.status, StatusCode::OK);
    assert_eq!(created.body["user"]["username"], "lance");

    let set_cookie = created.set_cookie.as_deref().unwrap();
    for attribute in ["HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(
            set_cookie.contains(attribute),
            "{attribute} in {set_cookie}"
        );
    }

    let session = panel
        .get("/api/v1/session", Some(&created.cookie()))
        .await
        .body;
    assert_eq!(session["setup_required"], false);
    assert_eq!(session["user"]["username"], "lance");

    assert_eq!(
        panel.post("/api/v1/setup", request, None).await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn signing_out_ends_the_session() {
    let panel = panel().await;
    let cookie = panel.set_up().await;

    let out = panel
        .post("/api/v1/logout", Value::Null, Some(&cookie))
        .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert!(out.set_cookie.unwrap().contains("Max-Age=0"));

    // The cookie a thief copied earlier is as dead as the one the browser dropped.
    assert_eq!(
        panel.get("/api/v1/session", Some(&cookie)).await.body["user"],
        Value::Null
    );
}

#[tokio::test]
async fn signing_in_takes_the_right_password_and_nothing_else() {
    let panel = panel().await;
    panel.set_up().await;
    let attempt = |username: &str, password: &str| {
        panel.post(
            "/api/v1/login",
            json!({ "username": username, "password": password }),
            None,
        )
    };

    let wrong = attempt("lance", "correct horse batterz").await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.set_cookie, None);
    let nobody = attempt("nobody", PASSWORD).await;
    assert_eq!(
        (nobody.status, &nobody.body),
        (wrong.status, &wrong.body),
        "an unknown name reads like a wrong password"
    );

    let signed_in = attempt("Lance", PASSWORD).await;
    assert_eq!(signed_in.status, StatusCode::OK);
    let session = panel
        .get("/api/v1/session", Some(&signed_in.cookie()))
        .await
        .body;
    assert_eq!(session["user"]["username"], "lance");
}

#[tokio::test]
async fn a_request_made_by_another_site_is_refused() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let logout = |origin: &'static str| {
        let request = Request::post("/api/v1/logout")
            .header(HOST, "192.168.1.250:3600")
            .header(ORIGIN, origin)
            .header(COOKIE, &cookie);
        panel.send(request.body(Body::empty()).unwrap())
    };

    assert_eq!(
        logout("http://evil.example").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        panel.get("/api/v1/session", Some(&cookie)).await.body["user"]["username"],
        "lance"
    );
    assert_eq!(
        logout("http://192.168.1.250:3600").await.status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn paths_outside_the_api_belong_to_the_web_interface() {
    let panel = panel().await;
    let missing = panel.get("/api/v1/nothing-here", None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body["error"], "There is no such endpoint.");

    // The page itself is there only in a build that includes the web interface.
    let page = panel.get("/servers/anything", None).await.status;
    assert!(
        page == StatusCode::OK || page == StatusCode::SERVICE_UNAVAILABLE,
        "{page}"
    );
}

#[tokio::test]
async fn templates_are_for_someone_signed_in() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let import = json!({ "egg": EGG });
    let created = panel
        .post("/api/v1/templates", import.clone(), Some(&cookie))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);

    for answer in [
        panel.get("/api/v1/templates", None).await,
        panel.get("/api/v1/templates/1", None).await,
        panel.post("/api/v1/templates", import, None).await,
        panel.delete("/api/v1/templates/1", None).await,
    ] {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.body["error"], "Sign in first.");
    }
    let still_there = panel.get("/api/v1/templates", Some(&cookie)).await;
    assert_eq!(still_there.body.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_egg_becomes_a_template_once() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let empty = panel.get("/api/v1/templates", Some(&cookie)).await;
    assert_eq!((empty.status, &empty.body), (StatusCode::OK, &json!([])));

    let import = json!({ "egg": EGG });
    let created = panel
        .post("/api/v1/templates", import.clone(), Some(&cookie))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let template = &created.body;
    assert_eq!(template["name"], "Example");
    assert_eq!(template["images"][1]["image"], "example.invalid/java:17");
    assert_eq!(template["startup"], "java -jar {{SERVER_JARFILE}}");
    assert_eq!(template["done"], json!(["Done"]));
    assert_eq!(
        template["stop"],
        json!({ "by": "command", "value": "stop" })
    );
    assert_eq!(template["config_files"], json!(["server.properties"]));
    assert_eq!(template["install"]["entrypoint"], "ash");
    assert_eq!(template["variables"][0]["env"], "SERVER_JARFILE");
    assert_eq!(template["variables"][0]["rules"], json!(["required"]));

    let id = template["id"].as_i64().unwrap();
    let listed = panel.get("/api/v1/templates", Some(&cookie)).await.body;
    assert_eq!(
        listed,
        json!([{
            "id": id,
            "name": "Example",
            "description": "An example server.",
            "image": "example.invalid/java:21",
        }])
    );
    let fetched = panel
        .get(&format!("/api/v1/templates/{id}"), Some(&cookie))
        .await;
    assert_eq!((fetched.status, &fetched.body), (StatusCode::OK, template));

    // The name is the same whatever its case.
    let again = json!({ "egg": EGG.replace("' Example '", "EXAMPLE") });
    let twice = panel.post("/api/v1/templates", again, Some(&cookie)).await;
    assert_eq!(twice.status, StatusCode::CONFLICT);
    assert!(twice.body["error"].as_str().unwrap().contains("EXAMPLE"));
}

#[tokio::test]
async fn a_removed_template_is_gone() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let created = panel
        .post("/api/v1/templates", json!({ "egg": EGG }), Some(&cookie))
        .await;
    let path = format!("/api/v1/templates/{}", created.body["id"]);

    assert_eq!(
        panel.delete(&path, Some(&cookie)).await.status,
        StatusCode::NO_CONTENT
    );
    for answer in [
        panel.get(&path, Some(&cookie)).await,
        panel.delete(&path, Some(&cookie)).await,
    ] {
        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body["error"], "There is no such template.");
    }
    assert_eq!(
        panel.get("/api/v1/templates", Some(&cookie)).await.body,
        json!([])
    );
}

#[tokio::test]
async fn what_is_not_an_egg_is_refused_in_words() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let attempt =
        |egg: String| panel.post("/api/v1/templates", json!({ "egg": egg }), Some(&cookie));

    for (egg, reason) in [
        ("hello".to_owned(), "`meta.version` is missing"),
        (EGG.replace("PLCN_v3", "PLCN_v9"), "unsupported egg format"),
        (EGG.replace("' Example '", "'  '"), "1 to 100 characters"),
        (format!("{EGG}#{}", "x".repeat(1 << 20)), "at most 1 MB"),
    ] {
        let refused = attempt(egg).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
        let said = refused.body["error"].as_str().unwrap();
        assert!(said.contains(reason), "{reason} in {said}");
    }
    assert_eq!(
        panel.get("/api/v1/templates", Some(&cookie)).await.body,
        json!([])
    );
}

#[tokio::test]
async fn a_server_is_asked_for_in_full_before_anything_is_made() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let created = panel
        .post("/api/v1/templates", json!({ "egg": EGG }), Some(&cookie))
        .await;
    let template = created.body["id"].as_i64().unwrap();
    let ask = |change: Value| {
        let mut server = json!({ "name": "Survival", "template_id": template, "memory_mb": 1024, "port": 25565 });
        for (key, value) in change.as_object().unwrap() {
            server[key] = value.clone();
        }
        panel.post("/api/v1/servers", server, Some(&cookie))
    };

    for (change, reason) in [
        (json!({ "name": "  " }), "1 to 60 characters"),
        (json!({ "memory_mb": 64 }), "128 MB"),
        (json!({ "port": 80 }), "1024 to 65535"),
        (
            json!({ "ports": [{ "port": 53, "protocol": "udp" }] }),
            "1024 to 65535",
        ),
        (
            json!({ "ports": [{ "port": 24454, "protocol": "udp" }, { "port": 25565 }] }),
            "Port 25565 is given twice.",
        ),
        (json!({ "template_id": template + 1 }), "no such template"),
        (
            json!({ "image": "example.invalid/other:1" }),
            "no such image",
        ),
        (
            json!({ "variables": { "SERVER_JARFILE": "" } }),
            "Server Jar File is required.",
        ),
        (
            json!({ "variables": { "NOT_ONE": "1" } }),
            "no variable called NOT_ONE",
        ),
    ] {
        let refused = ask(change).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{reason}");
        let said = refused.body["error"].as_str().unwrap();
        assert!(said.contains(reason), "{reason} in {said}");
    }

    // All of it in order, and no Docker to make it with: said, and nothing kept.
    let unmade = ask(json!({ "image": "example.invalid/java:17" })).await;
    assert_eq!(unmade.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(unmade.body["error"].as_str().unwrap().contains("Docker"));
    let listed = panel.get("/api/v1/servers", Some(&cookie)).await;
    assert_eq!((listed.status, &listed.body), (StatusCode::OK, &json!([])));
}

#[tokio::test]
async fn what_is_typed_into_a_console_is_one_line() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let type_in = |command: &str| {
        panel.post(
            "/api/v1/servers/1/command",
            json!({ "command": command }),
            Some(&cookie),
        )
    };

    // A second command must not ride in behind the first.
    for not_a_line in ["say hi\nstop", "", "\n", &"x".repeat(1001)] {
        let refused = type_in(not_a_line).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(refused.body["error"].as_str().unwrap().contains("one line"));
    }
    // A line as a terminal sends it, with its line ending. No Docker to type it into here.
    assert_eq!(
        type_in("say hi\r\n").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let nobody = panel
        .post(
            "/api/v1/servers/1/command",
            json!({ "command": "stop" }),
            None,
        )
        .await;
    assert_eq!(nobody.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_console_is_followed_only_from_this_site_and_signed_in() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let follow = |origin: &'static str, cookie: &str| {
        let request = Request::get("/api/v1/servers/1/console")
            .header(HOST, "192.168.1.250:3600")
            .header(ORIGIN, origin)
            .header(COOKIE, cookie);
        panel.send(request.body(Body::empty()).unwrap())
    };

    let elsewhere = follow("http://evil.example", &cookie).await;
    assert_eq!(elsewhere.status, StatusCode::FORBIDDEN);
    let nobody = follow("http://192.168.1.250:3600", "").await;
    assert_eq!(nobody.status, StatusCode::UNAUTHORIZED);
    // From here and signed in it is let through, as far as being told that an
    // ordinary request is not how a socket is opened.
    let here = follow("http://192.168.1.250:3600", &cookie).await;
    assert_eq!(here.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_vps_is_connected_with_one_command_that_counts_for_a_while() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let gate = || panel.get("/api/v1/gate", Some(&cookie));

    let none = gate().await;
    assert_eq!(none.status, StatusCode::OK);
    assert_eq!(none.body["state"], "none");
    assert_eq!(none.body["player_addresses"], "unchecked");
    assert_eq!(none.body["ports"], json!([]));

    for odd in ["", "  ", "203.0.113.10; reboot", "http://example.com"] {
        let refused = panel
            .post("/api/v1/gate", json!({ "address": odd }), Some(&cookie))
            .await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{odd}");
    }

    let asked = json!({ "address": " 203.0.113.10 ", "wg_port": 51999 });
    let waiting = panel.post("/api/v1/gate", asked, Some(&cookie)).await;
    assert_eq!(waiting.status, StatusCode::CREATED);
    assert_eq!(waiting.body["state"], "waiting");
    assert_eq!(waiting.body["address"], "203.0.113.10");
    assert_eq!(waiting.body["reachable"], false);
    // The command carries all a VPS needs, and says when it stops counting.
    let command = waiting.body["command"].as_str().unwrap();
    let token = command.strip_prefix("homewarp-gate join ").unwrap();
    let token = homewarp_proto::JoinToken::decode(token).unwrap();
    assert_eq!((token.wg_port, token.api_port), (51999, 4857));
    assert_eq!(token.gate_address.to_string(), "10.213.77.1");
    assert_eq!(token.home_address.to_string(), "10.213.77.2");
    for key in [
        &token.private_key,
        &token.home_public_key,
        &token.preshared_key,
    ] {
        assert_eq!(key.len(), 44);
    }
    let until = waiting.body["expires_at"].as_i64().unwrap();
    assert_eq!(token.expires_at, until.unsigned_abs());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!((now + 890..=now + 900).contains(&until));
    // Asked for again, it is the same command: the page can be opened anew.
    assert_eq!(gate().await.body["command"], waiting.body["command"]);
    // Asked to connect again while waiting, the command is a new one.
    let again = json!({ "address": "vps.example.com" });
    let again = panel.post("/api/v1/gate", again, Some(&cookie)).await;
    assert_eq!(again.status, StatusCode::CREATED);
    assert_eq!(again.body["address"], "vps.example.com");
    assert_ne!(again.body["command"], waiting.body["command"]);

    // Nothing has been enrolled, so there is nothing to check yet.
    let check = panel
        .post("/api/v1/gate/check", json!({}), Some(&cookie))
        .await;
    assert_eq!(check.status, StatusCode::CONFLICT);

    let gone = panel.delete("/api/v1/gate", Some(&cookie)).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert_eq!(gate().await.body["state"], "none");
}

#[tokio::test]
async fn the_gate_is_for_someone_signed_in() {
    let panel = panel().await;
    for answer in [
        panel.get("/api/v1/gate", None).await,
        panel
            .post("/api/v1/gate", json!({ "address": "203.0.113.10" }), None)
            .await,
        panel.post("/api/v1/gate/check", json!({}), None).await,
        panel.delete("/api/v1/gate", None).await,
    ] {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn servers_are_for_someone_signed_in() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let power = json!({ "action": "start" });
    for answer in [
        panel.get("/api/v1/servers", None).await,
        panel.get("/api/v1/servers/1", None).await,
        panel.post("/api/v1/servers", json!({}), None).await,
        panel.post("/api/v1/servers/1/power", power, None).await,
        panel.delete("/api/v1/servers/1", None).await,
        panel.put("/api/v1/servers/1", json!({}), None).await,
    ] {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    let settings = json!({ "name": "Survival", "memory_mb": 1024, "port": 25565 });
    let changed = panel
        .put("/api/v1/servers/1", settings, Some(&cookie))
        .await;
    assert_eq!(changed.status, StatusCode::NOT_FOUND);
    for answer in [
        panel.get("/api/v1/servers/1", Some(&cookie)).await,
        panel.delete("/api/v1/servers/1", Some(&cookie)).await,
    ] {
        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body["error"], "There is no such server.");
    }
}

#[tokio::test]
async fn a_servers_files_are_browsed_written_and_packed() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let root = panel.a_server().await;
    let files = "/api/v1/servers/1/files";
    let cookie = Some(cookie.as_str());

    let empty = panel.get(files, cookie).await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.body);
    assert_eq!(empty.body, json!([]));

    // An upload, into a folder that is not there yet.
    let motd = format!("{files}/content?path=config/motd.txt");
    let put = panel.upload(&motd, b"hello", cookie).await;
    assert_eq!(put.status, StatusCode::NO_CONTENT, "{}", put.body);
    assert_eq!(
        std::fs::read(root.join("config/motd.txt")).unwrap(),
        b"hello"
    );
    let top = panel.get(files, cookie).await;
    assert_eq!(top.body[0]["name"], "config");
    assert_eq!(top.body[0]["kind"], "folder");
    let inside = panel.get(&format!("{files}?path=/config/"), cookie).await;
    assert_eq!(inside.body[0]["name"], "motd.txt");
    assert_eq!(inside.body[0]["kind"], "file");
    assert_eq!(inside.body[0]["size"], 5);
    assert!(inside.body[0]["modified"].as_i64().unwrap() > 1_700_000_000);
    assert_eq!(inside.body.as_array().unwrap().len(), 1);

    // The editor reads text, saves the same way an upload arrives, and leaves the rest alone.
    assert_eq!(
        panel.get(&motd, cookie).await.body,
        json!({ "text": "hello" })
    );
    let saved = panel.upload(&motd, "hello, world".as_bytes(), cookie).await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT);
    assert_eq!(panel.get(&motd, cookie).await.body["text"], "hello, world");
    let blob = format!("{files}/content?path=blob%20one.bin");
    panel.upload(&blob, &[0xff, 0xfe, 0x00], cookie).await;
    let not_text = panel.get(&blob, cookie).await;
    assert_eq!(not_text.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        not_text.body["error"],
        "That file is not text. Download it instead."
    );
    for path in ["config", "nothing.txt"] {
        let answer = panel
            .get(&format!("{files}/content?path={path}"), cookie)
            .await;
        assert!(answer.status.is_client_error(), "{path}");
    }

    // A download is the file as it is, and never something a browser shows.
    let (status, headers, bytes) = panel
        .download(&format!("{files}/download?path=blob%20one.bin"), cookie)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, [0xff, 0xfe, 0x00]);
    assert_eq!(headers[CONTENT_TYPE], "application/octet-stream");
    assert_eq!(headers[X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(
        headers[CONTENT_DISPOSITION],
        "attachment; filename=\"blob one.bin\"; filename*=UTF-8''blob%20one.bin"
    );

    // Folders, moving and deleting.
    let folder = format!("{files}/folder");
    let world = json!({ "path": "world" });
    let made = panel.post(&folder, world.clone(), cookie).await;
    assert_eq!(made.status, StatusCode::NO_CONTENT, "{}", made.body);
    assert_eq!(
        panel.post(&folder, world, cookie).await.status,
        StatusCode::CONFLICT
    );
    let moving = format!("{files}/move");
    let moved = json!({ "from": "config/motd.txt", "to": "world/motd.txt" });
    assert_eq!(
        panel.post(&moving, moved, cookie).await.status,
        StatusCode::NO_CONTENT
    );
    let onto = json!({ "from": "blob one.bin", "to": "world/motd.txt" });
    let refused = panel.post(&moving, onto, cookie).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(
        refused.body["error"],
        "Something by that name is there already."
    );
    let removing = format!("{files}/remove");
    let gone = json!({ "paths": ["blob one.bin", "config"] });
    assert_eq!(
        panel.post(&removing, gone.clone(), cookie).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel.post(&removing, gone, cookie).await.status,
        StatusCode::NOT_FOUND
    );
    let top = panel.get(files, cookie).await;
    assert_eq!(top.body.as_array().unwrap().len(), 1);

    // Packed, deleted, and unpacked again.
    let packing = format!("{files}/pack");
    let nothing = json!({ "folder": "", "names": [] });
    assert_eq!(
        panel.post(&packing, nothing, cookie).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let packed = panel
        .post(
            &packing,
            json!({ "folder": "", "names": ["world"] }),
            cookie,
        )
        .await;
    assert_eq!(packed.status, StatusCode::OK, "{}", packed.body);
    let archive = packed.body["name"].as_str().unwrap().to_owned();
    assert!(archive.starts_with("archive-") && archive.ends_with(".tar.gz"));
    panel
        .post(&removing, json!({ "paths": ["world"] }), cookie)
        .await;
    assert!(!root.join("world").exists());
    let unpacking = format!("{files}/unpack");
    let unpacked = panel
        .post(&unpacking, json!({ "path": archive }), cookie)
        .await;
    assert_eq!(unpacked.status, StatusCode::OK, "{}", unpacked.body);
    assert_eq!(unpacked.body, json!({ "files": 1, "skipped": 0 }));
    assert_eq!(
        std::fs::read(root.join("world/motd.txt")).unwrap(),
        b"hello, world"
    );
    let not_one = panel
        .post(&unpacking, json!({ "path": "world/motd.txt" }), cookie)
        .await;
    assert_eq!(not_one.status, StatusCode::UNPROCESSABLE_ENTITY);

    let usage = panel.get(&format!("{files}/usage"), cookie).await;
    assert_eq!(usage.status, StatusCode::OK);
    assert!(usage.body["used_bytes"].as_u64().unwrap() > 0);
    assert!(usage.body["free_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn a_servers_files_end_at_its_own_folder() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let root = panel.a_server().await;
    let files = "/api/v1/servers/1/files";
    let cookie = Some(cookie.as_str());
    // What is beside the server's files is the machine's, and a server that
    // has been taken over can leave a link to it among them.
    let secret = panel.files.path().join("secret.txt");
    std::fs::write(&secret, "the machine's own").unwrap();
    std::fs::create_dir_all(&root).unwrap();
    symlink(panel.files.path(), root.join("out")).unwrap();

    for path in [
        "../secret.txt",
        "/../../secret.txt",
        "a/../../secret.txt",
        "%2e%2e/secret.txt",
        ".",
    ] {
        for answer in [
            panel.get(&format!("{files}?path={path}"), cookie).await,
            panel
                .get(&format!("{files}/content?path={path}"), cookie)
                .await,
            panel
                .upload(&format!("{files}/content?path={path}"), b"x", cookie)
                .await,
        ] {
            assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{path}");
        }
        let (status, _, _) = panel
            .download(&format!("{files}/download?path={path}"), cookie)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}");
    }

    for answer in [
        panel.get(&format!("{files}?path=out"), cookie).await,
        panel
            .get(&format!("{files}/content?path=out/secret.txt"), cookie)
            .await,
    ] {
        assert_eq!(answer.status, StatusCode::FORBIDDEN);
        assert_eq!(
            answer.body["error"],
            "That leads out of this server's files."
        );
    }
    let (status, _, bytes) = panel
        .download(&format!("{files}/download?path=out/secret.txt"), cookie)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&bytes).contains("the machine's own"));
    let through = [
        panel
            .upload(
                &format!("{files}/content?path=out/planted.txt"),
                b"x",
                cookie,
            )
            .await,
        panel
            .post(
                &format!("{files}/folder"),
                json!({ "path": "out/planted" }),
                cookie,
            )
            .await,
        panel
            .post(
                &format!("{files}/move"),
                json!({ "from": "out/secret.txt", "to": "taken.txt" }),
                cookie,
            )
            .await,
        panel
            .post(
                &format!("{files}/remove"),
                json!({ "paths": ["out/secret.txt"] }),
                cookie,
            )
            .await,
        panel
            .post(
                &format!("{files}/pack"),
                json!({ "folder": "out", "names": ["secret.txt"] }),
                cookie,
            )
            .await,
        panel
            .post(
                &format!("{files}/unpack"),
                json!({ "path": "out/secret.txt" }),
                cookie,
            )
            .await,
    ];
    for answer in through {
        assert!(answer.status.is_client_error(), "{}", answer.body);
    }
    // The server's folder itself is not one of its files.
    for path in ["", "/", "world/../.."] {
        let answer = panel
            .post(
                &format!("{files}/remove"),
                json!({ "paths": [path] }),
                cookie,
            )
            .await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{path:?}");
    }
    // Deleting the link deletes the link.
    let unlinked = panel
        .post(
            &format!("{files}/remove"),
            json!({ "paths": ["out"] }),
            cookie,
        )
        .await;
    assert_eq!(unlinked.status, StatusCode::NO_CONTENT);

    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "the machine's own"
    );
    assert!(root.exists());
    let beside: Vec<_> = std::fs::read_dir(panel.files.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with("homewarp.db"))
        .collect();
    assert_eq!(beside.len(), 2, "{beside:?}");
}

#[tokio::test]
async fn files_are_for_someone_signed_in_and_a_server_that_is_there() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let files = "/api/v1/servers/1/files";
    let paths = json!({ "paths": ["a"] });
    let each = |cookie: Option<&'static str>| {
        let panel = &panel;
        let paths = paths.clone();
        async move {
            vec![
                panel.get(files, cookie).await,
                panel.get(&format!("{files}/content?path=a"), cookie).await,
                panel.get(&format!("{files}/download?path=a"), cookie).await,
                panel.get(&format!("{files}/usage"), cookie).await,
                panel
                    .upload(&format!("{files}/content?path=a"), b"x", cookie)
                    .await,
                panel
                    .post(&format!("{files}/folder"), json!({ "path": "a" }), cookie)
                    .await,
                panel
                    .post(
                        &format!("{files}/move"),
                        json!({ "from": "a", "to": "b" }),
                        cookie,
                    )
                    .await,
                panel.post(&format!("{files}/remove"), paths, cookie).await,
                panel
                    .post(
                        &format!("{files}/pack"),
                        json!({ "folder": "", "names": ["a"] }),
                        cookie,
                    )
                    .await,
                panel
                    .post(&format!("{files}/unpack"), json!({ "path": "a" }), cookie)
                    .await,
            ]
        }
    };
    for answer in each(None).await {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    let cookie: &'static str = cookie.leak();
    for answer in each(Some(cookie)).await {
        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body["error"], "There is no such server.");
    }
    // Asking after a server that is not there makes nothing for it.
    assert!(!panel.files.path().join("servers").exists());
}

#[tokio::test]
async fn an_account_does_what_it_has_been_let_do_and_no_more() {
    let panel = panel().await;
    let owner = panel.set_up().await;
    panel.a_server().await;
    let owner = Some(owner.as_str());
    let password = "another long password";

    let new = json!({ "username": "sam", "password": password });
    let made = panel.post("/api/v1/users", new.clone(), owner).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    assert_eq!(made.body["owner"], false);
    let sam_id = made.body["id"].as_i64().unwrap();
    assert_eq!(
        panel.post("/api/v1/users", new.clone(), owner).await.status,
        StatusCode::CONFLICT
    );
    let weak = json!({ "username": "jo", "password": "short" });
    assert_eq!(
        panel.post("/api/v1/users", weak, owner).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let signed = panel.post("/api/v1/login", new.clone(), None).await;
    assert_eq!(signed.status, StatusCode::OK);
    assert_eq!(signed.body["user"]["owner"], false);
    let sam = signed.cookie();
    let sam = Some(sam.as_str());

    // A server it has not been let into is not there.
    assert_eq!(panel.get("/api/v1/servers", sam).await.body, json!([]));
    for path in ["/api/v1/servers/1", "/api/v1/servers/1/files"] {
        assert_eq!(panel.get(path, sam).await.status, StatusCode::NOT_FOUND);
    }
    // And what changes the machine, or who may use it, is the owner's.
    let grant = format!("/api/v1/servers/1/users/{sam_id}");
    let files = json!({ "permissions": ["files", "files"] });
    for answer in [
        panel.get("/api/v1/users", sam).await,
        panel.post("/api/v1/users", new, sam).await,
        panel.delete("/api/v1/users/1", sam).await,
        panel.get("/api/v1/activity", sam).await,
        panel
            .post("/api/v1/templates", json!({ "egg": EGG }), sam)
            .await,
        panel.delete("/api/v1/templates/1", sam).await,
        panel.post("/api/v1/servers", json!({}), sam).await,
        panel.delete("/api/v1/servers/1", sam).await,
        panel
            .post("/api/v1/gate", json!({ "address": "203.0.113.10" }), sam)
            .await,
        panel.delete("/api/v1/gate", sam).await,
        panel.get("/api/v1/servers/1/users", sam).await,
        panel.put(&grant, files.clone(), sam).await,
    ] {
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.body);
        assert_eq!(
            answer.body["error"],
            "Only the owner of this Homewarp can do that."
        );
    }

    // Let in, it may look.
    let looking = json!({ "permissions": [] });
    assert_eq!(
        panel.put(&grant, looking, owner).await.status,
        StatusCode::NO_CONTENT
    );
    let seen = panel.get("/api/v1/servers/1", sam).await;
    assert_eq!(seen.status, StatusCode::OK, "{}", seen.body);
    assert_eq!(seen.body["permissions"], json!([]));
    let listed = panel.get("/api/v1/servers", sam).await;
    assert_eq!(listed.body[0]["name"], "Survival");
    let touching = || async {
        [
            panel.get("/api/v1/servers/1/files", sam).await,
            panel
                .upload("/api/v1/servers/1/files/content?path=a", b"x", sam)
                .await,
            panel
                .post("/api/v1/servers/1/power", json!({ "action": "start" }), sam)
                .await,
            panel
                .post(
                    "/api/v1/servers/1/command",
                    json!({ "command": "say hi" }),
                    sam,
                )
                .await,
            panel
                .put(
                    "/api/v1/servers/1",
                    json!({ "name": "Mine", "memory_mb": 1024, "port": 25565 }),
                    sam,
                )
                .await,
        ]
    };
    for answer in touching().await {
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.body);
        assert_eq!(
            answer.body["error"],
            "Your account has not been let do that with this server."
        );
    }

    // Let do one thing, it may do that one.
    assert_eq!(
        panel.put(&grant, files, owner).await.status,
        StatusCode::NO_CONTENT
    );
    let seen = panel.get("/api/v1/servers/1", sam).await;
    assert_eq!(seen.body["permissions"], json!(["files"]));
    let [listing, upload, power, command, settings] = touching().await;
    assert_eq!(listing.status, StatusCode::OK);
    assert_eq!(upload.status, StatusCode::NO_CONTENT);
    for answer in [power, command, settings] {
        assert_eq!(answer.status, StatusCode::FORBIDDEN);
    }
    let users = panel.get("/api/v1/servers/1/users", owner).await;
    assert_eq!(
        users.body,
        json!([{ "user_id": sam_id, "username": "sam", "permissions": ["files"] }])
    );
    let accounts = panel.get("/api/v1/users", owner).await;
    assert_eq!(accounts.body[0]["username"], "lance");
    assert_eq!(accounts.body[0]["owner"], true);
    assert_eq!(accounts.body[1]["username"], "sam");
    assert_eq!(accounts.body[1]["servers"], 1);

    // The owner is in every server, and is not let in or turned out.
    let owner_grant = "/api/v1/servers/1/users/1";
    assert_eq!(
        panel
            .put(owner_grant, json!({ "permissions": [] }), owner)
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        panel.delete("/api/v1/users/1", owner).await.status,
        StatusCode::CONFLICT
    );

    // Turned out, the server is gone from its sight again.
    assert_eq!(
        panel.delete(&grant, owner).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel.delete(&grant, owner).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        panel.get("/api/v1/servers/1", sam).await.status,
        StatusCode::NOT_FOUND
    );

    // It changes its own password, knowing the one it has.
    let own = "/api/v1/account/password";
    let wrong = json!({ "current": "not this one at all", "password": "a third long password" });
    assert_eq!(
        panel.post(own, wrong, sam).await.status,
        StatusCode::FORBIDDEN
    );
    let right = json!({ "current": password, "password": "a third long password" });
    assert_eq!(
        panel.post(own, right, sam).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel.get("/api/v1/servers", sam).await.status,
        StatusCode::OK
    );
    let old = json!({ "username": "sam", "password": password });
    assert_eq!(
        panel.post("/api/v1/login", old, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    // The owner gives it another, and it is signed out where it was signed in.
    let reset = json!({ "password": "a fourth long password" });
    assert_eq!(
        panel
            .put(&format!("/api/v1/users/{sam_id}/password"), reset, owner)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel.get("/api/v1/servers", sam).await.status,
        StatusCode::UNAUTHORIZED
    );
    let again = json!({ "username": "sam", "password": "a fourth long password" });
    let signed = panel.post("/api/v1/login", again, None).await;
    assert_eq!(signed.status, StatusCode::OK);
    let sam = signed.cookie();

    // Removed, it is signed out for good.
    let account = format!("/api/v1/users/{sam_id}");
    assert_eq!(
        panel.delete(&account, owner).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel.delete(&account, owner).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        panel.get("/api/v1/servers", Some(&sam)).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn what_is_done_is_written_down_for_the_owner() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    panel.a_server().await;
    let cookie = Some(cookie.as_str());

    panel
        .upload("/api/v1/servers/1/files/content?path=a.txt", b"x", cookie)
        .await;
    let moved = json!({ "from": "a.txt", "to": "b.txt" });
    panel
        .post("/api/v1/servers/1/files/move", moved, cookie)
        .await;
    // What was refused did not happen, and is not written down as if it had.
    let onto = json!({ "from": "nothing.txt", "to": "b.txt" });
    panel
        .post("/api/v1/servers/1/files/move", onto, cookie)
        .await;
    let wrong = json!({ "username": "lance", "password": "not the password" });
    panel.post("/api/v1/login", wrong, None).await;
    let nobody = json!({ "username": "hunter2", "password": "not the password" });
    panel.post("/api/v1/login", nobody, None).await;

    let log = panel.get("/api/v1/activity", cookie).await;
    assert_eq!(log.status, StatusCode::OK, "{}", log.body);
    let actions = |log: &Value| -> Vec<String> {
        let lines = log.as_array().unwrap().iter();
        lines
            .map(|line| line["action"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        actions(&log.body),
        [
            "account.sign_in_failed",
            "files.move",
            "files.write",
            "account.setup"
        ]
    );
    let moved = &log.body[1];
    assert_eq!(moved["detail"], "a.txt to b.txt");
    assert_eq!(moved["username"], "lance");
    assert_eq!(moved["user_id"], 1);
    assert_eq!(moved["server"], "Survival");
    assert_eq!(moved["server_id"], 1);
    assert!(moved["at"].as_i64().unwrap() > 1_700_000_000);
    assert_eq!(log.body[3]["server"], Value::Null);

    let of_server = panel.get("/api/v1/activity?server=1", cookie).await;
    assert_eq!(actions(&of_server.body), ["files.move", "files.write"]);
    let older = format!("/api/v1/activity?before={}", moved["id"]);
    let older = panel.get(&older, cookie).await;
    assert_eq!(actions(&older.body), ["files.write", "account.setup"]);
    let nobodys = panel.get("/api/v1/activity?user=99", cookie).await;
    assert_eq!(nobodys.body, json!([]));
    assert_eq!(
        panel.get("/api/v1/activity", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

impl Panel {
    /// Asks again until the answer is as wanted. For what is answered at once
    /// and done afterwards: a backup, a schedule's run.
    async fn until(
        &self,
        path: &str,
        cookie: Option<&str>,
        wanted: impl Fn(&Value) -> bool,
    ) -> Value {
        for _ in 0..250 {
            let answer = self.get(path, cookie).await;
            if wanted(&answer.body) {
                return answer.body;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{path} never came to be as wanted");
    }
}

#[tokio::test]
async fn a_backup_is_made_kept_and_put_back() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let root = panel.a_server().await;
    let cookie = Some(cookie.as_str());
    let level = "/api/v1/servers/1/files/content?path=world/level.dat";
    panel.upload(level, b"level", cookie).await;
    let backups = "/api/v1/servers/1/backups";
    let done = |all: &Value| all["backups"][0]["state"] == "done";

    let none = panel.get(backups, cookie).await;
    assert_eq!(none.body, json!({ "kept": 3, "backups": [] }));
    let long = json!({ "name": "x".repeat(61) });
    assert_eq!(
        panel.post(backups, long, cookie).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Begun at once, done a moment later.
    let begun = panel
        .post(backups, json!({ "name": " Before the update " }), cookie)
        .await;
    assert_eq!(begun.status, StatusCode::ACCEPTED, "{}", begun.body);
    assert_eq!(begun.body["name"], "Before the update");
    let first = begun.body["id"].as_i64().unwrap();
    let all = panel.until(backups, cookie, done).await;
    assert!(all["backups"][0]["size_bytes"].as_i64().unwrap() > 0);
    assert!(all["backups"][0]["finished_at"].as_i64().unwrap() > 1_700_000_000);
    // Kept beside the server's files, and not among them.
    let kept_at = panel
        .files
        .path()
        .join(format!("backups/a-server/{first}.tar.zst"));
    assert!(kept_at.exists());

    let one = format!("{backups}/{first}");
    let (status, headers, bytes) = panel.download(&format!("{one}/download"), cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes[..4], [0x28, 0xb5, 0x2f, 0xfd]);
    let saved_as = headers[CONTENT_DISPOSITION].to_str().unwrap();
    assert!(
        saved_as.starts_with("attachment; filename=\"Before the update-20"),
        "{saved_as}"
    );
    assert!(saved_as.ends_with(".tar.zst"), "{saved_as}");

    // Put back, the files are what they were: what came since is gone.
    panel.upload(level, b"newer", cookie).await;
    panel
        .upload(
            "/api/v1/servers/1/files/content?path=added.txt",
            b"x",
            cookie,
        )
        .await;
    let restored = panel
        .post(&format!("{one}/restore"), json!({}), cookie)
        .await;
    assert_eq!(restored.status, StatusCode::ACCEPTED, "{}", restored.body);
    panel
        .until(level, cookie, |file| file["text"] == "level")
        .await;
    assert!(!root.join("added.txt").exists());

    // One more than is kept, and the oldest goes.
    let kept = format!("{backups}/kept");
    for (number, status) in [
        (0, StatusCode::UNPROCESSABLE_ENTITY),
        (21, StatusCode::UNPROCESSABLE_ENTITY),
        (1, StatusCode::NO_CONTENT),
    ] {
        assert_eq!(
            panel
                .put(&kept, json!({ "kept": number }), cookie)
                .await
                .status,
            status
        );
    }
    let second = panel.post(backups, json!({}), cookie).await;
    assert_eq!(second.body["name"], "Backup");
    let second = second.body["id"].as_i64().unwrap();
    let all = panel
        .until(backups, cookie, |all| {
            done(all) && all["backups"].as_array().unwrap().len() == 1
        })
        .await;
    assert_eq!(all["kept"], 1);
    assert_eq!(all["backups"][0]["id"], second);
    assert!(!kept_at.exists());
    for gone in ["download", "restore"] {
        let path = format!("{one}/{gone}");
        let answer = match gone {
            "restore" => panel.post(&path, json!({}), cookie).await.status,
            _ => panel.download(&path, cookie).await.0,
        };
        assert_eq!(answer, StatusCode::NOT_FOUND);
    }

    let last = format!("{backups}/{second}");
    assert_eq!(
        panel.delete(&last, cookie).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        panel.delete(&last, cookie).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(panel.get(backups, cookie).await.body["backups"], json!([]));
    assert_eq!(
        panel.get("/api/v1/servers/2/backups", cookie).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_schedule_is_kept_with_the_time_it_comes_next() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    panel.a_server().await;
    let cookie = Some(cookie.as_str());
    let schedules = "/api/v1/servers/1/schedules";
    assert_eq!(panel.get(schedules, cookie).await.body, json!([]));

    // Four in the morning, on a clock eight hours ahead of UTC.
    let nightly = json!({
        "name": " Nightly backup ",
        "cron": "0  4 * * *",
        "utc_offset": 480,
        "enabled": true,
        "tasks": [
            { "action": "command", "command": "save-all" },
            { "action": "backup", "wait_seconds": 10 },
        ],
    });
    let made = panel.post(schedules, nightly.clone(), cookie).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    assert_eq!(made.body["name"], "Nightly backup");
    assert_eq!(made.body["cron"], "0 4 * * *");
    assert_eq!(made.body["tasks"][1]["wait_seconds"], 10);
    assert_eq!(made.body["last_run_at"], Value::Null);
    let next = made.body["next_run_at"].as_i64().unwrap();
    assert_eq!((next + 480 * 60) % 86_400, 4 * 3600);
    let id = made.body["id"].as_i64().unwrap();
    let listed = panel.get(schedules, cookie).await;
    assert_eq!(listed.body, json!([made.body]));

    let with = |change: Value| {
        let mut changed = nightly.clone();
        for (key, value) in change.as_object().unwrap() {
            changed[key] = value.clone();
        }
        changed
    };
    for (wrong, said) in [
        (
            json!({ "cron": "nope" }),
            "A schedule's time is five fields: minute, hour, day, month and weekday.",
        ),
        (
            json!({ "cron": "0 25 * * *" }),
            "The hour is 0 to 23, and \"25\" is not that.",
        ),
        (json!({ "cron": "0 0 30 2 *" }), "That time never comes."),
        (
            json!({ "name": " " }),
            "A schedule's name is 1 to 60 characters.",
        ),
        (
            json!({ "utc_offset": 900 }),
            "That is not a clock's distance from UTC.",
        ),
        (json!({ "tasks": [] }), "A schedule does 1 to 10 things."),
        (
            json!({ "tasks": [{ "action": "command", "command": "one\ntwo" }] }),
            "A command is one line of up to 1000 characters.",
        ),
        (
            json!({ "tasks": [{ "action": "command" }] }),
            "A command is one line of up to 1000 characters.",
        ),
        (
            json!({ "tasks": [{ "action": "stop", "wait_seconds": 3601 }] }),
            "A task waits an hour at the most.",
        ),
    ] {
        let answer = panel.post(schedules, with(wrong), cookie).await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{said}");
        assert_eq!(answer.body["error"], said);
    }

    // Not enabled, it has no next time.
    let one = format!("{schedules}/{id}");
    let off = panel
        .put(
            &one,
            with(json!({ "enabled": false, "name": "Nightly" })),
            cookie,
        )
        .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["name"], "Nightly");
    assert_eq!(off.body["next_run_at"], Value::Null);
    // Set off by hand it runs all the same, and says what that came to.
    let run = panel.post(&format!("{one}/run"), json!({}), cookie).await;
    assert_eq!(run.status, StatusCode::ACCEPTED);
    let ran = panel
        .until(schedules, cookie, |all| {
            all[0]["last_result"] != Value::Null
        })
        .await;
    assert_eq!(
        ran[0]["last_result"],
        "Not run: Homewarp cannot reach Docker."
    );
    assert!(ran[0]["last_run_at"].as_i64().unwrap() > 1_700_000_000);

    assert_eq!(
        panel.delete(&one, cookie).await.status,
        StatusCode::NO_CONTENT
    );
    for answer in [
        panel.delete(&one, cookie).await,
        panel.put(&one, nightly.clone(), cookie).await,
        panel.post(&format!("{one}/run"), json!({}), cookie).await,
        panel.get("/api/v1/servers/2/schedules", cookie).await,
    ] {
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.body);
    }
    // What a schedule did when nobody asked is written down under Homewarp's name.
    let log = panel.get("/api/v1/activity", cookie).await;
    let ran = log
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["action"] == "schedule.ran")
        .unwrap();
    assert_eq!(ran["username"], "Homewarp");
    assert_eq!(ran["user_id"], Value::Null);
    assert_eq!(
        ran["detail"],
        "Nightly: Not run: Homewarp cannot reach Docker."
    );
}

#[tokio::test]
async fn where_servers_look_names_up_is_the_owners_to_set() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let cookie = Some(cookie.as_str());
    let settings = "/api/v1/settings";

    let first = panel.get(settings, cookie).await;
    assert_eq!(
        first.body,
        json!({ "resolvers": ["1.1.1.1", "1.0.0.1"], "new_connections": 30 })
    );
    // Kept each once, as addresses are written.
    let quad9 = json!({ "resolvers": ["9.9.9.9", " 9.9.9.9 ", "149.112.112.112"] });
    let changed = panel.put(settings, quad9, cookie).await;
    assert_eq!(changed.status, StatusCode::OK, "{}", changed.body);
    let kept = json!({ "resolvers": ["9.9.9.9", "149.112.112.112"], "new_connections": 30 });
    assert_eq!(changed.body, kept);

    for (wrong, said) in [
        (
            json!(["192.168.1.1"]),
            "Servers are kept from the home network, so they could not ask 192.168.1.1. Give a resolver on the internet, such as 1.1.1.1 or 9.9.9.9.",
        ),
        (
            json!(["dns.example"]),
            "dns.example is not an IPv4 address.",
        ),
        (json!([]), "Give one to three resolvers."),
        (
            json!(["1.1.1.1", "1.0.0.1", "8.8.8.8", "8.8.4.4"]),
            "Give one to three resolvers.",
        ),
    ] {
        let answer = panel
            .put(settings, json!({ "resolvers": wrong }), cookie)
            .await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{said}");
        assert_eq!(answer.body["error"], said);
    }
    assert_eq!(panel.get(settings, cookie).await.body, kept);

    // The limit on new connections is set by itself, and the resolvers stay.
    let limited = panel
        .put(settings, json!({ "new_connections": 5 }), cookie)
        .await;
    assert_eq!(limited.status, StatusCode::OK, "{}", limited.body);
    assert_eq!(limited.body["new_connections"], 5);
    assert_eq!(limited.body["resolvers"], kept["resolvers"]);
    for wrong in [json!(0), json!(10_001), json!(-1), json!("many")] {
        let answer = panel
            .put(settings, json!({ "new_connections": wrong }), cookie)
            .await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{wrong}");
    }
    // Nothing named, nothing changed.
    let same = panel.put(settings, json!({}), cookie).await;
    assert_eq!(same.body["new_connections"], 5);
    assert_eq!(same.body["resolvers"], kept["resolvers"]);

    assert_eq!(
        panel.get(settings, None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_panel_is_given_a_name_by_its_owner_where_it_has_a_door_for_tls() {
    let at = "/api/v1/panel";
    let named = json!({ "name": "panel.example.com", "agreed": true });

    // Without a door for TLS there is nothing for a name to lead to.
    let plain = panel().await;
    let cookie = plain.set_up().await;
    let view = plain.get(at, Some(&cookie)).await;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(view.body["available"], false);
    assert_eq!(view.body["state"], "off");
    let refused = plain.put(at, named.clone(), Some(&cookie)).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);

    let panel = panel_at(Some(8443)).await;
    let cookie = panel.set_up().await;
    let cookie = Some(cookie.as_str());
    let view = panel.get(at, cookie).await;
    assert_eq!(view.body["available"], true);
    assert_eq!(view.body["port"], 8443);
    assert_eq!(view.body["name"], Value::Null);
    assert_eq!(view.body["authority"], "acme-v02.api.letsencrypt.org");

    for (wrong, said) in [
        (
            json!({ "name": "192.168.1.250", "agreed": true }),
            "A name is like panel.example.com",
        ),
        (
            json!({ "name": "panel.example.com:8443", "agreed": true }),
            "A name is like panel.example.com",
        ),
        (
            json!({ "name": "panel.example.com" }),
            "whose terms have to be agreed to first",
        ),
    ] {
        let answer = panel.put(at, wrong, cookie).await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{said}");
        let error = answer.body["error"].as_str().unwrap();
        assert!(error.contains(said), "{error}");
    }
    assert_eq!(panel.get(at, cookie).await.body["name"], Value::Null);

    // Kept as names are written, and a certificate is asked for at once.
    let spelt = json!({ "name": " Panel.Example.com. ", "agreed": true });
    let given = panel.put(at, spelt, cookie).await;
    assert_eq!(given.status, StatusCode::OK, "{}", given.body);
    assert_eq!(given.body["name"], "panel.example.com");
    assert_eq!(given.body["state"], "asking");
    // There is no address to give out until there is a certificate for it.
    assert_eq!(given.body["address"], Value::Null);
    assert_eq!(given.body["certificate"], Value::Null);
    let again = panel
        .post("/api/v1/panel/certificate", json!({}), cookie)
        .await;
    assert_eq!(again.status, StatusCode::ACCEPTED, "{}", again.body);

    // The port is the panel's own now, and no server's.
    let template = panel
        .post("/api/v1/templates", json!({ "egg": EGG }), cookie)
        .await
        .body["id"]
        .as_i64()
        .unwrap();
    let server =
        json!({ "name": "Survival", "template_id": template, "memory_mb": 1024, "port": 8443 });
    let clash = panel.post("/api/v1/servers", server, cookie).await;
    assert_eq!(clash.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(clash.body["error"], "Port 8443 is the panel's own.");

    // Taken away again, there is nothing to ask a certificate for.
    let off = panel.put(at, json!({ "name": null }), cookie).await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["state"], "off");
    assert_eq!(off.body["name"], Value::Null);
    let nothing = panel
        .post("/api/v1/panel/certificate", json!({}), cookie)
        .await;
    assert_eq!(nothing.status, StatusCode::CONFLICT);

    // Written down, both times.
    let activity = panel.get("/api/v1/activity", cookie).await;
    let named: Vec<&str> = activity
        .body
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["action"] == "panel.name")
        .map(|entry| entry["detail"].as_str().unwrap())
        .collect();
    assert_eq!(named, ["no name", "panel.example.com"]);

    assert_eq!(panel.get(at, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        panel.put(at, json!({ "name": null }), None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_sign_in_that_keeps_failing_has_to_wait() {
    let panel = panel().await;
    panel.set_up().await;
    let wrong = json!({ "username": "lance", "password": "not the password" });
    for _ in 0..5 {
        let answer = panel.post("/api/v1/login", wrong.clone(), None).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    // The sixth is not looked at, and neither is the right password after it:
    // whoever is guessing learns nothing more from here.
    let right = json!({ "username": "lance", "password": PASSWORD });
    for tried in [wrong, right] {
        let answer = panel.post("/api/v1/login", tried, None).await;
        assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            answer.body["error"],
            "Too many wrong tries. Wait 5 minutes and try again."
        );
    }
}

/// The code an authenticator app would show now for a secret: RFC 6238,
/// written out a second time so that the test does not ask Homewarp's own.
fn code_now(secret: &str) -> String {
    use sha1::{Digest, Sha1};
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut bits, mut held, mut key) = (0u32, 0, Vec::new());
    for symbol in secret.bytes() {
        bits = (bits << 5) | alphabet.iter().position(|known| *known == symbol).unwrap() as u32;
        held += 5;
        if held >= 8 {
            held -= 8;
            key.push((bits >> held) as u8);
        }
    }
    let mut block = [0u8; 64];
    block[..key.len()].copy_from_slice(&key);
    let step = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30;
    let inner = Sha1::new()
        .chain_update(block.map(|byte| byte ^ 0x36))
        .chain_update(step.to_be_bytes())
        .finalize();
    let hash = Sha1::new()
        .chain_update(block.map(|byte| byte ^ 0x5c))
        .chain_update(inner)
        .finalize();
    let at = usize::from(hash[19] & 0x0f);
    let word = u32::from_be_bytes([hash[at], hash[at + 1], hash[at + 2], hash[at + 3]]);
    format!("{:06}", (word & 0x7fff_ffff) % 1_000_000)
}

#[tokio::test]
async fn a_second_step_is_asked_for_once_it_is_turned_on() {
    let panel = panel().await;
    let cookie = panel.set_up().await;
    let cookie = Some(cookie.as_str());
    let two_steps = "/api/v1/account/two-steps";
    let confirm = "/api/v1/account/two-steps/confirm";
    let login = |code: Option<String>| {
        let mut typed = json!({ "username": "lance", "password": PASSWORD });
        if let Some(code) = code {
            typed["code"] = json!(code);
        }
        panel.post("/api/v1/login", typed, None)
    };

    assert_eq!(
        panel.get(two_steps, cookie).await.body,
        json!({ "on": false, "recovery_codes": 0 })
    );
    // Nothing is being set up yet, so there is nothing to confirm.
    let early = panel
        .post(confirm, json!({ "code": "123456" }), cookie)
        .await;
    assert_eq!(early.status, StatusCode::CONFLICT);

    let begun = panel.post(two_steps, json!({}), cookie).await;
    assert_eq!(begun.status, StatusCode::OK, "{}", begun.body);
    let secret = begun.body["secret"].as_str().unwrap().to_owned();
    assert_eq!(
        begun.body["uri"],
        format!("otpauth://totp/Homewarp:lance?secret={secret}&issuer=Homewarp")
    );
    // Until the app has shown that it has the secret, nothing has changed.
    let wrong = panel
        .post(confirm, json!({ "code": "12345" }), cookie)
        .await;
    assert_eq!(wrong.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(login(None).await.status, StatusCode::OK);

    let code = code_now(&secret);
    let on = panel.post(confirm, json!({ "code": code }), cookie).await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    let recovery: Vec<String> = serde_json::from_value(on.body["recovery_codes"].clone()).unwrap();
    assert_eq!(recovery.len(), 8);
    assert_eq!(
        panel.get(two_steps, cookie).await.body,
        json!({ "on": true, "recovery_codes": 8 })
    );
    assert_eq!(
        panel.post(two_steps, json!({}), cookie).await.status,
        StatusCode::CONFLICT
    );

    // The password alone no longer signs in, and says what is missing.
    let asked = login(None).await;
    assert_eq!(asked.status, StatusCode::UNAUTHORIZED);
    assert_eq!(asked.body["code_required"], true);
    assert_eq!(
        asked.body["error"],
        "Enter the code from your authenticator app."
    );
    // A code is good once: the one that turned this on is spent.
    let again = login(Some(code)).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert_eq!(again.body["code_required"], true);
    // A wrong password is still only a wrong password.
    let guess = json!({ "username": "lance", "password": "not the password", "code": "123456" });
    let guessed = panel.post("/api/v1/login", guess, None).await;
    assert_eq!(
        guessed.body,
        json!({ "error": "Wrong username or password." })
    );

    // A recovery code signs in, once.
    let signed = login(Some(recovery[0].to_lowercase())).await;
    assert_eq!(signed.status, StatusCode::OK, "{}", signed.body);
    assert_eq!(
        login(Some(recovery[0].clone())).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(panel.get(two_steps, cookie).await.body["recovery_codes"], 7);

    // Turned off with the password, the password is enough again.
    let off = "/api/v1/account/two-steps/off";
    let wrong = panel
        .post(off, json!({ "password": "not the password" }), cookie)
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let right = panel
        .post(off, json!({ "password": PASSWORD }), cookie)
        .await;
    assert_eq!(right.status, StatusCode::NO_CONTENT);
    assert_eq!(login(None).await.status, StatusCode::OK);
    assert_eq!(
        panel.get(two_steps, cookie).await.body,
        json!({ "on": false, "recovery_codes": 0 })
    );
    assert_eq!(
        panel.get(two_steps, None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

/// The web client's types are generated from `web/openapi.json`. If this fails,
/// the API changed: run `scripts/dev.sh gen` and commit the result.
#[test]
fn the_api_description_in_web_is_current() {
    let committed = include_str!("../../../web/openapi.json");
    assert_eq!(committed.trim(), openapi().to_pretty_json().unwrap().trim());
}
