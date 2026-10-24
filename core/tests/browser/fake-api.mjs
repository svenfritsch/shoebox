// A stand-in for `shoebox serve` for the mocked browser tests (faces-mocked.mjs):
// the page's files come from core/web and core/i18n as they are on disk, the
// library's API from a small model in memory (people, groups, unnamed
// clusters, a person's faces and their photos' info, the search box, the
// status, the trash). Its answers have the shapes of the real server's;
// those are checked by the Rust API tests (core/tests/people.rs: the naming
// replies' `person_data` against the list) and the real-server browser tests
// (faces-e2e.mjs). When the page or the server changes what is asked or
// answered, change the model here too (see "Browser tests" in docs/plan.md).
//
// Like the server (`serve/regroup.rs`), a decision waits for the next
// regrouping: the status says `pending`, with the cards left before it starts
// (5 at first) and the time (30 s; `lib.waitMs` sets what the next status
// says). A merge or a deletion waits only a few seconds; names and groups ask
// for none. The test says when it starts (`lib.step = 'stale'` or `'running'`,
// with `lib.progress` = [done, total]) and that it is done (`lib.grouped()`).
// The event stream is refused (204), so the page follows the grouping by
// asking for the status every 1.5 s (`watchClusters`), on the page's fake clock.
//
// The search box's requests (suggestions and the timeline) are answered from
// the people and `lib.tags`; `lib.hold` (a pattern of paths) keeps the answers
// of matching requests back until `lib.release()`, so a test can type while
// they wait.
//
// A request the model does not know is answered 404 and kept in `lib.unknown`;
// the test fails on it.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../../', import.meta.url)); // core/
export const ORIGIN = 'http://shoebox.test';
const ID = 'lib1';
export const LIB = `/api/lib/${ID}`;
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');
const TYPES = { html: 'text/html', js: 'text/javascript', css: 'text/css', json: 'application/json', svg: 'image/svg+xml', png: 'image/png' };

const fold = (s) => String(s).normalize('NFC').trim().toLowerCase();

