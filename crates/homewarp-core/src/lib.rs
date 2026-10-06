//! Homewarp Core: the home-side panel and API (PLAN.md §5.2).

mod api;
mod auth;
mod db;
mod templates;
mod ui;

pub use api::{AppState, app, openapi};
pub use db::open;
