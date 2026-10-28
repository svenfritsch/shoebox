//! Forgetting what recognition found (`forget.rs`): what it would take, and
//! doing it, for the faces or the cats and dogs.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use super::people_api::faces_change;
use super::{ApiError, ApiResult, App, blocking};
use crate::forget::{self, Forgot};
use crate::pets::Space;

#[derive(Serialize)]
pub(super) struct Counts {
    faces: Forgot,
    pets: Forgot,
}

pub(super) async fn counts(State(app): State<Arc<App>>) -> ApiResult<Json<Counts>> {
    blocking(&app, move |app| {
        let conn = app.read()?;
        Ok(Json(Counts { faces: forget::counts(&conn, Space::Faces)?, pets: forget::counts(&conn, Space::Pets)? }))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct ForgetRequest {
    /// `faces` or `pets`.
    space: String,
    /// Also what the user decided (names, confirmations, drawn boxes).
    #[serde(default)]
    everything: bool,
}

pub(super) async fn forget(State(app): State<Arc<App>>, Json(req): Json<ForgetRequest>) -> ApiResult<Json<Forgot>> {
    let space = match req.space.as_str() {
        "faces" => Space::Faces,
        "pets" => Space::Pets,
        _ => return Err(ApiError::BadRequest("space is faces or pets".into())),
    };
    faces_change(&app, move |conn| forget::forget(conn, space, req.everything)).await.map(Json)
}
