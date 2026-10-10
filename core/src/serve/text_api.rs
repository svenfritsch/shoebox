//! The API of the text in photos (phase 9, `text.rs`): hiding a line, the
//! limits that decide which lines count, a summary, and forgetting it all.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use libheif_rs::LibHeif;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, App, IMMUTABLE, blocking, change, jpeg};
use crate::classify::Kind;
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


#[derive(Serialize)]
pub(super) struct Check {
    limits: text::Limits,
    lines: Vec<text::CheckLine>,
}

/// The Text check page: the limits, and the lines near them (what moving a
/// limit would take in or leave out), nearest first.
pub(super) async fn check(State(app): State<Arc<App>>) -> ApiResult<Json<Check>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        Ok(Json(Check { limits: text::limits(&conn), lines: text::check_lines(&conn, 60)? }))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct CropQuery {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// A crop of one line of text, cut from the original under the guard (a
/// photo is read for it, nothing is stored).
pub(super) async fn crop(State(app): State<Arc<App>>, Path(id): Path<i64>, Query(q): Query<CropQuery>) -> ApiResult<Response> {
    let valid = [q.x, q.y, q.w, q.h].iter().all(|v| v.is_finite()) && q.w > 0.0 && q.h > 0.0 && q.x >= 0.0 && q.y >= 0.0 && q.x + q.w <= 1.001 && q.y + q.h <= 1.001;
    if !valid {
        return Err(ApiError::BadRequest("the box is x, y, w, h as fractions of the picture".into()));
    }
    let src = blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        app.source(&conn, id)?.filter(|s| s.kind != Kind::Video && s.kind != Kind::Raw).ok_or(ApiError::NotFound)
    })
    .await?;
    let _permit = app.renders.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
    let made = blocking(&app, move |_| Ok(text::render_crop(&LibHeif::new(), &src, [q.x, q.y, q.w, q.h]))).await?;
    match made {
        Ok(bytes) => Ok(jpeg(bytes, IMMUTABLE)),
        Err(_) => Err(ApiError::NotFound),
    }
}
