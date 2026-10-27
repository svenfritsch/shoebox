# Phase 21: Responsive on large libraries (performance plan)

**Plan only, nothing built.** On a library of about 100,000 photos and 299,000
faces, the Face check page's request
`GET /api/lib/<id>/faces?sort=size&offset=0&limit=300&unmatched=true` took
about 2.5 minutes. While it ran, the rest of the photo app did not answer, and
leaving the page did not stop it. This plan says why, and how to make one
slow thing never freeze the app again. It looks again at older decisions
(one connection per library, no WAL, HTTP/1.1) and lists what each step buys
and what can go wrong.

## 1. What happened

Four things came together. The first made one page slow; the other three
turned that into a frozen app.

1. **A query that grows with the square of the library** (`faces::list`).
   The filter "Not matched yet" was written into the SQL as
   `f.id IN (<every unmatched face id>)`, up to ~100,000 numbers. With more
   than a few hundred numbers SQLite takes the list as part of the lookup in
   the index by content (`faces_key`, whose last column is the row id): for
   every present photo it looks up every number of the list. Measured below:
   400 ids 9.6 s, 2,000 ids 65 s, all of them over 10 minutes. Below about
   100 ids SQLite picks another plan, so small test libraries never show it.
   On top, the request read every face and decision twice (`Matched::load`,
   `View::load`), and the page's first request, `/faces/stats`, reads them
   once more (`View::load`), all on the shared connection.
2. **One connection for everything.** Most handlers lock `App::conn`
   (one `Mutex<Connection>` per library, `library.db` with `thumbs.db` and
   `recognition.db` attached). While one request holds it, every other one
   (thumbnails, crops, `/api/info`, the sidebar) waits. Nothing is wrong
   with any of those; they are behind the one that is slow. Changes hold it
   too, for as long as they take: a move of 5,000 photos renames and commits
   group by group (`organize::move_group`, a synced commit each on exFAT), and
   nothing can be read meanwhile.
3. **Nothing is called off.** The page keeps its request when it is left
   (only the timeline and the search box use an `AbortController`). And even
   when the browser closes the connection, the server only drops the async
   part of the handler: the work it gave to a blocking thread
   (`spawn_blocking`) runs to its end and keeps the connection locked.
4. **The page says nothing while it waits.** No sign that it is working, so
   it looks broken.

And one condition of the browser: over HTTP/1.1 it sends at most **six
requests at a time** to one server, for pictures and API calls together. The
event stream (`/api/events`) keeps one of them open for good, in every tab.
Requests that hang hold the others, so pictures stop loading too.

## 2. Measured (benchmark library)

`scripts/bench-people.py make` (100,000 files, 298,755 faces, 129,797
decisions, 300 people), release build, 4-core Linux VM with an SSD; the real
drive is slower. "Probe" is a client that asks for `/api/groups` (a trivial
query) every 100 ms meanwhile: its longest wait is how long the rest of the
app was frozen.

