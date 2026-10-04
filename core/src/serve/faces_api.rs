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

pub(super) async fn list(State(app): State<Arc<App>>, Query(q): Query<faces::ListQuery>) -> ApiResult<Json<faces::List>> {
    blocking(&app, move |app| Ok(Json(faces::list(&app.conn.lock().unwrap(), &q)?))).await
}

pub(super) async fn stats(State(app): State<Arc<App>>) -> ApiResult<Json<faces::Stats>> {
    blocking(&app, |app| Ok(Json(faces::stats(&app.conn.lock().unwrap())?))).await
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
        faces::similar(&app.conn.lock().unwrap(), id, limit)?.map(Json).ok_or(ApiError::NotFound)
    })
    .await
}

/// A face's crop: from `thumbs.db`, else made from the original (all faces
/// of the photo from one decode) and stored.
pub(super) async fn crop(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Response> {
    let found = blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let (key, b, file) = faces::face(&conn, id)?.ok_or(ApiError::NotFound)?;
        match faces::load_crop(&conn, &key, &b)? {
            Some(Ok(bytes)) => Ok(Err(jpeg(bytes, IMMUTABLE))),
            Some(Err(_)) => Err(ApiError::NotFound),
            None => {
                let src = file.map(|f| app.source(&conn, f)).transpose()?.flatten();
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

    let _permit = app.renders.acquire().await.map_err(|e| ApiError::Internal(e.into()))?;
    let made = blocking(&app, move |app| {
        // Another request may have made it while this one waited.
        if let Some(stored) = faces::load_crop(&app.conn.lock().unwrap(), &key, &b)? {
            return Ok(stored);
        }
        let boxes = faces::boxes_of(&app.conn.lock().unwrap(), &key)?;
        let result = faces::render_crops(&LibHeif::new(), &src, &boxes);
        if !matches!(&result, Err(e) if thumbs::is_transient(e))
            && let Err(e) = faces::store_crops(&app.conn.lock().unwrap(), &key, &boxes, &result)
        {
            eprintln!("could not store face crops: {e:#}"); // still worth sending
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
