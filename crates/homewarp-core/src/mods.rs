//! Mods and plugins for a Minecraft server, found on Modrinth and put among
//! the server's files (PLAN.md §11, Phase 7).
//!
//! It is Core that asks Modrinth, and not the page: the panel's pages talk to
//! the panel and to nothing else. What is asked is held to what any fetch is
//! held to (`fetch`), and what is installed is held to more. A release is
//! named by its id and described by Modrinth, not by the page, so the address
//! of its file and the checksum it must have come from there. The file is
//! fetched from Modrinth's own file server and from nowhere else, is a `.jar`
//! with a plain name, and is written only once its SHA-512 is the one
//! Modrinth gives for it.
//!
//! A mod is somebody's program, and it runs with the server it is put in:
//! inside the server's container, with the server's files. Installing one is
//! for an account that may write the server's files, which could upload the
//! same file by hand.

use std::{io::Write as _, time::Duration};

use axum::{
    Json,
    extract::{Path, Query, State},
};
use homewarp_template::Template;
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Problem, ProblemBody, SignedIn},
    audit, fetch,
    files::{NO_ROOM, files_of, room, with},
};

const MODRINTH: &str = "https://api.modrinth.com/v2";
/// Where Modrinth serves files from. A file is fetched from there and nowhere else.
const FILES_FROM: &str = "cdn.modrinth.com";
/// The most a list from Modrinth may be. A project with hundreds of releases
/// describes each of them.
const LARGEST_ANSWER: usize = 8 << 20;
/// The most a mod's file may be. The largest are a few tens of megabytes.
const LARGEST_FILE: u64 = 64 << 20;
/// How long a list may take, and how long a file.
const ASKING: Duration = Duration::from_secs(20);
const DOWNLOADING: Duration = Duration::from_secs(180);
/// How many projects one search answers with, and how many releases of one.
const FOUND: usize = 20;
const RELEASES: usize = 12;

/// What loads mods or plugins, and the folder each of them looks in.
const LOADERS: [(&str, &str); 12] = [
    ("paper", "plugins"),
    ("purpur", "plugins"),
    ("folia", "plugins"),
    ("spigot", "plugins"),
    ("bukkit", "plugins"),
    ("velocity", "plugins"),
    ("bungeecord", "plugins"),
    ("waterfall", "plugins"),
    ("fabric", "mods"),
    ("quilt", "mods"),
    ("forge", "mods"),
    ("neoforge", "mods"),
];

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(find_mods, install_mod))
        .routes(routes!(list_releases))
}

/// Whether a server made from this template is one of Minecraft's, and so one
/// that mods or plugins are for. An egg does not say what game it is; what
/// tells is the file it sets up, which only these servers have.
pub(crate) fn fits(template: &Template) -> bool {
    template.config_files.iter().any(|file| {
        let name = file.path.rsplit('/').next().unwrap_or(&file.path);
        matches!(name, "server.properties" | "velocity.toml")
    })
}

fn folder_of(loader: &str) -> Option<&'static str> {
    LOADERS
        .iter()
        .find(|(name, _)| *name == loader)
        .map(|(_, folder)| *folder)
}

/// A project's or a release's id, or a project's short name, as Modrinth
/// writes them. It goes into an address, so nothing else is taken.
fn is_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, b'-' | b'_'))
}

/// A version of Minecraft as it is written: 1.21.1, 24w14a, 1.20-pre1.
fn is_game(game: &str) -> bool {
    (1..=16).contains(&game.len())
        && game
            .bytes()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, b'.' | b'-'))
}

/// A file's name that is safe to give a file among a server's: a `.jar`, by a
/// plain name, with nothing in it that is a path.
fn is_file_name(name: &str) -> bool {
    (5..=120).contains(&name.len())
        && name.ends_with(".jar")
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|letter| letter.is_ascii_alphanumeric() || b"._+-()[] ".contains(&letter))
}