| Request | Took | Probe's longest wait |
|---|---|---|
| Face check, "Not matched yet" (`/faces?…&unmatched=true`), on the real drive | about 150 s | as long (the whole app) |
| Pet check, "Not matched yet" (1,519 pets, all unmatched) | 33.4 s | 33.4 s |
| `/faces/stats` (the Face check's first request) | 1.72 s | 1.72 s |
| `/faces/stats?kind=pets` | 1.26 s | 1.25 s |
| `/faces?sort=size&limit=300` (all faces) | 1.46 s | 1.45 s |
| `/faces?…&not_face=true` (54) / `pet_face=true` (30) / `rotated=true` | 0.71 / 0.79 / 0.88 s | as long |
| `/faces/{id}/similar?limit=24` | 1.42 s | 1.42 s |
| `/suggest?q=a`, first after the start | 2.20 s | 2.20 s |
| `/people`, first (reads the people view **on its own connection**) | 1.01 s | 0.00 s |
| `/duplicates` 0.28 s, `/info` 0.10 s, `/timeline` 0.02 s, `/folders` 0.00 s | | as long |

The probe waits exactly as long as each request holds `App::conn`, every
time; the one request that reads on a connection of its own (`/people`)
holds up nobody. That is the whole case for step 5 in one table.

The SQL alone (SQLite 3.45 in Python; the program bundles 3.53, which the
reproduction on the real program showed to behave the same), the Face check
query with the "unmatched" ids written into it, against the same ids passed
as one set:

| ids in the list | `f.id IN (1,2,…)` | `IN (SELECT value FROM json_each(?1))` |
|---|---|---|
| 100 | 0.07 s | 0.10 s |
| 400 | 9.6 s | 0.07 s |
| 2,000 | ~65 s (the program) | 0.09 s |
| 100,000 | over 10 min (the program) | 0.53 s |

`EXPLAIN QUERY PLAN` of the slow case: `SCAN p` (the present photos), then
`SEARCH f USING INDEX faces_key (key=? AND rowid=?)`: one seek per photo and
id. With the set, SQLite looks up each id once by row id.

The plan also depends on the SQLite version: the Pet check's list (1,519
ids) took 0.85 s in SQLite 3.45 and 33 s in the program (3.53). A query whose
speed hangs on the planner's guess about a list of numbers is not safe in any
version; hence the rule in step 1.

## 3. Goals

- **G1 No request reads the whole library.** Everything that needs every face
  or every file is read once into a kept view (`Snapshot`, `people::View`)
  and answered from memory. Budget on the benchmark library: no request's
  database work over 1 s, most under 50 ms.
- **G2 A slow request does not hold up the others.** The probe's longest
  wait stays under 100 ms while any one request runs.
- **G3 Leaving a page calls off what it asked for**, in the browser and on the
  server: the server never works on for a client that is gone.
- **G4 A runaway query is stopped and named**, not left to freeze the app:
  a deadline per request, and a log line with the request and its times.
- **G5 Anything slower than 0.4 s shows that it is working.**
- **G6 It stays measured:** a bench script over every request the pages make,
  run before a release.

## 4. Steps

Each step is a pull request of its own and helps alone; the order is by what
it fixes per effort. Steps 1 to 3 remove the incident and its kind; step 5 is
the structural change, worth it only on top of them (see the rollback journal
under "Pitfalls").

| Step | What | Effort | Fixes |
|---|---|---|---|
| 1 | Face check from the kept people view; no id lists in SQL | S | the 2.5 minutes (G1) |
| 2 | Slow-request log, `Server-Timing`, `scripts/bench-endpoints.py` | S | finding the next one (G4, G6) |
| 3 | Calling off: the page's requests, the server's work, a deadline | M | G3, G4 |
| 4 | Loading states: the busy line, placeholders, "still working", retry | S–M | G5 |
| 5 | Connection lanes: one writer, a pool of readers, kept views on their own connections | L | G2 |
| 6 | Start-up order, a kept view while `recognize` runs, warming up (measure first) | S each | first click, long runs |

### Step 1: the Face check from the kept people view

The people view (`App::people_view`, `people::View`) already holds every
detected face with its content, box, width (`px`), score, roll and species,
the decisions matched to them, the present files, the faces taken for a pet's
and the clusters; it is read on a connection of its own and kept until
something it was read from changes. Everything the Face check asks for can be
answered from it.

- `faces::list_in(view, query)`: the faces of the space whose content is
  present and was looked at (as the `JOIN recog.looked` and `JOIN PRESENT`
  do now), filtered in Rust: `unmatched` is `View::unplaced_ids` (kept in the
  view, `OnceLock`, like the counts), `pet_face` the view's pet faces,
  `not_face` the faces decided "not a face", the rest leaves both out; sizes,
  `rotated`; sorted by size or score, then id; `total`, then the page.
  Sorting 300,000 numbers takes milliseconds; if a measurement says it
  matters, keep the sorted order for the last query, keyed by the view
  (`Arc::ptr_eq`), so "Show more" only cuts a page.
