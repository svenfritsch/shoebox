//! The API of the text in photos (phase 9, `text.rs`): hiding a line, the
//! limits that decide which lines count, a summary, and forgetting it all.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::{ApiResult, App, blocking, change};
use crate::text;

#[derive(Deserialize)]
pub(super) struct HideRequest {
    /// The line's folded text (`norm` of a line in the photo's details).
    norm: String,
    hidden: bool,
}

#[derive(Serialize)]
pub(super) struct Hidden {
    hidden: bool,
}

/// Hide a line of recognized text from the search of this photo, or show it
/// again. The decision is the user's (by content, in `library.db`).
pub(super) async fn hide(State(app): State<Arc<App>>, Path(id): Path<i64>, Json(req): Json<HideRequest>) -> ApiResult<Json<Hidden>> {
    change(&app, move |_, conn| {
        let key: String = conn
            .query_row("SELECT quick_hash FROM files WHERE id = ?1 AND missing_since IS NULL", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("no such photo"))?;
        text::set_hidden(conn, &key, &req.norm, req.hidden)?;
        Ok(Hidden { hidden: req.hidden })
    })
    .await
    .map(Json)
}

#[derive(Serialize)]
pub(super) struct Summary {
    #[serde(flatten)]
    stats: text::Stats,
}

pub(super) async fn stats(State(app): State<Arc<App>>) -> ApiResult<Json<Summary>> {
    blocking(&app, move |app| Ok(Json(Summary { stats: text::stats(&app.conn.lock().unwrap())? }))).await
}

#[derive(Deserialize)]
pub(super) struct LimitsRequest {
    /// Both are sent; one that is missing goes back to its default.
    min_score: Option<f64>,
    min_height: Option<f64>,
}

pub(super) async fn limits_get(State(app): State<Arc<App>>) -> ApiResult<Json<text::Limits>> {
    blocking(&app, move |app| Ok(Json(text::limits(&app.conn.lock().unwrap())))).await
}

pub(super) async fn limits_set(State(app): State<Arc<App>>, Json(req): Json<LimitsRequest>) -> ApiResult<Json<text::Limits>> {
    change(&app, move |_, conn| text::set_limits(conn, req.min_score, req.min_height)).await.map(Json)
}

#[derive(Serialize)]
pub(super) struct Deleted {
    lines: u64,
}

/// Forget all recognized text (the lines and the memory of having read each
/// photo). The hidden lines stay: they are decisions. Photos are read again
/// by the next "Recognize text".
pub(super) async fn delete_all(State(app): State<Arc<App>>) -> ApiResult<Json<Deleted>> {
    change(&app, move |_, conn| Ok(Deleted { lines: text::delete_all(conn)? })).await.map(Json)
}

