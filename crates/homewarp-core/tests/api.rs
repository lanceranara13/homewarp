//! The API as a browser meets it: requests in, responses out, a real database
//! file underneath.

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{
        Request, StatusCode,
        header::{CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE},
    },
};
use homewarp_core::{AppState, app, open, openapi};
use serde_json::{Value, json};
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
    _files: tempfile::TempDir,
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
    let files = tempfile::tempdir().unwrap();
    let db = open(&files.path().join("homewarp.db")).await.unwrap();
    let state = AppState::start(db).await.unwrap();
    let setup_code = state
        .setup_code()
        .expect("a new database has no account")
        .to_owned();
    Panel {
        app: app(state),
        setup_code,
        _files: files,
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

    async fn delete(&self, path: &str, cookie: Option<&str>) -> Answer {
        let request = Request::delete(path).header(COOKIE, cookie.unwrap_or_default());
        self.send(request.body(Body::empty()).unwrap()).await
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

/// The web client's types are generated from `web/openapi.json`. If this fails,
/// the API changed: run `scripts/dev.sh gen` and commit the result.
#[test]
fn the_api_description_in_web_is_current() {
    let committed = include_str!("../../../web/openapi.json");
    assert_eq!(committed.trim(), openapi().to_pretty_json().unwrap().trim());
}
