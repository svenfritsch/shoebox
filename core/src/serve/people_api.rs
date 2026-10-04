//! People, groups, clusters and face decisions (phase 5c-2, `people.rs`).
//! Changes go through `change` like every other change to the index (one
//! at a time, backed up afterwards) and ask for the clusters and
//! suggestions to be recomputed in the background. Nothing here reads an
//! original.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use super::{ApiError, ApiResult, App, blocking, change};
use crate::people::{self, Action, Who};

/// Run a change to people or decisions, then recompute the clusters.
async fn people_change<T: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<T> {
    let result = change(app, move |_, conn| f(conn)).await;
    app.request_clusters();
    result
}

// ---------------------------------------------------------------- people

#[derive(Deserialize)]
pub(super) struct PeopleQuery {
    /// Include hidden people.
    hidden: Option<u8>,
}

pub(super) async fn list(State(app): State<Arc<App>>, Query(q): Query<PeopleQuery>) -> ApiResult<Json<Vec<people::Person>>> {
    let hidden = q.hidden.is_some_and(|h| h != 0);
    blocking(&app, move |app| Ok(Json(people::people(&app.conn.lock().unwrap(), hidden)?))).await
}

pub(super) async fn get_person(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<people::Person>> {
    blocking(&app, move |app| people::person(&app.conn.lock().unwrap(), id)?.map(Json).ok_or(ApiError::NotFound)).await
}

#[derive(Deserialize)]
pub(super) struct CreatePerson {
    name: String,
    group_id: Option<i64>,
}

pub(super) async fn create(State(app): State<Arc<App>>, Json(req): Json<CreatePerson>) -> ApiResult<Json<people::Person>> {
    people_change(&app, move |conn| people::create_person(conn, &req.name, req.group_id)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct NameRequest {
    name: String,
}

pub(super) async fn rename(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<NameRequest>,
) -> ApiResult<Json<people::Person>> {
    people_change(&app, move |conn| people::rename_person(conn, id, &req.name)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct MergeRequest {
    /// The person who stays.
    into: i64,
}

pub(super) async fn merge(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<MergeRequest>,
) -> ApiResult<Json<people::Person>> {
    people_change(&app, move |conn| people::merge_people(conn, id, req.into)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct HideRequest {
    hidden: bool,
}

pub(super) async fn hide(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<HideRequest>,
) -> ApiResult<Json<people::Person>> {
    people_change(&app, move |conn| people::hide_person(conn, id, req.hidden)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct GroupRequest {
    /// `null`: no group.
    group_id: Option<i64>,
}

pub(super) async fn set_group(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<GroupRequest>,
) -> ApiResult<Json<people::Person>> {
    people_change(&app, move |conn| people::set_group(conn, id, req.group_id)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct CoverRequest {
    face: i64,
}

pub(super) async fn set_cover(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<CoverRequest>,
) -> ApiResult<Json<people::Person>> {
    people_change(&app, move |conn| people::set_cover(conn, id, req.face)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct PersonFacesQuery {
    /// `confirmed` (default), `suggested`, `maybe` or `rejected`.
    state: Option<people::Which>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

pub(super) async fn person_faces(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Query(q): Query<PersonFacesQuery>,
) -> ApiResult<Json<people::FaceList>> {
    let which = q.state.unwrap_or(people::Which::Confirmed);
    let limit = q.limit.unwrap_or(300).min(2000);
    blocking(&app, move |app| {
        people::person_faces(&app.conn.lock().unwrap(), id, which, q.offset, limit)?.map(Json).ok_or(ApiError::NotFound)
    })
    .await
}

// ---------------------------------------------------------------- groups

pub(super) async fn groups(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<people::Group>>> {
    blocking(&app, |app| Ok(Json(people::groups(&app.conn.lock().unwrap())?))).await
}

pub(super) async fn create_group(State(app): State<Arc<App>>, Json(req): Json<NameRequest>) -> ApiResult<Json<people::Group>> {
    people_change(&app, move |conn| people::create_group(conn, &req.name)).await.map(Json)
}

pub(super) async fn rename_group(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<NameRequest>,
) -> ApiResult<Json<people::Group>> {
    people_change(&app, move |conn| people::rename_group(conn, id, &req.name)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct ReorderRequest {
    ids: Vec<i64>,
}

pub(super) async fn reorder_groups(
    State(app): State<Arc<App>>,
    Json(req): Json<ReorderRequest>,
) -> ApiResult<Json<Vec<people::Group>>> {
    people_change(&app, move |conn| people::reorder_groups(conn, &req.ids)).await.map(Json)
}

/// Its people have no group afterwards.
pub(super) async fn delete_group(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    people_change(&app, move |conn| people::delete_group(conn, id))
        .await
        .map(|n| Json(serde_json::json!({ "people": n })))
}

// ---------------------------------------------------------------- clusters

#[derive(Deserialize)]
pub(super) struct ClustersQuery {
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
    /// Faces shown per cluster.
    samples: Option<usize>,
}

pub(super) async fn clusters(State(app): State<Arc<App>>, Query(q): Query<ClustersQuery>) -> ApiResult<Json<people::Clusters>> {
    let (limit, samples) = (q.limit.unwrap_or(50).min(500), q.samples.unwrap_or(8).min(100));
    blocking(&app, move |app| Ok(Json(people::clusters(&app.conn.lock().unwrap(), q.offset, limit, samples)?))).await
}

#[derive(Deserialize)]
pub(super) struct GenerationQuery {
    generation: Option<i64>,
}

pub(super) async fn cluster_faces(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Query(q): Query<GenerationQuery>,
) -> ApiResult<Json<Vec<people::FaceItem>>> {
    blocking(&app, move |app| match people::cluster_faces(&app.conn.lock().unwrap(), id, q.generation) {
        Ok(faces) => faces.map(Json).ok_or(ApiError::NotFound),
        Err(e) if e.is::<people::Stale>() => Err(ApiError::Conflict(format!("{e}"))),
        Err(e) => Err(e.into()),
    })
    .await
}

/// An action on a cluster: all its faces, or some of them (a split).
#[derive(Deserialize)]
pub(super) struct ClusterRequest {
    /// From the list the cluster was shown in; a 409 if they changed since.
    generation: Option<i64>,
    /// Only these faces of the cluster.
    faces: Option<Vec<i64>>,
    #[serde(flatten)]
    who: Who,
}

async fn on_cluster(app: Arc<App>, id: i64, req: ClusterRequest, action: fn(Who) -> Action) -> ApiResult<Json<people::Decided>> {
    people_change(&app, move |conn| {
        let faces = people::cluster_face_ids(conn, id, req.generation, req.faces.as_deref())?;
        people::decide(conn, &faces, &action(req.who))
    })
    .await
    .map(Json)
}

/// Name the cluster (or some of its faces): confirmed for that person.
pub(super) async fn name_cluster(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<ClusterRequest>,
) -> ApiResult<Json<people::Decided>> {
    on_cluster(app, id, req, Action::Assign).await
}

/// Strangers nobody needs to name.
pub(super) async fn ignore_cluster(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<ClusterRequest>,
) -> ApiResult<Json<people::Decided>> {
    on_cluster(app, id, req, |_| Action::Ignore).await
}

/// False finds.
pub(super) async fn not_face_cluster(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<ClusterRequest>,
) -> ApiResult<Json<people::Decided>> {
    on_cluster(app, id, req, |_| Action::NotFace).await
}

// ---------------------------------------------------------------- faces

#[derive(Deserialize)]
pub(super) struct FacesRequest {
    /// Detected faces (`recog.faces` ids).
    #[serde(default)]
    faces: Vec<i64>,
    /// Undo: hand-drawn faces to delete (their `manual` ids).
    #[serde(default)]
    manual: Vec<i64>,
    /// Reject: the person (else the suggested one).
    person_id: Option<i64>,
    /// Assign: the person, or a name.
    name: Option<String>,
}

async fn on_faces(app: Arc<App>, req: FacesRequest, action: Action) -> ApiResult<Json<people::Decided>> {
    people_change(&app, move |conn| {
        let mut done = people::decide(conn, &req.faces, &action)?;
        if matches!(action, Action::Undo) {
            done.faces += people::delete_manual(conn, &req.manual)?;
        }
        Ok(done)
    })
    .await
    .map(Json)
}

/// Accept what was suggested (or offered as "maybe").
pub(super) async fn confirm(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    on_faces(app, req, Action::Confirm).await
}

/// Not this person.
pub(super) async fn reject(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    let person = req.person_id;
    on_faces(app, req, Action::Reject(person)).await
}

/// This is the person (by id, or by name: made if there is nobody of that name).
pub(super) async fn assign(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    let who = Who { person_id: req.person_id, name: req.name.clone() };
    on_faces(app, req, Action::Assign(who)).await
}

pub(super) async fn ignore(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    on_faces(app, req, Action::Ignore).await
}

pub(super) async fn not_face(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    on_faces(app, req, Action::NotFace).await
}

/// Forget the decisions about faces; hand-drawn faces are deleted.
pub(super) async fn undo(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    on_faces(app, req, Action::Undo).await
}

#[derive(Deserialize)]
pub(super) struct ManualRequest {
    file: i64,
    /// x, y, w, h as fractions of the upright picture.
    #[serde(rename = "box")]
    b: [f64; 4],
    #[serde(flatten)]
    who: Who,
}

/// A face drawn by hand (missed by the detector), with who it is.
pub(super) async fn manual(State(app): State<Arc<App>>, Json(req): Json<ManualRequest>) -> ApiResult<Json<serde_json::Value>> {
    people_change(&app, move |conn| people::add_manual(conn, req.file, req.b, &req.who))
        .await
        .map(|id| Json(serde_json::json!({ "manual": id })))
}