export function fakeLibrary() {
  let faceSeq = 0, clusterSeq = 0, personSeq = 0, groupSeq = 0, time = 1_800_000_000;
  const lib = {
    clusters: [], people: [], groups: [],
    faceLists: {}, // person id: { confirmed, suggested, rejected } (a person's page)
    trash: [], // what GET /trash answers (organize::TrashItem)
    dupGroups: [], // what GET /duplicates answers (duplicates::Group)
    photos: null, // [{ id, version, hidden?, folder? }]: the timeline (newest first, one month), else it is empty; `hidden`: 'tag' or 'folder'
    folders: null, // [{ id, parent, name, path, count, hidden?, hidden_own? }], else the root only (hidden folders hide the photos whose `folder` is theirs)
    thumbs: [], // addresses of the thumbnails (and pictures) the page asked for, as written: `/files/7/thumb?v=...`
    step: null, // null (grouped), 'stale' (starting) or 'running'
    progress: [0, 0], // done, total while running
    pending: false, // decisions wait for the next regrouping
    cards: 0, // cards named since the last one started
    waitMs: 30000, // what the status says is left of the pause
    finished: time,
    log: [], // { method, path, query } relative to LIB
    unknown: [],
    open: 0, // requests being answered
    tags: [], // { id, name, count, kind } for /tags
    textLines: 0, // lines of recognized text (`/info`); 0: nothing read yet
    words: {}, // word -> photos whose recognized text has it (`text`)
    fileNames: {}, // part of a file name -> photos (`name`)
    hold: null, // paths (relative to LIB) whose answers wait for `release`
    held: [],
  };
  const face = () => {
    const id = ++faceSeq;
    return { id, file: id, kind: 'png', version: String(id).padStart(8, '0'), x: 0.25, y: 0.25, w: 0.5, h: 0.5, roll: 0, px: 100, score: 0.99, small: false, state: null, species: null, person: null, similarity: null };
  };

  // ---- set up (by the tests)

  // An unnamed cluster of `size` faces; `suggest`: a person it looks like.
  // `species`: 'cat' or 'dog' for a cluster of pets (their faces say it, as the server's do).
  lib.cluster = (size, suggest, species = null) => {
    const c = { id: ++clusterSeq, generation: 1000 + clusterSeq, faces: Array.from({ length: size }, () => ({ ...face(), species })), suggestion: null };
    if (suggest) c.suggestion = { person: { id: suggest.id, name: suggest.name }, faces: size };
    lib.clusters.push(c);
    return c;
  };
  // Someone with `faces` confirmed faces (their picture one of them), in `group`.
  // `species`: a pet ('cat', 'dog' or 'pet'); `suggested`: that many faces
  // suggested for them (most similar first, each on a photo of its own).
  lib.person = (name, { faces = 3, group = null, species = null, suggested = 0 } = {}) => {
    const p = { id: ++personSeq, name, group_id: group ? group.id : null, hidden: false, faces, photos: faces, suggested: 0, maybe: 0, cover: null, cover_manual: null };
    if (species) p.species = species;
    const confirmed = Array.from({ length: faces }, () => ({ ...face(), state: 'confirmed', person: { id: p.id, name } }));
    if (faces) { p.cover = confirmed[0].id; p.last_named = time; }
    lib.people.push(p);
    lib.faceLists[p.id] = { confirmed, suggested: [], rejected: [] };
    for (let i = 0; i < suggested; i++) {
      lib.faceLists[p.id].suggested.push({ ...face(), state: 'suggested', person: { id: p.id, name }, similarity: 0.95 - i * 0.0001 });
    }
    p.suggested = suggested;
    return p;
  };
  // A group (made with its first person, as the app always does).
  lib.group = (name, first) => {
    const g = { id: ++groupSeq, name, position: Math.max(0, ...lib.groups.map((x) => x.position)) + 1 };
    lib.groups.push(g);
    first.group_id = g.id;
    return g;
  };
  // The grouping that followed the last change is done.
  lib.grouped = () => { lib.step = null; lib.finished = ++time; lib.counted = null; };
  // The regrouping starts (what was decided until now goes into it).
  lib.start = (step = 'running') => { lib.step = step; lib.pending = false; lib.cards = 0; };
  // What the sidebar should show (`sidebarIs` in faces-common.mjs).
  // (The number of clusters is the last grouping's, as `/api/info` says.)
  lib.lists = () => ({ groups: groupsOut(), people: peopleOut(), clusters: (lib.counted || counts()).clusters });

  // ---- what the server says

  const members = (g) => lib.people.filter((p) => p.group_id === g.id).length;
  const groupOut = (g) => ({ id: g.id, name: g.name, position: g.position, people: members(g) });
  const groupsOut = () => [...lib.groups].sort((a, b) => a.position - b.position).map(groupOut);
  // `people::people`: by group (none last), then name, then id.
  const peopleOut = () => {
    const rank = (p) => { const g = lib.groups.find((x) => x.id === p.group_id); return g ? g.position : Infinity; };
    return [...lib.people].sort((a, b) => rank(a) - rank(b) || (fold(a.name) < fold(b.name) ? -1 : fold(a.name) > fold(b.name) ? 1 : a.id - b.id)).map((p) => ({ ...p }));
  };
  const large = () => lib.clusters.filter((c) => c.faces.length > 2);
  const small = () => lib.clusters.filter((c) => c.faces.length <= 2);
  const count = (cs) => cs.reduce((n, c) => n + c.faces.length, 0);
  const clusterOut = (c, samples) => ({ id: c.id, generation: c.generation, size: c.faces.length, faces: c.faces.slice(0, samples), suggestion: c.suggestion });
  // What the cache of the last grouping counts (the server's `clusters::overview`).
  const counts = () => ({ clusters: lib.clusters.length, unnamed: count(lib.clusters), suggested: lib.clusters.filter((c) => c.suggestion).length });
  const info = () => ({
    name: 'Faces', version: '0.0.1', photos: 200, videos: 0, missing: 0, thumbs_done: 200, thumbs_total: 200, video_posters: true,
    jobs: [{ kind: 'scan', state: 'done', started_at: 1, updated_at: 1, finished_at: 1, done: 200, total: 200 }],
    busy: false, index_version: '1.0', trash: 0, needs_date: 0, hidden: (lib.photos || []).filter((x) => x.hidden || (x.folder != null && (lib.folders || []).some((f) => f.id === x.folder && f.hidden))).length,
    faces: { done: 200, total: 200, faces: 200, pets_done: 0, pets: 0, running: false },
    clusters: {
      running: lib.step === 'running', done: lib.step === 'running' ? lib.progress[0] : 0, total: lib.step === 'running' ? lib.progress[1] : 0,
      state: lib.step === 'running' ? 'running' : 'done', finished_at: lib.finished,
      ...(lib.counted || counts()), people: lib.people.length, stale: lib.step === 'stale', embedding: false,
      pending: lib.pending, cards_left: lib.pending ? Math.max(0, 5 - lib.cards) : 0, wait_ms: lib.pending ? lib.waitMs : 0, pause_ms: lib.pending ? 30000 : 0,
    },
    text: { done: 0, total: 200, lines: lib.textLines }, reveal: 'Open folder', export: true, read_only: false,
  });

  // ---- changes, as the server makes them

  // Groups nobody is in any more are gone. A card waits for the next
  // regrouping (5 cards, 30 s), a merge or deletion only a few seconds.
  const changed = (regroup) => {
    lib.groups = lib.groups.filter((g) => members(g) > 0);
    if (regroup === 'card') { lib.pending = true; lib.cards++; lib.waitMs = 30000; }
    if (regroup === 'soon') { lib.pending = true; lib.waitMs = Math.min(lib.pending ? lib.waitMs : 3000, 3000); }
  };
  const byId = (id) => lib.people.find((p) => p.id === Number(id));
  const who = (body) => {
    if (body.person_id != null) return byId(body.person_id);
    const named = lib.people.find((p) => fold(p.name) === fold(body.name));
    if (named) return named;
    const p = { id: ++personSeq, name: String(body.name).trim(), group_id: null, species: null, hidden: false, faces: 0, photos: 0, suggested: 0, maybe: 0, cover: null, cover_manual: null };
    lib.people.push(p);
    return p;
  };
  // `faces` (ids) confirmed for whoever `body` names; the reply of the naming
  // endpoints (`people::Decided` with `person_data`).
  const decide = (ids, body) => {
    const p = who(body);
    if (!p) return [400, { error: 'there is no such person' }];
    // The cache stays as it was until the next grouping.
    if (!lib.counted) lib.counted = counts();
    const named = lib.clusters.flatMap((c) => c.faces).filter((f) => ids.includes(f.id));
    (lib.faceLists[p.id] ||= { confirmed: [], suggested: [], rejected: [] }).confirmed.push(...named.map((f) => ({ ...f, state: 'confirmed', person: { id: p.id, name: p.name } })));
    for (const c of lib.clusters) c.faces = c.faces.filter((f) => !ids.includes(f.id));
    lib.clusters = lib.clusters.filter((c) => c.faces.length);
    p.faces += ids.length;
    p.photos += ids.length;
    if (p.cover == null) p.cover = ids[0];
    p.last_named = ++time;
    changed('card');
    return [200, { faces: ids.length, person: { id: p.id, name: p.name }, person_data: { ...p, suggested: 0, maybe: 0 } }];
  };

  const get = {
    '/info': () => info(),
    '/people': () => peopleOut(),
    '/groups': () => groupsOut(),
    '/folders': () => (lib.folders || [{ id: 1, parent: null, name: '', path: '', count: 200 }]).map((f) => ({ hidden: false, hidden_own: false, ...f })),
    '/tags': (q) => lib.tags.filter((t) => fold(t.name).includes(fold(q.get('q') || ''))).slice(0, Number(q.get('limit') || 50)),
    // The search box (no filter here): own tags and people by name and the
    // text row's number; with `date:` only dates (none here), with `text:` or
    // `name:` only that term's number.
    '/suggest': (q) => {
      const typed = q.get('q') || '';
      const none = { tags: [], people: [], pets: [], groups: [], dates: [], text: null, name: null };
      const count = (map, term) => (term ? { term, count: map[fold(term)] || 0 } : null);
      const pre = /^\s*(date|datum|text|name):\s*(.*)$/i.exec(typed);
      if (pre) {
        const kind = pre[1].toLowerCase(), term = pre[2].trim();
        if (kind === 'text') return { ...none, text: count(lib.words, term) };
        if (kind === 'name') return { ...none, name: count(lib.fileNames, term) };
        return none;
      }
      const needle = fold(typed);
      return {
        ...none,
        tags: lib.tags.filter((t) => t.kind !== 'folder' && fold(t.name).includes(needle)).slice(0, 8),
        people: peopleOut().filter((p) => p.photos && fold(p.name).includes(needle)).slice(0, 6)
          .map((p) => ({ id: p.id, name: p.name, group_id: p.group_id, species: p.species || null, cover: p.cover, cover_manual: p.cover_manual, photos: p.photos })),
        text: [...needle].length >= 2 ? count(lib.words, needle) : null,
      };
    },
    '/timeline': (q) => {
      // The server's rule: hidden photos are out, unless the chip asks for them (then only they) or the
      // folder is a hidden one.
      const folder = Number(q.get('folder') || 0);
      const isHidden = (x) => !!x.hidden || (x.folder != null && (lib.folders || []).some((f) => f.id === x.folder && f.hidden));
      const revealed = (x) => folder && x.folder === folder && (lib.folders || []).some((f) => f.id === folder && f.hidden);
      const ph = (lib.photos || []).filter((x) => !folder || x.folder === folder).filter((x) => (q.get('hidden') === '1' ? isHidden(x) : !isHidden(x) || revealed(x)));
      return { count: ph.length, ids: ph.map((x) => x.id), kinds: ph.map((x) => x.kind || 'j').join(''), days: ph.map(() => 20260315), versions: ph.map((x) => x.version).join(''), live: [], tags: [], people: [], groups: [], favs: [], hids: ph.filter(isHidden).map((x) => x.id), est: [] };
    },
    '/places': () => [],
    '/event-pattern': () => ({ pattern: 'YYYY-MM Name' }),
    '/allow-trash': () => ({ allow: false, read_only: false }),
    '/trash': () => lib.trash,
    '/duplicates': () => ({ groups: lib.dupGroups, total_groups: lib.dupGroups.length, hidden_groups: 0 }),
    '/clusters': (q) => {
      const cs = q.get('size') === 'large' ? large() : q.get('size') === 'small' ? small() : lib.clusters;
      const sorted = [...cs].sort((a, b) => b.faces.length - a.faces.length || a.id - b.id);
      const offset = Number(q.get('offset') || 0), limit = Number(q.get('limit') || 50), samples = Number(q.get('samples') || 8);
      return { total: sorted.length, unnamed: count(sorted), small_faces: count(small()), clusters: sorted.slice(offset, offset + limit).map((c) => clusterOut(c, samples)) };
    },
  };
  // A person's faces in one tab (`people::person_faces_in`): `total` and a page.
  const personFaces = (m, q) => {
    const p = byId(m[1]);
    if (!p) return [404, { error: 'no such person' }];
    const list = (lib.faceLists[p.id] || {})[q.get('state')] || [];
    const offset = Number(q.get('offset') || 0), limit = Math.min(Number(q.get('limit') || 300), 2000);
    return [200, { total: list.length, faces: list.slice(offset, offset + limit) }];
  };
  // The viewer's info panel (`FileInfo`) of a photo with one face of a
  // person's lists on it: a suggestion, or one rejected for them.
  const fileInfo = (m) => {
    const id = Number(m[1]);
    if (lib.photos?.some((x) => x.id === id)) {
      return [200, {
        id, name: `IMG_${id}.JPG`, path: `IMG_${id}.JPG`, folder_id: 1, kind: 'jpeg', size: 1000, taken: null, taken_offset: null, estimate: null, exif: null,
        modified: '2026-01-01T10:00:00', width: 100, height: 100, duration_ms: null, camera: null, tags: [], missing: false,
        date_source: null, sort_date: null, live: null, linked: [], faces: [],
        view_turn: 0, screenshot: false, screenshot_mark: null, position: null, text: [],
        hidden: !!lib.photos.find((x) => x.id === id).hidden, hidden_tag: lib.photos.find((x) => x.id === id).hidden === 'tag', hidden_folder: lib.photos.find((x) => x.id === id).hidden === 'folder' ? 'Privat' : null,
      }];
    }
    for (const [pid, l] of Object.entries(lib.faceLists)) {
      const f = [...l.suggested, ...l.rejected].find((x) => x.file === id);
      if (!f) continue;
      const rejected = l.rejected.includes(f) ? [Number(pid)] : [];
      return [200, {
        id, name: `IMG_${id}.PNG`, path: `IMG_${id}.PNG`, folder_id: 1, kind: 'png', size: 1000, taken: null, taken_offset: null, estimate: null, exif: null,
        modified: '2026-01-01T10:00:00', width: 100, height: 100, duration_ms: null, camera: null, tags: [], missing: false,
        date_source: null, sort_date: null, live: null, linked: [], faces: [{ ...f, rejected, landmarks: [] }],
        view_turn: 0, screenshot: false, screenshot_mark: null, position: null, text: [],
      }];
    }
    const d = lib.dupGroups.flatMap((g) => g.files).find((x) => x.id === id);
    if (d) {
      return [200, {
        id, name: d.name, path: d.path, folder_id: d.folder_id, kind: 'jpeg', size: d.size, taken: d.taken, taken_offset: null, estimate: null, exif: null,
        modified: '2026-01-01T10:00:00', width: d.width, height: d.height, duration_ms: null, camera: null, tags: [], missing: false,
        date_source: null, sort_date: null, live: null, linked: [], faces: [],
        view_turn: 0, screenshot: false, screenshot_mark: null, position: null, text: [],
      }];
    }
    return [404, { error: 'no such file' }];
  };
  const getPattern = [
    [/^\/files\/(\d+)$/, fileInfo],
    [/^\/people\/(\d+)$/, (m) => { const p = byId(m[1]); return p ? [200, { ...p }] : [404, { error: 'no such person' }]; }],
    [/^\/people\/(\d+)\/faces$/, personFaces],
    [/^\/clusters\/(\d+)\/faces$/, (m) => { const c = lib.clusters.find((x) => x.id === Number(m[1])); return c ? [200, c.faces] : [404, { error: 'no such cluster' }]; }],
  ];
  const post = [
    // organize::empty_trash: one batch, or the whole trash when `batch` is null.
    [/^\/trash\/empty$/, (m, body) => {
      const gone = lib.trash.filter((t) => body.batch == null || t.batch === body.batch);
      lib.trash = lib.trash.filter((t) => !gone.includes(t));
      return [200, { deleted: gone.length }];
    }],
    // organize::rotate: a JPEG turned in the file has a new quick hash, so a new version.
    [/^\/files\/(\d+)\/rotate$/, (m) => {
      const x = lib.photos?.find((y) => y.id === Number(m[1]));
      if (!x) return [404, { error: 'not in the index' }];
      x.turns = (x.turns || 0) + 1;
      x.version = `rot${String(x.turns).padStart(5, '0')}`;
      return [200, { id: x.id, version: x.version, orientation: 6, view_only: false }];
    }],
    // tags::set_hidden: the own tag `hidden` on or off; `files` is how many changed.
    [/^\/hidden$/, (m, body) => {
      let files = 0;
      for (const x of lib.photos || []) {
        if (!body.ids.includes(x.id)) continue;
        if (body.on && !x.hidden) { x.hidden = 'tag'; files++; } else if (!body.on && x.hidden === 'tag') { x.hidden = false; files++; }
      }
      return [200, { tag: { id: 99, name: 'hidden' }, files, folder: 0 }];
    }],
    // tags::set_folder_hidden: the flag of a folder.
    [/^\/hidden\/folder$/, (m, body) => {
      const f = (lib.folders || []).find((x) => x.path === body.path);
      if (!f) return [400, { error: 'no such folder: ' + body.path }];
      const changed = !!f.hidden_own !== !!body.on;
      f.hidden_own = !!body.on;
      for (const x of lib.folders) x.hidden = lib.folders.some((o) => o.hidden_own && (x.path === o.path || x.path.startsWith(o.path + '/')));
      return [200, { changed }];
    }],
    [/^\/clusters\/(\d+)\/name$/, (m, body) => {
      const c = lib.clusters.find((x) => x.id === Number(m[1]));
      if (!c || (body.generation != null && body.generation !== c.generation)) return [409, { error: 'the faces changed' }];
      const ids = body.faces || c.faces.map((f) => f.id);
      const [status, reply] = decide(ids, body);
      if (status !== 200) return [status, reply];
      const left = lib.clusters.find((x) => x.id === c.id);
      if (left) left.generation += 1000;
      return [200, { ...reply, cluster: left ? { id: left.id, generation: left.generation, size: left.faces.length } : null }];
    }],
    [/^\/faces\/assign$/, (m, body) => decide(body.faces, body)],
    // A suggestion confirmed or rejected (a person's page, the viewer): it
    // leaves the list at once; the other suggestions wait for the grouping.
    [/^\/faces\/(confirm|reject)$/, (m, body) => {
      const ids = body.faces || [];
      let p = null, n = 0;
      for (const [pid, l] of Object.entries(lib.faceLists)) {
        const gone = l.suggested.filter((f) => ids.includes(f.id));
        if (!gone.length) continue;
        p = byId(pid);
        l.suggested = l.suggested.filter((f) => !ids.includes(f.id));
        p.suggested = l.suggested.length;
        if (m[1] === 'confirm') { p.faces += gone.length; p.photos += gone.length; l.confirmed.push(...gone.map((f) => ({ ...f, state: 'confirmed', similarity: null }))); } else l.rejected.push(...gone.map((f) => ({ ...f, state: null, similarity: null })));
        n += gone.length;
      }
      changed('card');
      return [200, { faces: n, person: p && { id: p.id, name: p.name }, ...(p && m[1] === 'confirm' ? { person_data: { ...p, suggested: 0, maybe: 0 } } : {}) }];
    }],
    // The Unnamed page was left: what waits is regrouped soon.
    [/^\/clusters\/left$/, () => { if (lib.pending) changed('soon'); return [200, {}]; }],
    [/^\/people$/, (m, body) => {
      const p = who({ name: body.name });
      if (body.group_id != null) p.group_id = body.group_id;
      changed();
      return [200, p];
    }],
    [/^\/people\/(\d+)\/group$/, (m, body) => { const p = byId(m[1]); p.group_id = body.group_id; changed(); return [200, { ...p }]; }],
    [/^\/people\/(\d+)\/rename$/, (m, body) => { const p = byId(m[1]); p.name = body.name.trim(); changed(); return [200, { ...p }]; }],
    [/^\/people\/(\d+)\/merge$/, (m, body) => {
      const from = byId(m[1]), into = byId(body.into);
      into.faces += from.faces;
      into.photos += from.photos;
      if (into.group_id == null) into.group_id = from.group_id;
      lib.people = lib.people.filter((p) => p !== from);
      changed('soon');
      return [200, { ...into }];
    }],
    [/^\/people\/(\d+)\/delete$/, (m) => {
      const p = byId(m[1]);
      if (p.faces) return [400, { error: 'only someone without faces can be deleted' }];
      lib.people = lib.people.filter((x) => x !== p);
      changed('soon');
      return [200, { deleted: p.id }];
    }],
    [/^\/groups$/, (m, body) => {
      const g = lib.group(body.name.trim(), byId(body.person_id));
      changed();
      return [200, groupOut(g)];
    }],
    [/^\/groups\/reorder$/, (m, body) => {
      const order = [...body.ids, ...lib.groups.map((g) => g.id).filter((id) => !body.ids.includes(id))];
      lib.groups.forEach((g) => { g.position = order.indexOf(g.id) + 1; });
      changed();
      return [200, groupsOut()];
    }],
    [/^\/groups\/(\d+)\/rename$/, (m, body) => { const g = lib.groups.find((x) => x.id === Number(m[1])); g.name = body.name.trim(); changed(); return [200, groupOut(g)]; }],
    [/^\/groups\/(\d+)\/delete$/, (m) => {
      const id = Number(m[1]);
      const left = lib.people.filter((p) => p.group_id === id);
      left.forEach((p) => { p.group_id = null; });
      lib.groups = lib.groups.filter((g) => g.id !== id);
      changed();
      return [200, { people: left.length }];
    }],
  ];

  const json = (route, status, body) => route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });

  // Route everything of ORIGIN to this (`page.route(ORIGIN + '/**', lib.route)`).
  lib.route = async (route) => {
    lib.open++;
    try {
      await answer(route);
    } catch (e) {
      if (!route.held) throw e; // a held request the page called off cannot be answered any more
    } finally {
      lib.open--;
    }
  };
  lib.release = () => {
    const held = lib.held;
    lib.hold = null;
    lib.held = [];
    held.forEach((go) => go());
  };
  const answer = async (route) => {
    const req = route.request();
    const u = new URL(req.url());
    const path = u.pathname;
    if (!path.startsWith('/api/')) {
      const file = path === '/' ? 'web/index.html' : path.startsWith('/i18n/') ? path.slice(1) : 'web' + path;
      // APP_JS=<file>: this app.js instead (to see that a test fails when the page does something else).
      const from = path === '/app.js' && process.env.APP_JS ? process.env.APP_JS : ROOT + file;
      try {
        return route.fulfill({ status: 200, contentType: TYPES[file.split('.').pop()] || 'application/octet-stream', body: readFileSync(from) });
      } catch (e) {
        lib.unknown.push(`GET ${path}`);
        return route.fulfill({ status: 404, body: 'not found' });
      }
    }
    if (path === '/api/libraries') return json(route, 200, [{ id: ID, name: 'Faces', online: true }]);
    if (path === '/api/session') return json(route, 200, { authenticated: true, pin_enabled: false });
    if (path === '/api/all/drives') return json(route, 200, [{ library: ID, name: 'Faces', online: true, role: 'unknown', suggested_backup_of: null, shown_in_all: true, volume: null, folder: null, files: 200, last_scan: 1, last_new_files: 1 }]);
    if (!path.startsWith(LIB + '/')) { lib.unknown.push(`${req.method()} ${path}`); return json(route, 404, { error: 'not in the fake' }); }
    const rel = path.slice(LIB.length);
    if (rel === '/events') return route.fulfill({ status: 204 }); // no stream: the page asks for the status itself
    if (/^\/trash\/\d+\/thumb$/.test(rel)) return route.fulfill({ status: 200, contentType: 'image/png', body: PNG });
    if (/^\/files\/\d+\/(view|thumb)$/.test(rel)) lib.thumbs.push(rel + u.search);
    if (/^\/files\/\d+\/(view|thumb)$/.test(rel)) return route.fulfill({ status: 200, contentType: 'image/png', body: PNG });
    if (/^\/faces\/(manual\/)?\d+\/crop$/.test(rel)) return route.fulfill({ status: 200, contentType: 'image/png', body: PNG });
    // A video no browser can play: junk under a video type.
    if (/^\/files\/\d+\/original$/.test(rel)) return route.fulfill({ status: 200, contentType: 'video/x-msvideo', body: 'not a video the browser knows' });
    lib.log.push({ method: req.method(), path: rel, query: u.searchParams });
    if (lib.hold && lib.hold.test(rel)) {
      route.held = true;
      await new Promise((go) => lib.held.push(go));
    }
    if (req.method() === 'GET') {
      if (get[rel]) return json(route, 200, get[rel](u.searchParams));
      for (const [re, fn] of getPattern) { const m = rel.match(re); if (m) { const [s, b] = fn(m, u.searchParams); return json(route, s, b); } }
    } else if (req.method() === 'POST') {
      const body = req.postDataJSON() || {};
      for (const [re, fn] of post) { const m = rel.match(re); if (m) { const [s, b] = fn(m, body); return json(route, s, b); } }
    }
    lib.unknown.push(`${req.method()} ${rel}`);
    return json(route, 404, { error: 'not in the fake' });
  };
  return lib;
}
