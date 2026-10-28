//! People, groups, clusters and face decisions (phase 5c-2, `people.rs`).
//! Changes go through `change` like every other change to the index (one
//! at a time, backed up afterwards) and say whether, and how soon, the
//! clusters and suggestions should be recomputed in the background
//! (`regroup`). Nothing here reads an original.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, App, IndexVersion, Pairs, Why, blocking, change_with, filter_of, param, queued};
use crate::browse;
use crate::db;
use crate::faces;
use crate::people::{self, Action, Who};

/// Run a change to people or decisions, tidy up what it left behind, then
/// ask for the clusters to be recomputed as `why` says (`None`: the clusters
/// and suggestions do not depend on it, like a name or a group). It changes
/// nothing but people, groups, pictures and face decisions (`faces_change`
/// for the detected faces themselves): the people view (`App::people_view`)
/// reads only those again.
pub(super) async fn people_change<T: Send + 'static>(
    app: &Arc<App>,
    why: Option<Why>,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<T> {
    people_change_then(app, why, move |_, conn| f(conn), |_, _, done| Ok(done)).await
}

/// `people_change` for a change that also touches the detected faces or
/// what they are (forgetting them, a pet's species): regrouped at once.
pub(super) async fn faces_change<T: Send + 'static>(
    app: &Arc<App>,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<T> {
    changed(app, true, |_| Some(Why::Now), move |_, conn| f(conn), |_, _, done| Ok(done)).await
}

/// `people_change`, then `reply` with what the change returned: it sees the
/// people view with the change in it (`change` the one from before).
pub(super) async fn people_change_then<A, T: Send + 'static>(
    app: &Arc<App>,
    why: Option<Why>,
    change: impl FnOnce(&App, &rusqlite::Connection) -> anyhow::Result<A> + Send + 'static,
    reply: impl FnOnce(&App, &rusqlite::Connection, A) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<T> {
    changed(app, false, move |_| why, change, reply).await
}

/// `why` sees what the change returned. A change that failed asks for nothing.
async fn changed<A, T: Send + 'static>(
    app: &Arc<App>,
    faces: bool,
    why: impl FnOnce(&A) -> Option<Why> + Send + 'static,
    change: impl FnOnce(&App, &rusqlite::Connection) -> anyhow::Result<A> + Send + 'static,
    reply: impl FnOnce(&App, &rusqlite::Connection, A) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<T> {
    let result = change_with(app, false, move |app, conn| {
        let done = change(app, conn);
        // Whatever it got to write, also when it failed half-way.
        if faces {
            app.faces_version.fetch_add(1, Ordering::SeqCst);
        }
        app.people_version.fetch_add(1, Ordering::SeqCst);
        let done = done?;
        let why = why(&done);
        // The cards left after a card was decided (`regroup::Schedule::cards_now`).
        let mut cards = None;
        match app.people_view() {
            Ok(v) => {
                tidy_with(conn, &v);
                if why == Some(Why::Card) {
                    cards = Some(people::cards_now(&v));
                }
            }
            Err(e) => eprintln!("could not read the faces: {e:#}"),
        }
        Ok((reply(app, conn, done)?, why, cards))
    })
    .await;
    match result {
        Ok((reply, why, cards)) => {
            if let Some(why) = why {
                app.regroup.ask(why);
            }
            if let Some((cards, suggested)) = cards {
                app.regroup.cards_now(cards, suggested);
            }
            if why.is_some() {
                app.faces_changed();
            }
            Ok(reply)
        }
        Err(e) => Err(e),
    }
}

/// The person left the Unnamed page: decisions that wait are regrouped soon.
pub(super) async fn left_unnamed(State(app): State<Arc<App>>) -> ApiResult<Json<serde_json::Value>> {
    app.regroup(Why::Left);
    Ok(Json(serde_json::json!({})))
}

/// After a change to people or decisions: everyone with confirmed faces has a
/// picture, and faces that are decided lose their stored crops (the old
/// picture of someone who chose a new one included). The change itself is
/// done by now, so a failure here is only reported.
pub(super) fn tidy(conn: &rusqlite::Connection) -> (u64, u64) {
    match people::View::load(conn) {
        Ok(v) => tidy_with(conn, &v),
        Err(e) => {
            eprintln!("could not read the faces: {e:#}");
            (0, 0)
        }
    }
}

