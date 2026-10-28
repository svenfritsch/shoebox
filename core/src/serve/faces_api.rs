//! The face check page's API (phase 5c-1, `faces.rs`): the list of faces,
//! a summary, crops and nearest neighbours. Only reads originals: crops are
//! made under the guard and cached in `thumbs.db`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use libheif_rs::LibHeif;
use serde::Deserialize;

use super::{ApiError, ApiResult, App, IMMUTABLE, blocking, jpeg};
use crate::classify::Kind;
use crate::faces;
use crate::thumbs;

/// The Face check's requests answer from the kept people view
/// (`App::people_view`, read on a connection of its own); only what the view
/// does not hold is read on a reader, taken after the view is there. Their
/// parameters are checked first: a request refused never waits for the view.
pub(super) async fn list(State(app): State<Arc<App>>, Query(q): Query<faces::ListQuery>) -> ApiResult<Json<faces::List>> {
    let space = faces::space_of(q.kind.as_deref()).map_err(ApiError::BadRequest)?;
    let sort = faces::sort_of(q.sort.as_deref()).map_err(ApiError::BadRequest)?;
    blocking(&app, move |app| Ok(Json(faces::list_in(&*app.people_view()?, &q, space, sort)))).await
}

#[derive(Deserialize)]
pub(super) struct StatsQuery {
    /// `faces` (default) or `pets`.
    kind: Option<String>,
}

pub(super) async fn stats(State(app): State<Arc<App>>, Query(q): Query<StatsQuery>) -> ApiResult<Json<faces::Stats>> {
    let space = faces::space_of(q.kind.as_deref()).map_err(ApiError::BadRequest)?;
    blocking(&app, move |app| {
        let view = app.people_view()?;
        Ok(Json(faces::stats_in(&*app.read()?, &view, space)?))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct SimilarQuery {
    limit: Option<usize>,
}

pub(super) async fn similar(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Query(q): Query<SimilarQuery>,
) -> ApiResult<Json<Vec<faces::Neighbour>>> {
    let limit = q.limit.unwrap_or(24).min(200);
    blocking(&app, move |app| {
        // A face it does not know is not found before the view is read.
        let target = faces::target(&*app.read()?, id)?.ok_or(ApiError::NotFound)?;
        let view = app.people_view()?;
        Ok(Json(faces::similar(&*app.read()?, &view, &target, limit)?))
    })
    .await
}

/// A face's crop: from `thumbs.db`, else made from the original (all faces
/// of the photo from one decode) and stored if the face still needs one
/// (`faces::wanted`).
pub(super) async fn crop(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Response> {
    crop_of(app, move |conn| faces::face(conn, id)).await
}

/// The crop of a face drawn by hand (its `manual` id).
pub(super) async fn manual_crop(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Response> {
    crop_of(app, move |conn| faces::drawn_face(conn, id)).await
}

async fn crop_of(
    app: Arc<App>,
    find: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<Option<(String, faces::FaceBox, Option<i64>)>> + Send + 'static,
) -> ApiResult<Response> {
    let found = blocking(&app, move |app| {
        let (key, b, file, stored) = {
            let conn = app.read()?;
            let (key, b, file) = find(&conn)?.ok_or(ApiError::NotFound)?;
            let stored = faces::load_crop(&conn, &key, &b)?;
            (key, b, file, stored)
        };
        match stored {
            Some(Ok(bytes)) => Ok(Err(jpeg(bytes, IMMUTABLE))),
            Some(Err(_)) => Err(ApiError::NotFound),
            None => {
                let src = file.map(|f| app.source(f)).transpose()?.flatten();
                let src = src.filter(|s| s.kind != Kind::Video && s.kind != Kind::Raw).ok_or(ApiError::NotFound)?;
                Ok(Ok((key, b, src)))
            }
        }
    })
    .await?;
    let (key, b, src) = match found {
        Ok(todo) => todo,
        Err(stored) => return Ok(stored),
    };

    let _permit = super::readers::in_line(app.renders.acquire()).await.map_err(|e| ApiError::Internal(e.into()))?;
    let made = blocking(&app, move |app| {
        // Another request may have made it while this one waited.
        let boxes = {
            let conn = app.read()?;
            if let Some(stored) = faces::load_crop(&conn, &key, &b)? {
                return Ok(stored);
            }
            faces::boxes_of(&conn, &key)?
        };
        let result = faces::render_crops(&LibHeif::new(), &src, &boxes);
        if !matches!(&result, Err(e) if thumbs::is_transient(e)) {
            // Only faces still waiting for a decision and people's pictures
            // are kept; a decided face is made again whenever it is asked for.
            // Not while a change holds the writer: then it is made again next time.
            let stored = app.store_now(|conn| faces::wanted(conn, &key, &boxes).and_then(|wanted| faces::store_crops(conn, &key, &boxes, &wanted, &result)));
            if let Some(Err(e)) = stored {
                eprintln!("could not store face crops: {e:#}"); // still worth sending
            }
        }
        Ok(result.and_then(|crops| {
            boxes.iter().position(|x| *x == b).and_then(|i| crops.into_iter().nth(i)).ok_or_else(|| "face gone".into())
        }))
    })
    .await?;
    match made {
        Ok(bytes) => Ok(jpeg(bytes, IMMUTABLE)),
        Err(_) => Err(ApiError::NotFound),
    }
}
