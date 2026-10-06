//! The eggs there are to be had (PLAN.md §11, Phase 6): a catalogue of what
//! the Pelican community publishes, and an egg fetched from its address.
//!
//! Nothing is shipped with Homewarp: an egg is fetched when its owner asks
//! for it, from where its authors keep it, and shown before it is imported.
//! The catalogue is a list of names and addresses and nothing more. It is
//! fetched when the owner asks, and kept, because the place it comes from
//! answers one address only so many times an hour.

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody},
    audit, auth, fetch,
};

/// Whose eggs these are, and which of their collections hold eggs. All are
/// published under the MIT licence.
const PUBLISHER: &str = "pelican-eggs";
const COLLECTIONS: [&str; 10] = [
    "minecraft",
    "games-steamcmd",
    "games-standalone",
    "software",
    "generic",
    "chatbots",
    "voice",
    "database",
    "monitoring",
    "storage",
];
/// An egg is a few kilobytes. This leaves room for a very long install script.
const LARGEST_EGG: usize = 1 << 20;
/// A collection's list of files is a few hundred lines of JSON.
const LARGEST_LIST: usize = 4 << 20;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_catalogue, refresh_catalogue))
        .routes(routes!(fetch_egg))
}

/// One egg there is to be had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
struct CatalogueEgg {
    /// What it is called, as its file's name has it.
    name: String,
    /// The collection it is in: `minecraft`, `games-steamcmd` and so on.
    kind: String,
    /// Where in the collection: `java/paper`.
    folder: String,
    /// Where its text is fetched from.
    url: String,
}

/// The catalogue as it was last fetched.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
struct Catalogue {
    /// When, in Unix seconds. Nothing if it never was.
    fetched_at: Option<i64>,
    eggs: Vec<CatalogueEgg>,
}

/// What a collection's publisher answers when asked for all its files.
#[derive(Deserialize)]
struct Listing {
    tree: Vec<Listed>,
}

#[derive(Deserialize)]
struct Listed {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

/// A file's name as a title: `egg-counter-strike-2.json` is "Counter Strike 2".
fn title(stem: &str) -> String {
    let words = stem.split(['-', '_']).filter(|word| !word.is_empty());
    let titled: Vec<String> = words
        .map(|word| {
            let mut letters = word.chars();
            match letters.next() {
                Some(first) => first.to_uppercase().chain(letters).collect(),
                None => String::new(),
            }
        })
        .collect();
    titled.join(" ")
}

/// A path as an address writes it: what is not a letter, a digit or one of a
/// few marks is written as its bytes.
fn in_an_address(path: &str) -> String {
    let mut written = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                written.push(char::from(byte));
            }
            other => written.push_str(&format!("%{other:02X}")),
        }
    }
    written
}

/// The eggs in one collection's list of files. A collection keeps each egg
/// twice, as Pelican exports it and as Pterodactyl does: the first is listed.
fn eggs_of(collection: &str, listing: &str) -> Result<Vec<CatalogueEgg>, String> {
    let listing: Listing = serde_json::from_str(listing)
        .map_err(|_| format!("The list of {collection} was not one that could be read."))?;
    let mut eggs: Vec<CatalogueEgg> = listing
        .tree
        .iter()
        .filter(|listed| listed.kind == "blob" && !listed.path.starts_with('.'))
        .filter_map(|listed| {
            let (folder, file) = listed.path.rsplit_once('/').unwrap_or(("", &listed.path));
            let (stem, kind) = file.rsplit_once('.')?;
            let stem = stem.strip_prefix("egg-")?;
            if stem.starts_with("pterodactyl-") || !matches!(kind, "json" | "yaml" | "yml") {
                return None;
            }
            Some(CatalogueEgg {
                name: title(stem),
                kind: collection.to_owned(),
                folder: folder.to_owned(),
                url: format!(
                    "https://raw.githubusercontent.com/{PUBLISHER}/{collection}/main/{}",
                    in_an_address(&listed.path)
                ),
            })
        })
        .collect();
    eggs.sort_by(|a, b| (&a.folder, &a.name).cmp(&(&b.folder, &b.name)));
    Ok(eggs)
}