/// `tidy` with the faces and decisions of `v` (as they are now). Neither
/// changes what `v` holds: pictures are in `people`, crops in `thumbs.db`.
fn tidy_with(conn: &rusqlite::Connection, v: &people::View) -> (u64, u64) {
    let covers = people::ensure_covers_in(conn, v).unwrap_or_else(|e| {
        eprintln!("could not set people's pictures: {e:#}");
        0
    });
    let pruned = faces::prune_crops_in(conn, &v.m).unwrap_or_else(|e| {
        eprintln!("could not remove face crops: {e:#}");
        0
    });
    (covers, pruned)
}

/// A person's reply after a change, from the view that has it.
fn person_now(app: &App, _: &rusqlite::Connection, id: i64) -> anyhow::Result<people::Person> {
    app.with_people(|conn, v| people::person_in(conn, v, id))?.ok_or_else(|| anyhow::anyhow!("there is no person {id}"))
}

// ---------------------------------------------------------------- people

#[derive(Deserialize)]
pub(super) struct PeopleQuery {
    /// Include hidden people.
    hidden: Option<u8>,
}

pub(super) async fn list(State(app): State<Arc<App>>, Query(q): Query<PeopleQuery>) -> ApiResult<Json<Vec<people::Person>>> {
    let hidden = q.hidden.is_some_and(|h| h != 0);
    blocking(&app, move |app| Ok(Json(app.people(hidden)?))).await
}

pub(super) async fn get_person(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<people::Person>> {
    blocking(&app, move |app| app.with_people(|conn, v| people::person_in(conn, v, id))?.map(Json).ok_or(ApiError::NotFound)).await
}

#[derive(Deserialize)]
pub(super) struct CreatePerson {
    name: String,
    group_id: Option<i64>,
}

pub(super) async fn create(State(app): State<Arc<App>>, Json(req): Json<CreatePerson>) -> ApiResult<Json<people::Person>> {
    people_change_then(&app, None, move |_, conn| people::create_person(conn, &req.name, req.group_id), person_now).await.map(Json)
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
    people_change_then(&app, None, move |_, conn| people::rename_person(conn, id, &req.name), move |app, conn, ()| person_now(app, conn, id))
        .await
        .map(Json)
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
    let into = req.into;
    people_change_then(&app, Some(Why::Soon), move |_, conn| people::merge_people(conn, id, into), move |app, conn, ()| person_now(app, conn, into))
        .await
        .map(Json)
}

pub(super) async fn delete(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    people_change(&app, Some(Why::Soon), move |conn| people::delete_person(conn, id)).await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
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
    people_change_then(&app, None, move |_, conn| people::hide_person(conn, id, req.hidden), move |app, conn, ()| person_now(app, conn, id))
        .await
        .map(Json)
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
    people_change_then(&app, None, move |_, conn| people::set_group(conn, id, req.group_id), move |app, conn, ()| person_now(app, conn, id))
        .await
        .map(Json)
}

/// The picture of a person: one of their confirmed faces, or (`file`) their
/// largest confirmed face in a photo.
#[derive(Deserialize)]
pub(super) struct CoverRequest {
    face: Option<i64>,
    file: Option<i64>,
}

pub(super) async fn set_cover(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<CoverRequest>,
) -> ApiResult<Json<people::Person>> {
    people_change_then(
        &app,
        None,
        move |_, conn| match (req.face, req.file) {
            (Some(face), None) => people::set_cover(conn, id, face),
            (None, Some(file)) => people::set_cover_from_file(conn, id, file),
            _ => anyhow::bail!("send either a face or a file"),
        },
        move |app, conn, ()| person_now(app, conn, id),
    )
    .await
    .map(Json)
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
        app.with_people(|conn, v| people::person_faces_in(conn, v, id, which, q.offset, limit))?.map(Json).ok_or(ApiError::NotFound)
    })
    .await
}

#[derive(Serialize)]
pub(super) struct PersonHit {
    id: i64,
    name: String,
    group_id: Option<i64>,
    /// `cat`, `dog` or `pet` for a pet, else absent.
    species: Option<String>,
    /// Their picture, as in the sidebar.
    cover: Option<i64>,
    cover_manual: Option<i64>,
    /// Photos with them, within the filter.
    photos: u64,
}