/// Text for one part of an address: everything that is not a letter, a digit
/// or one of the four marks an address may hold anywhere is written by its number.
fn encoded(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                written.push(char::from(byte));
            }
            other => written.push_str(&format!("%{other:02X}")),
        }
    }
    written
}

/// What somebody else wrote, as it is shown: on one line, and no longer than `longest`.
fn tidy(text: &str, longest: usize) -> String {
    let one_line: String = text
        .chars()
        .map(|letter| if letter.is_control() { ' ' } else { letter })
        .collect();
    one_line.trim().chars().take(longest).collect()
}

/// What is looked for.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Wanted {
    /// Words to look for. With none, the most downloaded are answered.
    #[serde(default)]
    query: String,
    /// What loads it on this server: paper, purpur, folia, spigot, bukkit,
    /// velocity, bungeecord, waterfall, fabric, quilt, forge or neoforge.
    loader: String,
    /// The version of Minecraft the server runs, such as 1.21.1. Left out, any.
    #[serde(default)]
    game: String,
}

impl Wanted {
    /// Refuses a loader Homewarp does not know and a version that is not written as one.
    fn checked(&self) -> Result<(), Problem> {
        if folder_of(&self.loader).is_none() {
            return Err(Problem::Invalid(
                "Say what loads it: paper, purpur, folia, spigot, bukkit, velocity, bungeecord, waterfall, fabric, quilt, forge or neoforge.".into(),
            ));
        }
        if !self.game.is_empty() && !is_game(&self.game) {
            return Err(Problem::Invalid(
                "A version of Minecraft is written like 1.21.1.".into(),
            ));
        }
        Ok(())
    }
}

/// A project on Modrinth.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
struct Found {
    /// Its id on Modrinth.
    id: String,
    /// The name it has in its address there: modrinth.com/project/<slug>.
    slug: String,
    title: String,
    description: String,
    author: String,
    downloads: u64,
}

/// One release of a project: one file, for some loaders and some versions of Minecraft.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
struct Release {
    /// Its id on Modrinth.
    id: String,
    name: String,
    /// The version its authors gave it.
    number: String,
    /// release, beta or alpha.
    channel: String,
    /// The versions of Minecraft it is for, oldest first.
    games: Vec<String>,
    loaders: Vec<String>,
    /// The name of its file.
    file: String,
    /// In bytes.
    size: u64,
    /// How many other projects it needs, which are not installed with it.
    requires: u32,
}

/// A release, with what is needed to fetch its file and to know it is that file.
struct Described {
    release: Release,
    url: String,
    sha512: String,
}