async fn kept(db: &SqlitePool) -> Result<Catalogue, Problem> {
    let json: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'catalogue'")
            .fetch_optional(db)
            .await?;
    Ok(json
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

/// The catalogue as it was last fetched. Empty until it has been.
#[utoipa::path(
    get,
    path = "/api/v1/catalogue",
    responses(
        (status = OK, body = Catalogue),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn get_catalogue(
    State(state): State<AppState>,
    _: Owner,
) -> Result<Json<Catalogue>, Problem> {
    Ok(Json(kept(&state.db).await?))
}

/// Fetches the catalogue afresh from where the eggs are published, and keeps
/// it. A collection that could not be listed leaves the whole as it was.
#[utoipa::path(
    post,
    path = "/api/v1/catalogue",
    responses(
        (status = OK, body = Catalogue),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = CONFLICT, body = ProblemBody, description = "The catalogue could not be fetched."),
    )
)]
async fn refresh_catalogue(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<Json<Catalogue>, Problem> {
    let mut eggs = Vec::new();
    for collection in COLLECTIONS {
        let list = format!(
            "https://api.github.com/repos/{PUBLISHER}/{collection}/git/trees/main?recursive=1"
        );
        let listing = fetch::text(&list, LARGEST_LIST)
            .await
            .map_err(|why| Problem::Conflict(why.into()))?;
        eggs.extend(eggs_of(collection, &listing).map_err(|why| Problem::Conflict(why.into()))?);
    }
    let catalogue = Catalogue {
        fetched_at: Some(auth::now()),
        eggs,
    };
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('catalogue', ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(serde_json::to_string(&catalogue).map_err(anyhow::Error::new)?)
    .execute(&state.db)
    .await?;
    let detail = format!("{} eggs", catalogue.eggs.len());
    audit::record(&state.db, &who, None, "catalogue.fetch", &detail).await;
    Ok(Json(catalogue))
}

/// Where an egg is to be fetched from.
#[derive(Deserialize, ToSchema)]
struct EggAddress {
    /// An address that begins with `https://`, by a name, on the usual port.
    url: String,
}

/// An egg's text, as it was fetched. Nothing has been done with it.
#[derive(Serialize, ToSchema)]
struct FetchedEgg {
    egg: String,
}

/// Fetches an egg's text from the address it is published at, for its owner
/// to read and then import. Nothing is imported by this.
#[utoipa::path(
    post,
    path = "/api/v1/templates/fetch",
    request_body = EggAddress,
    responses(
        (status = OK, body = FetchedEgg),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "That address is not one eggs are fetched from, or nothing could be fetched from it."),
    )
)]
async fn fetch_egg(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(asked): Json<EggAddress>,
) -> Result<Json<FetchedEgg>, Problem> {
    let egg = fetch::text(&asked.url, LARGEST_EGG)
        .await
        .map_err(|why| Problem::Invalid(why.into()))?;
    let from: String = asked.url.trim().chars().take(300).collect();
    audit::record(&state.db, &who, None, "template.fetch", &from).await;
    Ok(Json(FetchedEgg { egg }))
}

#[cfg(test)]
mod tests {
    use super::{eggs_of, in_an_address, title};

    #[test]
    fn a_collections_list_of_files_is_read_for_its_eggs() {
        let listing = r#"{ "sha": "x", "truncated": false, "tree": [
            { "path": ".github/ISSUE_TEMPLATE/egg-request.yml", "type": "blob" },
            { "path": "java", "type": "tree" },
            { "path": "java/paper/README.md", "type": "blob" },
            { "path": "java/paper/egg-paper.yaml", "type": "blob" },
            { "path": "java/paper/egg-pterodactyl-paper.json", "type": "blob" },
            { "path": "bedrock/PowerNukkitX/egg-power-nukkit-x.json", "type": "blob" },
            { "path": "odd name/egg-with space.json", "type": "blob" },
            { "path": "egg-at-the-top.yml", "type": "blob" },
            { "path": "java/egg-notes.txt", "type": "blob" }
        ] }"#;
        let eggs = eggs_of("minecraft", listing).unwrap();
        let found: Vec<(&str, &str, &str)> = eggs
            .iter()
            .map(|egg| (egg.name.as_str(), egg.folder.as_str(), egg.url.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "At The Top",
                    "",
                    "https://raw.githubusercontent.com/pelican-eggs/minecraft/main/egg-at-the-top.yml"
                ),
                (
                    "Power Nukkit X",
                    "bedrock/PowerNukkitX",
                    "https://raw.githubusercontent.com/pelican-eggs/minecraft/main/bedrock/PowerNukkitX/egg-power-nukkit-x.json"
                ),
                (
                    "Paper",
                    "java/paper",
                    "https://raw.githubusercontent.com/pelican-eggs/minecraft/main/java/paper/egg-paper.yaml"
                ),
                (
                    "With space",
                    "odd name",
                    "https://raw.githubusercontent.com/pelican-eggs/minecraft/main/odd%20name/egg-with%20space.json"
                ),
            ]
        );
        assert!(eggs.iter().all(|egg| egg.kind == "minecraft"));
        assert!(eggs_of("minecraft", "not a list").is_err());
        assert!(eggs_of("minecraft", r#"{ "message": "API rate limit exceeded" }"#).is_err());
    }

    #[test]
    fn a_files_name_is_made_a_title_and_a_path_an_address() {
        assert_eq!(title("counter-strike-2"), "Counter Strike 2");
        assert_eq!(title("paper"), "Paper");
        assert_eq!(title("a--b_c"), "A B C");
        assert_eq!(title(""), "");
        assert_eq!(
            in_an_address("java/paper/egg-paper.yaml"),
            "java/paper/egg-paper.yaml"
        );
        assert_eq!(in_an_address("a b/ü?#.json"), "a%20b/%C3%BC%3F%23.json");
    }
}