/// People whose name contains `q`, for the search box: with the photos they
/// are on within the filter (`tag`, `folder`, `person`), most first; people
/// of the filter itself, hidden ones and ones that would show nothing are
/// left out.
pub(super) async fn search(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<PersonHit>>> {
    queued(&app, move |app| {
        let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
        let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(8);
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        Ok(Json(person_hits(app, &pairs, &filter, None, &needle, limit)?))
    })
    .await
}

/// `search` for `needle` (folded) within `filter`, whose photos are `shown`
/// when the caller has them already (`/api/suggest`). From the kept people
/// view and counts alone: it takes no reader of its own.
pub(super) fn person_hits(
    app: &App,
    pairs: &Pairs,
    filter: &browse::Query,
    shown: Option<&HashSet<i64>>,
    needle: &str,
    limit: usize,
) -> anyhow::Result<Vec<PersonHit>> {
    let list = app.with_people(|conn, v| people::people_in(conn, v, false))?;
    let counts = counts(app, pairs, filter, shown)?;
    let mut hits: Vec<PersonHit> = list
        .into_iter()
        .filter(|p| !filter.people.contains(&p.id) && db::tag_fold(&p.name).contains(needle))
        .map(|p| {
            let photos = counts.people.get(&p.id).copied().unwrap_or(0);
            PersonHit { id: p.id, name: p.name, group_id: p.group_id, species: p.species, cover: p.cover, cover_manual: p.cover_manual, photos }
        })
        .filter(|h| h.photos > 0)
        .collect();
    hits.sort_by(|a, b| b.photos.cmp(&a.photos).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    hits.truncate(limit);
    Ok(hits)
}

/// The photos of each person, group and pet species within a filter, as the
/// search box's lists count them (`search`, `pets_search`, `groups_search`),
/// kept while the index, the people view (it is read again after every
/// decision, group change and recognition) and the filter stay the same: the
/// search box asks with every letter typed, and counting reads every file and
/// every confirmed face (about 0.1 s on 100 000 photos), the pets every face
/// found (about 0.7 s). One reading of the files serves all three; they were
/// counted with a search each (a reading of every file per group and species).
pub(super) struct PersonPhotos {
    version: IndexVersion,
    view: Arc<people::View>,
    /// The request's filter without what is typed (`q`, `limit`), sorted.
    filter: Pairs,
    counts: Arc<Counts>,
    /// The contents of each person, group and pet species: what the counts are
    /// made of, the same for any filter while the view is (a picked chip asks
    /// for new counts, not for new contents).
    keys: Arc<Keys>,
}

struct Keys {
    view: Arc<people::View>,
    of_person: HashMap<i64, HashSet<String>>,
    of_group: Vec<(i64, HashSet<String>)>,
    of_pets: HashMap<String, HashSet<String>>,
}

#[derive(Default)]
pub(super) struct Counts {
    people: HashMap<i64, u64>,
    groups: HashMap<i64, u64>,
    /// By species: `cat`, `dog`, `pet` (any).
    pets: HashMap<String, u64>,
}

/// The view and the snapshot are taken before the reader the counting needs
/// (under this cache's lock, so the search box's requests count once).
fn counts(app: &App, pairs: &Pairs, filter: &browse::Query, shown: Option<&HashSet<i64>>) -> anyhow::Result<Arc<Counts>> {
    let view = app.people_view()?;
    let mut within: Pairs = pairs.iter().filter(|(k, _)| k != "q" && k != "limit").cloned().collect();
    within.sort();
    let mut cache = app.person_photos.lock().unwrap();
    let version = app.version()?;
    if let Some(c) = cache.as_ref()
        && c.version == version
        && Arc::ptr_eq(&c.view, &view)
        && c.filter == within
    {
        return Ok(c.counts.clone());
    }
    let shown = super::shown_by(app, filter, shown)?;
    let conn = app.read()?;
    let conn = &*conn;
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
    let keys = match cache.as_ref() {
        Some(c) if Arc::ptr_eq(&c.keys.view, &view) => c.keys.clone(),
        _ => {
            let of_person = people::keys_by_person(conn)?;
            let mut of_group = Vec::new();
            for g in people::groups(conn)? {
                // Any member: a content with two of them counts once.
                let keys: HashSet<String> = people::members_of_group(conn, g.id)?.iter().filter_map(|p| of_person.get(p)).flatten().cloned().collect();
                of_group.push((g.id, keys));
            }
            Arc::new(Keys { view: view.clone(), of_person, of_group, of_pets: people::keys_of_all_pets(conn)? })
        }
    };
    let of_keys = |keys: &HashSet<String>| -> u64 { keys.iter().map(|k| files_per_key.get(k).copied().unwrap_or(0)).sum() };
    let counts = Counts {
        people: keys.of_person.iter().map(|(person, keys)| (*person, of_keys(keys))).collect(),
        groups: keys.of_group.iter().map(|(group, keys)| (*group, of_keys(keys))).collect(),
        pets: keys.of_pets.iter().map(|(species, keys)| (species.clone(), of_keys(keys))).collect(),
    };
    let counts = Arc::new(counts);
    *cache = Some(PersonPhotos { version, view, filter: within, counts: counts.clone(), keys });
    Ok(counts)
}

/// A pet search term offered while typing.
#[derive(Serialize)]
pub(super) struct PetHit {
    /// `cat`, `dog` or `pet` (any); the value of `pet=` in a search.
    pub(super) species: &'static str,
    /// Photos it shows within the filter of the request.
    pub(super) photos: u64,
}

/// The pet terms ("all cats", "all dogs", "any pet") that fit what is typed
/// (`q`, in English or German) and would show photos within the filter
/// (`tag`, `folder`, `person`, `pet`), with how many, most first. Terms the
/// filter already has are left out.
pub(super) async fn pets_search(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<PetHit>>> {
    queued(&app, move |app| {
        let needle = param(&pairs, "q").unwrap_or("");
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        Ok(Json(pet_hits(app, &pairs, &filter, None, needle)?))
    })
    .await
}

/// `pet_hits` for a filter that is not a request's own (over all drives it is
/// worked out for each drive): a search for each species, nothing kept.
pub(super) fn pet_hits_uncached(app: &App, filter: &browse::Query, needle: &str) -> anyhow::Result<Vec<PetHit>> {
    let snapshot = app.snapshot()?;
    let conn = app.read()?;
    let mut hits = Vec::new();
    for species in crate::pets::species_matching(needle) {
        if filter.pets.iter().any(|p| p == species) {
            continue;
        }
        let mut with = filter.clone();
        with.pets.push(species.to_string());
        let photos = snapshot.query(&conn, &with)?.len() as u64;
        if photos > 0 {
            hits.push(PetHit { species, photos });
        }
    }
    hits.sort_by(|a, b| b.photos.cmp(&a.photos));
    Ok(hits)
}

/// `pets_search` for `needle` (as typed) within `filter`, whose photos are
/// `shown` when the caller has them already (`/api/suggest`).
pub(super) fn pet_hits(
    app: &App,
    pairs: &Pairs,
    filter: &browse::Query,
    shown: Option<&HashSet<i64>>,
    needle: &str,
) -> anyhow::Result<Vec<PetHit>> {
    let counts = counts(app, pairs, filter, shown)?;
    let mut hits = Vec::new();
    for species in crate::pets::species_matching(needle) {
        if filter.pets.iter().any(|p| p == species) {
            continue;
        }
        let photos = counts.pets.get(species).copied().unwrap_or(0);
        if photos > 0 {
            hits.push(PetHit { species, photos });
        }
    }
    hits.sort_by(|a, b| b.photos.cmp(&a.photos));
    Ok(hits)
}

// ---------------------------------------------------------------- groups

pub(super) async fn groups(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<people::Group>>> {
    blocking(&app, |app| Ok(Json(people::groups(&*app.read()?)?))).await
}

#[derive(Deserialize)]
pub(super) struct NewGroupRequest {
    name: String,
    /// The group's first member: a group is never empty.
    person_id: i64,
}

pub(super) async fn create_group(State(app): State<Arc<App>>, Json(req): Json<NewGroupRequest>) -> ApiResult<Json<people::Group>> {
    people_change(&app, None, move |conn| people::create_group(conn, &req.name, req.person_id)).await.map(Json)
}

#[derive(Serialize)]
pub(super) struct GroupHit {
    id: i64,
    name: String,
    people: u64,
    /// Photos with anyone in it, within the filter.
    photos: u64,
}

/// Groups whose name contains `q`, for the search box: a group in the search
/// shows the photos of any of its people. With the photos it would show within
/// the filter (`tag`, `folder`, `person`, `group`, ...), most first; groups of the
/// filter itself and ones that would show nothing are left out.
pub(super) async fn groups_search(State(app): State<Arc<App>>, Query(pairs): Query<Pairs>) -> ApiResult<Json<Vec<GroupHit>>> {
    queued(&app, move |app| {
        let needle = param(&pairs, "q").map(db::tag_fold).unwrap_or_default();
        let limit = param(&pairs, "limit").and_then(|l| l.parse().ok()).unwrap_or(8);
        let filter = browse::Query { text: None, ..filter_of(&pairs)? };
        Ok(Json(group_hits(app, &pairs, &filter, None, &needle, limit)?))
    })
    .await
}

/// `groups_search` for `needle` (folded) within `filter`, whose photos are
/// `shown` when the caller has them already (`/api/suggest`).
pub(super) fn group_hits(
    app: &App,
    pairs: &Pairs,
    filter: &browse::Query,
    shown: Option<&HashSet<i64>>,
    needle: &str,
    limit: usize,
) -> anyhow::Result<Vec<GroupHit>> {
    let counts = counts(app, pairs, filter, shown)?;
    let mut hits = Vec::new();
    let groups = people::groups(&*app.read()?)?;
    for g in groups {
        if filter.groups.contains(&g.id) || !db::tag_fold(&g.name).contains(needle) {
            continue;
        }
        let photos = counts.groups.get(&g.id).copied().unwrap_or(0);
        if photos > 0 {
            hits.push(GroupHit { id: g.id, name: g.name, people: g.people, photos });
        }
    }
    hits.sort_by(|a, b| b.photos.cmp(&a.photos).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    hits.truncate(limit);
    Ok(hits)
}

pub(super) async fn rename_group(
    State(app): State<Arc<App>>,
    Path(id): Path<i64>,
    Json(req): Json<NameRequest>,
) -> ApiResult<Json<people::Group>> {
    people_change(&app, None, move |conn| people::rename_group(conn, id, &req.name)).await.map(Json)
}

#[derive(Deserialize)]
pub(super) struct ReorderRequest {
    ids: Vec<i64>,
}

pub(super) async fn reorder_groups(
    State(app): State<Arc<App>>,
    Json(req): Json<ReorderRequest>,
) -> ApiResult<Json<Vec<people::Group>>> {
    people_change(&app, None, move |conn| people::reorder_groups(conn, &req.ids)).await.map(Json)
}

/// Its people have no group afterwards.
pub(super) async fn delete_group(State(app): State<Arc<App>>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    people_change(&app, None, move |conn| people::delete_group(conn, id))
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
    /// `large` (more than two faces), `small` (one or two, to pick one by
    /// one) or `all` (default).
    size: Option<String>,
}

pub(super) async fn clusters(State(app): State<Arc<App>>, Query(q): Query<ClustersQuery>) -> ApiResult<Json<people::Clusters>> {
    let (limit, samples) = (q.limit.unwrap_or(50).min(500), q.samples.unwrap_or(8).min(100));
    let kind = match q.kind.as_deref() {
        None | Some("") | Some("all") => None,
        Some("faces") => Some(crate::pets::Space::Faces),
        Some("pets") => Some(crate::pets::Space::Pets),
        Some(other) => return Err(ApiError::BadRequest(format!("kind is faces or pets, not {other:?}"))),
    };
    let size = match q.size.as_deref() {
        None | Some("") | Some("all") => people::Size::All,
        Some("large") => people::Size::Large,
        Some("small") => people::Size::Small,
        Some(other) => return Err(ApiError::BadRequest(format!("size is large, small or all, not {other:?}"))),
    };
    blocking(&app, move |app| Ok(Json(app.with_people(|_, v| Ok(people::clusters_in(v, q.offset, limit, samples, kind, size)))?))).await
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
    blocking(&app, move |app| {
        match app.with_people(|_, v| people::cluster_faces_in(v, id, q.generation)) {
            Ok(faces) => faces.map(Json).ok_or(ApiError::NotFound),
            Err(e) if e.is::<people::Stale>() => Err(ApiError::Conflict(format!("{e}"))),
            Err(e) => Err(e.into()),
        }
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
    people_change_then(
        &app,
        Some(Why::Card),
        move |app, conn| {
            let v = app.people_view()?;
            let all = people::cluster_face_ids_in(&v, id, req.generation, None)?;
            let faces = people::cluster_face_ids_in(&v, id, req.generation, req.faces.as_deref())?;
            let done = people::decide_in(conn, &v, &faces, &action(req.who))?;
            let left: Vec<i64> = all.into_iter().filter(|f| !faces.contains(f)).collect();
            Ok((done, left))
        },
        |app, conn, (mut done, left)| {
            done.cluster = Some(if left.is_empty() { None } else { people::cluster_of_in(app.people_view()?.as_ref(), &left) });
            with_person(conn, done)
        },
    )
    .await
    .map(Json)
}

/// The person an action named, as the list shows them, for the reply: the page
/// adds or updates them without asking for the whole list again.
fn with_person(conn: &rusqlite::Connection, mut done: people::Decided) -> anyhow::Result<people::Decided> {
    if let Some(id) = done.person.as_ref().map(|p| p.id) {
        done.person_data = people::person_light(conn, id)?;
    }
    Ok(done)
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
    /// Species: `cat` or `dog`.
    species: Option<String>,
    /// Only faces nobody has decided on (the Unnamed page): if one of them
    /// was named, ignored or marked in the meantime (in the viewer, on
    /// another device), nothing is done and the reply is a 409.
    #[serde(default)]
    open: bool,
}

/// A decision about single faces counts as one card. A rejection only starts
/// the pause again; taking one back, or undoing a decision made before the
/// last regrouping (its face is not in the clusters), only a regrouping shows,
/// so it comes soon. Undoing a newer decision shows at once.
async fn on_faces(app: Arc<App>, req: FacesRequest, action: Action) -> ApiResult<Json<people::Decided>> {
    let why = match action {
        Action::Reject(_) => Why::Pause,
        Action::Unreject(_) => Why::Soon,
        Action::Undo => Why::Pause,
        _ => Why::Card,
    };
    changed(
        &app,
        false,
        |(_, why): &(people::Decided, Why)| Some(*why),
        move |app, conn| {
            let v = app.people_view()?;
            if req.open {
                people::ensure_undecided_in(&v, &req.faces)?;
            }
            let mut done = people::decide_in(conn, &v, &req.faces, &action)?;
            let mut why = why;
            if matches!(action, Action::Undo) {
                // Hand-drawn faces are references for the suggestions.
                let drawn = people::delete_manual(conn, &req.manual)?;
                if drawn > 0 || req.faces.iter().any(|&f| !v.in_clusters(f)) {
                    why = Why::Soon;
                }
                done.faces += drawn;
            }
            Ok((done, why))
        },
        |_, conn, (done, _)| with_person(conn, done),
    )
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

/// These pets are a cat, or a dog, whatever the detector took them for.
pub(super) async fn species(State(app): State<Arc<App>>, Json(req): Json<FacesRequest>) -> ApiResult<Json<serde_json::Value>> {
    let species = req.species.clone().ok_or_else(|| ApiError::BadRequest("species is missing".into()))?;
    faces_change(&app, move |conn| people::set_species(conn, &req.faces, &species))
        .await
        .map(|n| Json(serde_json::json!({ "faces": n })))
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
    /// `cat` or `dog` for a pet rather than a person's face.
    #[serde(default)]
    species: Option<String>,
    /// A pet of no species (older clients): the same as `species: "pet"`.
    #[serde(default)]
    pet: bool,
    #[serde(flatten)]
    who: Who,
}

/// A face or pet drawn by hand (missed by the detector), with who it is.
pub(super) async fn manual(State(app): State<Arc<App>>, Json(req): Json<ManualRequest>) -> ApiResult<Json<serde_json::Value>> {
    let species = req.species.clone().or_else(|| req.pet.then(|| crate::pets::PET.to_string()));
    let added = people_change(&app, None, move |conn| people::add_manual(conn, req.file, req.b, &req.who, species.as_deref())).await;
    app.request_embed();
    added.map(|id| Json(serde_json::json!({ "manual": id })))
}