- `faces::stats_in(conn, view, space)`: the buckets from the view's faces
  instead of `View::load` and a join over all faces; the few counts over
  `recog.looked` (photos looked at, errors, runs) stay SQL.
- `faces::similar`: which faces to leave out from the view instead of
  `Matched::load`; reading the embeddings stays (a calibration tool, rarely
  used; keeping them in memory was decided against, 200 MB), but on a
  read-only connection of its own (`faces::open_readonly`), never on
  `App::conn`.
- The handlers take the view first (`app.people_view()`, an `Arc`), then the
  connection for the little SQL that is left.
- The same rule everywhere: **no list of ids written into SQL.**
  `browse::all_tags_shown` writes the hidden photos and folders as
  `NOT IN (…)`; `NOT IN` cannot drive a lookup, so it is not slow today, but it
  goes to `json_each` too, so nobody copies the pattern.

Tests (`core/tests/people.rs`, next to the pets' `unmatched` test): for each
kind and filter the listed ids are exactly the expected set and `total` equals
the stats' number (`unmatched`, `not_faces`, `pet_faces`, `small`); sort
orders with ties by id; pages joined equal one large page, an offset past the
end gives `total` and no faces; a decision made in between is seen at once
(marked "not a face": gone from all, there in `not_face`; named: gone from
`unmatched`); an unknown `kind` refused. The speed itself is not a test (a time
limit in CI is flaky): `bench-people.py run` gets the Face check requests
(stats, each filter, `similar`), alone and under the probe.

### Step 2: seeing where the time goes

- **Slow-request log.** Every library request that takes over 1 s
  (`SHOEBOX_SLOW_MS`) prints one line: method, path with query, total time,
  time spent waiting for the connection and time holding it. The incident
  would have printed `GET …/faces?…unmatched=true 152 s (held the database
  152 s)` and, for each request behind it, how long it waited.
- **`Server-Timing` header** with the same parts (`wait`, `db`, `total`), so
  the browser's network panel shows them on the real drive, also on the iPad
  through Safari's inspector.
- **`scripts/bench-endpoints.py`** on the library of `bench-people.py make`
  (with the tags of `bench-search.py`): every GET the pages make (status,
  folders, tags, suggestions, timeline, people, groups, clusters, a person's
  faces, Face and Pet check, duplicates, places, map points, trash, text
  check, a file's details, a page of thumbnails and crops), each cold and
  warm, and once more under the probe. Prints a table; `--budget` fails when
  a request takes over 1 s warm or the probe waits over 0.5 s. Not in CI
  (making the library and its first grouping take minutes); run by hand
  before a release, the numbers go into this file.

### Step 3: calling off

In the page:

- Every page gets an `AbortController` (`pageSignal()`): showing another
  page or another Face check filter aborts the old one. The requests a page
  makes for itself go with it (`pageApi(path)`); those of the whole app (the
  status, the people list, folders, drives) do not. The timeline keeps its
  own (`state.loadCtrl`), the search box its own (`sugg.ctrl`).
- An abort is not an error: `failed` opens a dialog, so every `catch` that
  reaches it lets `AbortError` pass silently, or leaving a page would show
  "That did not work".
- `dropPending` already stops the pictures of a page that is left.

On the server:

- When the browser closes a connection, hyper drops the handler's future
  (`mid_message_detect_eof` in hyper 1.11; the search's `in_turn` relies on
  it already). `blocking` gets a token that a guard in that future sets when
  it is dropped before the work is done.
- A request whose client left while it waited for the connection does not
  start at all: the token is looked at once the connection is taken. Today
  such requests sit in blocking threads, each waiting for the lock, and all
  run one after the other once the slow one is done, for nobody.
- While a request holds a connection, a SQLite progress handler (every
  10,000 steps of SQLite's machine, so it costs nothing) stops the statement
  (`SQLITE_INTERRUPT`) when the token is set or the request's **deadline**
  (20 s) has passed. Long loops in Rust look at the token too. Works on the
  one shared connection already (set when it is locked, removed when it is
  given back), so it does not wait for step 5.
- A request stopped by its deadline answers 503 with a text saying it took
  too long and was stopped so the rest keeps working; step 2's log names it.
- **Never a change.** Writes run to their end: a half-done move is worse than
  a slow one (`change_with` installs no handler).
- **Never the building of a kept view** (snapshot, people view, tags,
  duplicates): other requests wait for the same view; it is built without
  the handler (in step 5 on its own connection), and a request that leaves
  meanwhile leaves the build running for the next one.

Tests: Rust API tests need a request that is slow on purpose. A route only a
test can switch on (`serve::Options::test_routes`, not reachable from the
command line) runs SQL that counts until it is stopped. With it: a client
that goes away after 100 ms frees the connection (`/api/groups` answers long
before the deadline); the deadline answers 503 with the message; a change is
not stopped by a client that leaves. In the browser (`faces-mocked.mjs`, fake
clock, `lib.hold`): Face check held, the People page opened: the held request
is aborted, no dialog; another filter aborts the old list; a reply that comes
after the page was left draws nothing.

### Step 4: showing that it works

- **The busy line**: a thin animated line under the top bar (new common
  class `.busy-line` in the common block of `app.css`, listed in CLAUDE.md;
  no motion with `prefers-reduced-motion`), shown when one of the page's
  requests (`pageApi`) has run 0.4 s, gone when they are done or the page is
  left; `aria-busy` on the page. Not for pictures: they have their grey tile.
  One line for the whole app, so it looks the same on every page.
- **Placeholders**: a list page whose first answer has not come shows grey
  tiles (the look of the search box's `.wait-row`, made common as
  `.wait-tile`), as the search box does after 120 ms.
- **Still working**: after 10 s a line on the page, "Still working. On a
  large library this can take a while." (`app.still_working`).
- **Stopped**: a 503 from the deadline shows its text and a "Try again"
  button (`.btn`) in the page, not the error dialog.
- Texts in `en.json` and `de.json`; the guide gets one sentence in "Finding
  your way around" (both languages): the line under the top bar while a page
  loads, and that leaving a page stops what it was loading.

Tests (`faces-mocked.mjs`, fake clock): no line for an answer within 0.4 s;
the line at 0.4 s and gone with the answer; gone at once when the page is
left; placeholders and their removal; the 10 s line; "Try again" after a 503
asks again; the fake answers 503 like the server.

### Step 5: connection lanes

| Lane | Connection | Used by |
|---|---|---|
| Writer | one (`App::writer`, the old `App::conn`): `library.db` + `thumbs.db` + `recognition.db` | `change`, `change_with`; the source of `index_version` |
| Readers | a pool of 2 to 4 (by cores), `PRAGMA query_only`, the same three attached | every request's own reads |
| Cache writer | one, writes only `thumbs.db` | thumbnails, crops and video posters made on request |
| Kept views | their own: the people view's (as now) and one for the file views | `Snapshot`, tags, duplicates, copies, the status counts |
| Background | their own, as now | healer, backup, regrouping, embedding |

Why the cache writer is its own: a thumbnail made while someone moves 5,000
photos should not wait for the move; the two write different files, so they
never lock each other.

Rules:

- A handler takes the kept views it needs first (an `Arc`), then borrows a
  reader for its own reads, and gives it back before it waits for anything
  else. Four requests waiting for the people view while each holds a reader
  would leave none for the rest.
- Only the writer writes: readers are `query_only`, so a write in the wrong
  lane fails in the tests instead of locking in production.
- **`index_version` stays what it is.** It is `(PRAGMA data_version,
  generation)` today and relies on two things: `data_version` ignores the
  connection's own commits, and changes to people do not move `generation`,
  so naming faces does not reload every open page. `data_version` belongs to
  one connection: numbers from two connections cannot be compared, so caches
  keyed by it would be thrown away all the time with a pool. It stays on the
  writer: there it still sees every other commit (a scan in another process,
  the healer) and none of its own. Readers ask for it with `try_lock`; while a
  change holds the writer they use the last number read (the change moves
  `generation` itself when it is done). The same for `recog.data_version` in
  the status.
- The writer starts its changes with `BEGIN IMMEDIATE`, so it waits for the
  write lock (the busy timeout) instead of failing at once when a scan or the
  regrouping writes at the same moment.

Order: 5a the pool and its guard (`app.read()`), the heavy reads move first
(Face check, people search, suggestions, timeline, duplicates, status,
thumbnails and crops with the cache writer); 5b the rest, and `App::conn`
becomes `App::writer`, so a read left on it no longer compiles by accident;
5c the file views on their own connection.

Tests: while the test route of step 3 holds a reader, `/api/groups`, a
thumbnail, `/api/info` and a change answer before it ends (no time limit:
only that they are not behind it); for every kept view, a change through the
API **and** a commit by another connection (a scan) are seen at once; a write
on a reader fails. The browser tests against the real server
(`keyboard.mjs`, `faces-e2e.mjs`) as they are. `bench-endpoints.py`: the
probe's longest wait under the heaviest request.

### Step 6: start-up and long runs (measure first)

- **Start-up order.** The page asks for the timeline only when folders, the
  status, own tags, people, groups and places have all come (`Promise.all`
  before `applyFilter`): the photos wait for the slowest of the sidebar
  (people: the whole people view the first time). Ask for the timeline at
  once and fill the sidebar as each part comes. Eight requests and the event
  stream at start are more than the six the browser sends at a time; the
  timeline should be among the first.
- **While `shoebox recognize` runs** in another process, every commit to
  `recognition.db` makes the next people request read the whole view again
  (about 1.2 s on the benchmark library, more on a drive). Keep the view for
  a few seconds while a recognize job runs, as `/api/info` keeps its counts.
- **Warming up.** Build the snapshot, the tags and (with faces) the people
  view in the background once the first timeline has answered, so the first
  click on People or Face check does not pay for it. Not before: on a cold
  USB drive it would compete with the first page for the disk.
- **Crops of decided faces on the Face check.** The page asks
  `/faces/{id}/crop` for every face; a face with a decision has no kept crop
  (phase 5e), so filters like "Not a face" decode the original for every
  crop (HEIC: a few hundred ms each, holding one of the six connections). The
  other pages cut such faces out of the photo's thumbnail in the browser
  (`zoomFace`); the face list would need the box (`x`, `y`, `w`, `h`) for
  that.
- **Pictures and the pool.** If a page of thumbnails keeps every reader
  busy, keep one reader for requests that are not pictures.

## 5. Older decisions, looked at again

| Decision | Why it was made | Now |
|---|---|---|
| One connection per library (`App::conn`) | simple; `data_version` ignores own commits, which the caches use; requests were short | **Replaced** (step 5). Requests are not always short, and the cost of one that is not is the whole app |
| Rollback journal, no WAL | the `-shm` file on external exFAT drives; copies of `.shoebox` without the `-wal` file | **Stays.** WAL would let reads and commits overlap, which a pool alone does not (see "Pitfalls"). But short reads (steps 1 and 3) give most of that, and the reasons still hold for drives. Look again only for indexes on the computer (phase 16), and only if `bench-endpoints.py` shows readers waiting for commits |
| HTTP/1.1 only | | **Stays.** Browsers speak HTTP/2 only over TLS: certificates on every device (the iPad) for a local app. The six connections are handled by keeping requests short and calling them off |
| (new idea) Pictures from a second host name, for six more connections | | **No.** `Cross-Origin-Resource-Policy: same-origin`, the session cookie and the PIN would all need exceptions |
| Embeddings not kept in memory | 200 MB; nobody waits for a regrouping | **Stays.** `similar` (the only request that reads them) leaves the shared connection and can be called off |
| (new idea) HTTP timeouts (`tower::timeout`) | | **Not alone.** They end the reply but not the blocking work; the progress handler ends both |
| The search's turn (`in_turn`) | requests waited for the one connection anyway | **Stays.** It keeps the order and drops letters typed over; with a pool it is no longer needed for the connection, still for the CPU |
| (new idea) Sorting the face list in the browser | | **No.** 300,000 faces are ~15 MB of JSON |

## 6. Pitfalls

- **The rollback journal and several readers.** Readers on several
  connections read at the same time, but a commit (the writer, a scan, the
  regrouping) waits until every reader is done, and while it waits no new
  reader starts. So with a pool one long read still freezes the app as soon
  as anything writes; the pool helps only because steps 1 and 3 keep reads
  short. This is why step 5 comes last.
- **`data_version` per connection**: see step 5. A test for every cache: a
  change in between is seen at once, from the API and from another process.
- **Holding a connection while waiting** for a view or a lock starves the
  pool (step 5 rules). Today the search holds `App::conn` while the people
  view and the people's contents are read (`person_hits`, `counts`), which is
  the same convoy with one connection: the first `/suggest?q=a` after the
  start held it 2.2 s.
- **Calling off a shared build** (a kept view) makes the next request start it
  again; builds are never tied to a request.
- **Calling off a write** could leave half a change; writes are never stopped.
- **Aborts are not errors** in the page: otherwise every page change shows the
  error dialog.
- **Interrupted statements** are reads: the request answers an error; nothing
  is cached from it (a kept view is built only by its own builder).
- **`mmap_size` stays off**: on a drive that is pulled out, a read through a
  mapping ends the program (SIGBUS).
- **Memory**: each connection has its own page cache (2 MiB per database by
  default): six connections with three databases each at most 36 MiB.
- **Two writers** (the server and a scan, or the regrouping on
  `recognition.db`): a deferred transaction that read first fails at once with
  "database is locked"; `BEGIN IMMEDIATE` waits instead.
- **A drive pulled out** during a request: the pool belongs to the library
  (`App`); requests answer "offline" as now.
- **The busy line must not flicker**: shown only after 0.4 s, and not for
  requests of the whole app (status every 5 s while a scan runs).
- **The six connections**: the event stream keeps one per tab; two tabs leave
  four for everything else. Keep one stream per tab (as now).

## 7. Rules for CLAUDE.md (each added with the step that makes it true)

- No list of ids written into SQL; filter in Rust over a kept view or pass a
  set as one parameter (`IN (SELECT value FROM json_each(?1))`). (step 1)
- A request never reads all faces or all files itself; it asks a kept view.
  (step 1)
- A request that holds a connection for over 1 s is logged; the budget is in
  `bench-endpoints.py`. (step 2)
- A page's own requests carry the page's signal; an abort is not an error.
  Reads are stopped when their client leaves or after the deadline; changes
  and the building of kept views never are. (step 3)
- A page that waits shows the common busy line and placeholders. (step 4)
- Take kept views first, then a reader; never hold a connection while
  waiting for something else. Only the writer writes. (step 5)

## 8. Open questions

- The pool's size on the old Intel MacBook (2 cores): 2 readers may be enough;
  measure with `bench-endpoints.py` there.
- The deadline: 20 s is a guess for a USB hard disk; step 2's log on the real
  drive decides.
- Does every page need the event stream, or only People and Unnamed? It holds
  one of the six connections in every tab.
