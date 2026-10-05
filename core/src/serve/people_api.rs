//! People, groups, clusters and face decisions (phase 5c-2, `people.rs`).
//! Changes go through `change` like every other change to the index (one
//! at a time, backed up afterwards) and ask for the clusters and
//! suggestions to be recomputed in the background. Nothing here reads an
//! original.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, App, Pairs, blocking, change, filter_of, param};
use crate::browse;
use crate::db;
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

#[derive(Serialize)]
pub(super) struct PersonHit {
    id: i64,
    name: String,
    group_id: Option<i64>,
    /// Photos with them, within the filter.
    photos: u64,
}

/// People whose name contains `q`, for the search box: with the photos they
/// are on within the filter (`tag`, `folder`, `person`), most first; people
/// of the filter itself, hidden ones and ones that would show nothing are
/// left out.
pub(super) async fn search(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<PersonHit>>> {
    blocking(&app, move |app| {
        let conn = app.conn.lock().unwrap();
        let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
        let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(8);
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        let snapshot = app.snapshot(&conn)?;
        let shown: HashSet<i64> = snapshot.query(&conn, &filter)?.iter().map(|it| it.id).collect();
        let mut files_per_key: HashMap<String, u64> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, quick_hash FROM files WHERE missing_since IS NULL")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                if shown.contains(&r.get::<_, i64>(0)?) {
                    *files_per_key.entry(r.get(1)?).or_default() += 1;
                }
            }
        }
        let keys = people::keys_by_person(&conn)?;
        let mut hits: Vec<PersonHit> = people::people(&conn, false)?
            .into_iter()
            .filter(|p| !filter.people.contains(&p.id) && db::tag_fold(&p.name).contains(&needle))
            .map(|p| {
                let photos = keys.get(&p.id).into_iter().flatten().map(|k| files_per_key.get(k).copied().unwrap_or(0)).sum();
                PersonHit { id: p.id, name: p.name, group_id: p.group_id, photos }
            })
            .filter(|h| h.photos > 0)
            .collect();
        hits.sort_by(|a, b| b.photos.cmp(&a.photos).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        hits.truncate(limit);
        Ok(Json(hits))
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
    /// `faces` or `pets`: only that kind of cluster (default: both).
    kind: Option<String>,
}

pub(super) async fn clusters(State(app): State<Arc<App>>, Query(q): Query<ClustersQuery>) -> ApiResult<Json<people::Clusters>> {
    let (limit, samples) = (q.limit.unwrap_or(50).min(500), q.samples.unwrap_or(8).min(100));
    let kind = match q.kind.as_deref() {
        None | Some("") | Some("all") => None,
        Some("faces") => Some(crate::pets::Space::Faces),
        Some("pets") => Some(crate::pets::Space::Pets),
        Some(other) => return Err(ApiError::BadRequest(format!("kind is faces or pets, not {other:?}"))),
    };
    blocking(&app, move |app| Ok(Json(people::clusters(&app.conn.lock().unwrap(), q.offset, limit, samples, kind)?))).await
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

/// A cluster that changed since it was shown is refused (409); the reply
/// says what is left of the cluster (its new generation, for a split).
async fn on_cluster(app: Arc<App>, id: i64, req: ClusterRequest, action: fn(Who) -> Action) -> ApiResult<Json<people::Decided>> {
    people_change(&app, move |conn| {
        let all = people::cluster_face_ids(conn, id, req.generation, None)?;
        let faces = people::cluster_face_ids(conn, id, req.generation, req.faces.as_deref())?;
        let mut done = people::decide(conn, &faces, &action(req.who))?;
        let left: Vec<i64> = all.into_iter().filter(|f| !faces.contains(f)).collect();
        done.cluster = Some(if left.is_empty() { None } else { people::cluster_of(conn, &left)? });
        Ok(done)
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

/// Forget that faces are not this person (undo rejections, nothing else).
pub(super) async fn unreject(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<people::Decided>> {
    let person = req.person_id.ok_or_else(|| ApiError::BadRequest("person_id is missing".into()))?;
    on_faces(app, req, Action::Unreject(person)).await
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
    /// A pet (a cat or a dog) rather than a person's face.
    #[serde(default)]
    pet: bool,
    #[serde(flatten)]
    who: Who,
}

/// A face or pet drawn by hand (missed by the detector), with who it is.
pub(super) async fn manual(State(app): State<Arc<App>>, Json(req): Json<ManualRequest>) -> ApiResult<Json<serde_json::Value>> {
    let added = people_change(&app, move |conn| people::add_manual(conn, req.file, req.b, &req.who, req.pet)).await;
    app.request_embed();
    added.map(|id| Json(serde_json::json!({ "manual": id })))
}