/// The projects in what a search answered.
fn found(answer: &str) -> Option<Vec<Found>> {
    let answer: Value = serde_json::from_str(answer).ok()?;
    let hits = answer.get("hits")?.as_array()?;
    let projects = hits.iter().filter_map(|hit| {
        let text = |name: &str, longest: usize| Some(tidy(hit.get(name)?.as_str()?, longest));
        let id = text("project_id", 64).filter(|id| is_id(id))?;
        Some(Found {
            id,
            slug: text("slug", 64)
                .filter(|slug| is_id(slug))
                .unwrap_or_default(),
            title: text("title", 100)?,
            description: text("description", 300).unwrap_or_default(),
            author: text("author", 60).unwrap_or_default(),
            downloads: hit.get("downloads").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    Some(projects.take(FOUND).collect())
}

/// A release as Modrinth describes it. None for one that has no file, or
/// whose description lacks what a file is known by.
fn described(version: &Value) -> Option<Described> {
    let text =
        |from: &Value, name: &str, longest: usize| Some(tidy(from.get(name)?.as_str()?, longest));
    let texts = |name: &str| -> Vec<String> {
        version
            .get(name)
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .filter_map(Value::as_str)
                    .map(|one| tidy(one, 24))
                    .take(60)
                    .collect()
            })
            .unwrap_or_default()
    };
    let files = version.get("files")?.as_array()?;
    // The one its authors call the main one, where there are several.
    let file = files
        .iter()
        .find(|file| file.get("primary").and_then(Value::as_bool) == Some(true))
        .or_else(|| files.first())?;
    let requires = version
        .get("dependencies")
        .and_then(Value::as_array)
        .map_or(0, |needed| {
            needed
                .iter()
                .filter(|one| {
                    one.get("dependency_type").and_then(Value::as_str) == Some("required")
                })
                .count()
        });
    Some(Described {
        release: Release {
            id: text(version, "id", 64).filter(|id| is_id(id))?,
            name: text(version, "name", 100).unwrap_or_default(),
            number: text(version, "version_number", 60).unwrap_or_default(),
            channel: text(version, "version_type", 12).unwrap_or_default(),
            games: texts("game_versions"),
            loaders: texts("loaders"),
            file: file.get("filename")?.as_str()?.to_owned(),
            size: file.get("size").and_then(Value::as_u64)?,
            requires: u32::try_from(requires).unwrap_or(u32::MAX),
        },
        url: file.get("url")?.as_str()?.to_owned(),
        sha512: file
            .get("hashes")?
            .get("sha512")?
            .as_str()?
            .to_ascii_lowercase(),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Asks Modrinth, and puts what went wrong in a sentence for whoever asked.
async fn ask(address: &str) -> Result<String, Problem> {
    let answered = fetch::bytes(address, LARGEST_ANSWER, ASKING)
        .await
        .map_err(|why| Problem::Conflict(why.into()))?;
    String::from_utf8(answered.to_vec())
        .map_err(|_| Problem::Conflict("Modrinth answered with something that is not text.".into()))
}

const NOT_UNDERSTOOD: Problem = Problem::Conflict(std::borrow::Cow::Borrowed(
    "Modrinth answered with something Homewarp does not understand.",
));

/// Mods or plugins on Modrinth for what this server runs: by what was typed,
/// or the most downloaded where nothing was. Core asks Modrinth; the page
/// does not.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/mods",
    params(("id" = i64, Path, description = "The server's id."), Wanted),
    responses(
        (status = OK, body = Vec<Found>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let write this server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "Modrinth could not be asked, or did not answer as expected."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The loader or the version is not one."),
    )
)]
async fn find_mods(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Query(wanted): Query<Wanted>,
) -> Result<Json<Vec<Found>>, Problem> {
    files_of(&state, &who, id).await?;
    wanted.checked()?;
    let query = tidy(&wanted.query, 80);
    // Mods and plugins, not packs of them; for this loader; for this version.
    let mut facets = vec![
        r#"["project_type:mod","project_type:plugin"]"#.to_owned(),
        format!(r#"["categories:{}"]"#, wanted.loader),
    ];
    if !wanted.game.is_empty() {
        facets.push(format!(r#"["versions:{}"]"#, wanted.game));
    }
    let address = format!(
        "{MODRINTH}/search?limit={FOUND}&index={}&query={}&facets={}",
        if query.is_empty() {
            "downloads"
        } else {
            "relevance"
        },
        encoded(&query),
        encoded(&format!("[{}]", facets.join(","))),
    );
    found(&ask(&address).await?).map(Json).ok_or(NOT_UNDERSTOOD)
}

/// The newest releases of one project for what this server runs.
#[utoipa::path(
    get,
    path = "/api/v1/servers/{id}/mods/{project}/releases",
    params(
        ("id" = i64, Path, description = "The server's id."),
        ("project" = String, Path, description = "The project's id on Modrinth."),
        Wanted,
    ),
    responses(
        (status = OK, body = Vec<Release>),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let write this server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "Modrinth could not be asked, or did not answer as expected."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The project, the loader or the version is not one."),
    )
)]
async fn list_releases(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path((id, project)): Path<(i64, String)>,
    Query(wanted): Query<Wanted>,
) -> Result<Json<Vec<Release>>, Problem> {
    files_of(&state, &who, id).await?;
    wanted.checked()?;
    if !is_id(&project) {
        return Err(Problem::Invalid(
            "That is not a project on Modrinth.".into(),
        ));
    }
    let mut address = format!(
        "{MODRINTH}/project/{project}/version?loaders={}",
        encoded(&format!(r#"["{}"]"#, wanted.loader)),
    );
    if !wanted.game.is_empty() {
        address.push_str("&game_versions=");
        address.push_str(&encoded(&format!(r#"["{}"]"#, wanted.game)));
    }
    let answer: Value = serde_json::from_str(&ask(&address).await?).map_err(|_| NOT_UNDERSTOOD)?;
    let releases = answer
        .as_array()
        .ok_or(NOT_UNDERSTOOD)?
        .iter()
        .filter_map(described)
        .map(|described| described.release)
        .take(RELEASES)
        .collect();
    Ok(Json(releases))
}

/// The release to install.
#[derive(Deserialize, ToSchema)]
struct Wants {
    /// The release's id on Modrinth.
    release: String,
    /// What loads it on this server, which says which folder it goes in.
    loader: String,
}

/// Where a mod was put.
#[derive(Serialize, ToSchema)]
struct Installed {
    /// The file, from the top of the server's files.
    path: String,
}

/// Fetches a release's file from Modrinth and puts it in the folder this
/// server's loader reads: `plugins` or `mods`. It is written only once its
/// checksum is the one Modrinth gives for it, and is loaded when the server
/// next starts. What the release needs beside itself is not installed with it.
#[utoipa::path(
    post,
    path = "/api/v1/servers/{id}/mods",
    params(("id" = i64, Path, description = "The server's id.")),
    request_body = Wants,
    responses(
        (status = OK, body = Installed),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account has not been let write this server's files."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such server."),
        (status = CONFLICT, body = ProblemBody, description = "The file could not be fetched, was not the file Modrinth describes, or there is no room for it."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The release or the loader is not one, or the release is not for that loader."),
    )
)]
async fn install_mod(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
    Path(id): Path<i64>,
    Json(wants): Json<Wants>,
) -> Result<Json<Installed>, Problem> {
    let files = files_of(&state, &who, id).await?;
    let invalid = |sentence: String| Problem::Invalid(sentence.into());
    let conflict = |sentence: String| Problem::Conflict(sentence.into());
    let Some(folder) = folder_of(&wants.loader) else {
        return Err(invalid("Homewarp knows no such loader.".to_owned()));
    };
    if !is_id(&wants.release) {
        return Err(invalid("That is not a release on Modrinth.".to_owned()));
    }
    // Described by Modrinth, whatever the page said of it.
    let answer: Value =
        serde_json::from_str(&ask(&format!("{MODRINTH}/version/{}", wants.release)).await?)
            .map_err(|_| NOT_UNDERSTOOD)?;
    let Described {
        release,
        url,
        sha512,
    } = described(&answer).ok_or(NOT_UNDERSTOOD)?;
    if !release.loaders.contains(&wants.loader) {
        return Err(invalid(format!(
            "That release is not for {}. It is for {}.",
            wants.loader,
            release.loaders.join(", ")
        )));
    }
    if !is_file_name(&release.file) {
        return Err(invalid(format!(
            "Its file is called {}, which is not a name Homewarp gives a file. Download it from Modrinth and upload it under Files.",
            tidy(&release.file, 80)
        )));
    }
    if fetch::site(&url).ok().as_deref() != Some(FILES_FROM) {
        return Err(conflict(
            "Modrinth says the file is somewhere other than on Modrinth, and it is not fetched from there.".to_owned(),
        ));
    }
    if release.size > LARGEST_FILE {
        return Err(conflict(format!(
            "Its file is {} MB, which is more than Homewarp fetches. Upload it under Files.",
            release.size >> 20
        )));
    }
    if room(&files).await? < release.size {
        return Err(NO_ROOM);
    }
    let most = usize::try_from(release.size).unwrap_or(usize::MAX);
    let fetched = fetch::bytes(&url, most, DOWNLOADING)
        .await
        .map_err(conflict)?;
    // The file Modrinth describes, and not one put in its place on the way.
    if hex(digest::digest(&digest::SHA512, &fetched).as_ref()) != sha512 {
        return Err(conflict(
            "What was fetched is not the file Modrinth describes: its checksum is another. Nothing was written.".to_owned(),
        ));
    }
    let path = format!("{folder}/{}", release.file);
    let to = path.clone();
    with(&files, move |files| {
        let (mut file, part) = files.begin(&to)?;
        match file.write_all(&fetched).and_then(|()| file.flush()) {
            Ok(()) => files.finish(&part, &to),
            Err(error) => {
                files.abandon(&part);
                Err(error)
            }
        }
    })
    .await?;
    let detail = format!("{path} ({})", release.number);
    audit::record(&state.db, &who, Some(id), "mods.install", &detail).await;
    Ok(Json(Installed { path }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Found, described, encoded, fits, folder_of, found, hex, is_file_name, is_game, is_id,
    };

    #[test]
    fn a_search_is_read_for_its_projects() {
        // In the shape Modrinth answers a search in.
        let answer = json!({
            "hits": [
                {
                    "project_id": "Vebnzrzj", "slug": "luckperms", "title": "LuckPerms",
                    "description": "A permissions plugin\nfor Minecraft servers.", "author": "lucko",
                    "downloads": 1_234_567, "project_type": "mod", "icon_url": "https://cdn.modrinth.com/x.png"
                },
                // One whose id could not go into an address is passed over.
                { "project_id": "../../etc", "slug": "x", "title": "X" },
                { "project_id": "AANobbMI", "title": "Sodium" }
            ],
            "offset": 0, "limit": 20, "total_hits": 3
        });
        assert_eq!(
            found(&answer.to_string()).unwrap(),
            [
                Found {
                    id: "Vebnzrzj".to_owned(),
                    slug: "luckperms".to_owned(),
                    title: "LuckPerms".to_owned(),
                    description: "A permissions plugin for Minecraft servers.".to_owned(),
                    author: "lucko".to_owned(),
                    downloads: 1_234_567,
                },
                Found {
                    id: "AANobbMI".to_owned(),
                    slug: String::new(),
                    title: "Sodium".to_owned(),
                    description: String::new(),
                    author: String::new(),
                    downloads: 0,
                },
            ]
        );
        assert_eq!(found(r#"{"error":"not_found"}"#), None);
        assert_eq!(found("<html>"), None);
    }

    #[test]
    fn a_release_is_read_for_its_one_file_and_what_it_is_known_by() {
        let version = json!({
            "id": "tVWPKTgl", "project_id": "Vebnzrzj", "name": "LuckPerms Velocity 5.4.150",
            "version_number": "v5.4.150-velocity", "version_type": "release",
            "game_versions": ["1.20.6", "1.21", "1.21.1"], "loaders": ["velocity"],
            "dependencies": [
                { "project_id": "aaaa", "dependency_type": "required" },
                { "project_id": "bbbb", "dependency_type": "optional" }
            ],
            "files": [
                { "filename": "sources.jar", "url": "https://cdn.modrinth.com/data/x/sources.jar",
                  "primary": false, "size": 10, "hashes": { "sha1": "00", "sha512": "AB" } },
                { "filename": "LuckPerms-Velocity-5.4.150.jar",
                  "url": "https://cdn.modrinth.com/data/Vebnzrzj/versions/tVWPKTgl/LuckPerms-Velocity-5.4.150.jar",
                  "primary": true, "size": 1_500_000, "hashes": { "sha1": "11", "sha512": "CDEF01" } }
            ]
        });
        let described = described(&version).unwrap();
        assert_eq!(described.release.id, "tVWPKTgl");
        assert_eq!(described.release.file, "LuckPerms-Velocity-5.4.150.jar");
        assert_eq!(described.release.size, 1_500_000);
        assert_eq!(described.release.requires, 1);
        assert_eq!(described.release.games, ["1.20.6", "1.21", "1.21.1"]);
        assert_eq!(described.release.loaders, ["velocity"]);
        assert_eq!(described.release.channel, "release");
        // As it is compared: in small letters.
        assert_eq!(described.sha512, "cdef01");
        assert!(described.url.ends_with("/LuckPerms-Velocity-5.4.150.jar"));

        // One with no file, and one whose file has no checksum, are not releases to install.
        assert!(super::described(&json!({ "id": "abc", "files": [] })).is_none());
        let unknown = json!({ "id": "abc", "files": [{ "filename": "a.jar", "url": "https://cdn.modrinth.com/a.jar", "size": 1, "hashes": {} }] });
        assert!(super::described(&unknown).is_none());
    }

    #[test]
    fn only_a_plain_jar_is_given_a_place_among_a_servers_files() {
        for name in [
            "LuckPerms-Velocity-5.4.150.jar",
            "sodium-fabric-0.6.0+mc1.21.1.jar",
            "[Fabric 1.21] Some Mod (v2).jar",
        ] {
            assert!(is_file_name(name), "{name}");
        }
        for name in [
            "../../start.sh",
            "plugins/../../x.jar",
            "x/y.jar",
            "..jar",
            ".hidden.jar",
            "mod.zip",
            "mod.jar.sh",
            "a\\b.jar",
            "a\nb.jar",
            "mod$(id).jar",
            ".jar",
            "",
        ] {
            assert!(!is_file_name(name), "{name}");
        }
        assert!(!is_file_name(&format!("{}.jar", "x".repeat(200))));
    }

    #[test]
    fn what_goes_into_an_address_is_written_for_one() {
        assert_eq!(
            encoded(r#"[["categories:paper"],["versions:1.21.1"]]"#),
            "%5B%5B%22categories%3Apaper%22%5D%2C%5B%22versions%3A1.21.1%22%5D%5D"
        );
        assert_eq!(
            encoded("world edit & more/é"),
            "world%20edit%20%26%20more%2F%C3%A9"
        );
        assert!(is_id("Vebnzrzj") && is_id("luck-perms_2"));
        for odd in ["", "a/b", "a?b", "a b", "../x", &"x".repeat(65)] {
            assert!(!is_id(odd), "{odd}");
        }
        assert!(is_game("1.21.1") && is_game("24w14a") && is_game("1.20-pre1"));
        for odd in ["", "1.21\"]", "1 21", &"1".repeat(17)] {
            assert!(!is_game(odd), "{odd}");
        }
        assert_eq!(hex(&[0x00, 0x0f, 0xab]), "000fab");
    }

    #[test]
    fn a_loader_says_which_folder_and_a_template_whether_mods_are_for_it() {
        assert_eq!(folder_of("paper"), Some("plugins"));
        assert_eq!(folder_of("velocity"), Some("plugins"));
        assert_eq!(folder_of("neoforge"), Some("mods"));
        assert_eq!(folder_of("fabric"), Some("mods"));
        assert_eq!(folder_of("sponge"), None);
        assert_eq!(folder_of("../plugins"), None);

        let egg = |files: serde_json::Value| {
            let egg = json!({
                "meta": { "version": "PTDL_v2" }, "name": "X",
                "docker_images": { "x": "example.invalid/x" }, "startup": "run",
                "config": { "files": files.to_string(), "startup": "{\"done\": \"Done\"}", "stop": "stop" },
                "scripts": { "installation": { "script": null, "container": "example.invalid/i", "entrypoint": "sh" } },
                "variables": []
            });
            homewarp_template::import(&egg.to_string()).unwrap()
        };
        let set = |path: &str| json!({ path: { "parser": "properties", "find": { "a": "b" } } });
        assert!(fits(&egg(set("server.properties"))));
        assert!(fits(&egg(set("velocity.toml"))));
        assert!(!fits(&egg(set("valheim/config.ini"))));
        assert!(!fits(&egg(json!({}))));
    }
}
