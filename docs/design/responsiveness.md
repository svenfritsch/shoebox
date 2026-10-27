# Responsive on large libraries (planned)

October 2026. Step 1 is merged (why, and what was not done: "No id lists in
SQL" in [decisions.md](../decisions.md)); the others are not built. Linked from [open.md](../open.md). Each step, when it
is merged: its rule in `CLAUDE.md` (section 6), the entry "One shared
connection per drive" in [decisions.md](../decisions.md) with step 2, the
guide with step 4, and its part of this file deleted; the file goes with the
last step.

On a library of about 100,000 photos and 299,000 faces, the Face check page's
request `GET /api/lib/<id>/faces?sort=size&offset=0&limit=300&unmatched=true`
took about 2.5 minutes. While it ran, the rest of the photo app did not
answer, and leaving the page did not stop it. The fix is to remove the bad
query and the single connection that let one request block all others, not to
build machinery around slow requests: once no request reads the whole library
and reads do not wait for each other, there is nothing long left to stop.

## 1. What happened

1. **A query that grows with the square of the library** (`faces::list`):
   every unmatched face id written into the SQL. Fixed by step 1.
2. **One connection for everything.** Most handlers lock `App::conn` (one
   `Mutex<Connection>` per library, `library.db` with `thumbs.db` and
   `recognition.db` attached). While one request holds it, every other one
   (thumbnails, crops, `/api/info`, the sidebar) waits behind it. Changes hold
   it for as long as they take too: a move of 5,000 photos renames and commits
   group by group (`organize::move_group`, a synced commit each on exFAT).
3. **Requests nobody waits for still run.** When the browser gives up, the
   server drops the async part of the handler, but its blocking part already
   waits for the lock and runs once it gets it. After the slow request, every
   request that queued behind it ran too, for nobody.
4. **No sign of waiting.** The page shows nothing while a request is slow.

Over HTTP/1.1 the browser sends at most six requests at a time to one server,
pictures and API calls together, and the event stream (`/api/events`) keeps
one of them open in every tab. Requests that hang hold the others, so
pictures stop loading too. Short requests are the answer to that as well.

## 2. Measured (benchmark library)

`scripts/bench-people.py make` (100,000 files, 298,755 faces, 129,797
decisions, 300 people), release build, 4-core Linux VM with an SSD; the real
drive is slower. "Probe" is a client that asks for `/api/groups` (a trivial
query) every 100 ms meanwhile: its longest wait is how long the rest of the
app was frozen.

| Request | Took | Probe's longest wait |
|---|---|---|
| Face check, "Not matched yet", on the real drive | about 150 s | as long (the whole app) |
| The same on the benchmark library (about 165,000 faces without a decision) | no answer in 5 minutes; the client gave up | every probe request timed out (60 s); `/api/groups` asked after the client had gone still got no answer within a minute |
| Pet check, "Not matched yet" (1,519 pets) | 33.4 s | 33.4 s |
| `/faces/stats` (the Face check's first request) | 1.72 s | 1.72 s |
| `/faces/stats?kind=pets` | 1.26 s | 1.25 s |
| `/faces?sort=size&limit=300` (all faces) | 1.46 s | 1.45 s |
| `/faces?…&not_face=true` (54) / `pet_face=true` (30) / `rotated=true` | 0.71 / 0.79 / 0.88 s | as long |
| `/faces/{id}/similar?limit=24` | 1.42 s | 1.42 s |
| `/suggest?q=a`, first after the start | 2.20 s | 2.20 s |
| `/people`, first (reads the people view **on its own connection**) | 1.01 s | 0.00 s |
| `/duplicates` 0.28 s, `/info` 0.10 s, `/timeline` 0.02 s, `/folders` 0.00 s | | as long |

The probe waits exactly as long as each request holds `App::conn`; the one
request that reads on a connection of its own (`/people`) holds up nobody.

After step 1, same library and machine: every Face check list (all faces,
each filter, "Not matched yet" of faces and pets) 0.01 to 0.11 s, the probe
not held up; `/faces/stats` 0.36 s (pets 0.20 s) and `similar` 0.42 s, the
probe waiting about as long: the counts over `recog.looked` and the
embeddings still read on `App::conn`. The first Face check request after a
start reads the people view on its own connection (0.9 s; `/faces/stats`
then takes 1.9 s, the probe waiting its 0.36 s).

## 3. Steps

Each step is a pull request of its own and helps alone.

| Step | What | Fixes |
|---|---|---|
| 2 | A pool of read-only connections; writes stay on `App::conn`; requests nobody waits for do not start | one request holding up all others |
| 3 | Slow-request log; the Face check in `bench-people.py` | finding the next one |
| 4 | A busy line while a request takes long | no sign of waiting |
| 5 | Measure first: the people view and `similar` read in pieces, and smaller items | commits waiting behind long reads |

### Step 2: a pool of read-only connections

- `App` gets a small pool of read-only connections (2 to 4 by cores,
  `PRAGMA query_only`, `thumbs.db` and `recognition.db` attached; a
  `Mutex<Vec<Connection>>` with a guard, no new crate). Requests that only
  read take one (`app.read()`); every write stays on `App::conn` as now
  (changes, and the thumbnails and crops stored on request). A write sent to a
  reader fails in the tests instead of locking in production.
- **The version stays on one connection, `App::conn`.** `index_version` is
  `(PRAGMA data_version, generation)` and relies on `data_version` ignoring
  the connection's own commits, so changes to people (which do not move
  `generation`) do not reload every open page. `data_version` belongs to one
  connection; numbers from two connections cannot be compared. Readers ask for
  it with `try_lock` and, while a change holds `App::conn`, use the last number
  read: otherwise every read would wait for a long move again. The change
  moves `generation` itself when it is done, and whatever another process
  committed meanwhile is seen by the first read after it. The same for
  `recog.data_version` in the status.
- **A request nobody waits for does not start.** `blocking` gets a flag that a
  guard in the handler's future sets when hyper drops it (the browser closed
  the connection); it is looked at once a connection is taken, and the request
  ends there. What has started runs to its end. The search keeps its queue
  (`in_turn`): one at a time in the order typed, dropping letters typed over.
- **Take the kept view first, then a reader**, and give the reader back before
  waiting for anything else. Four requests waiting for the people view while
  each holds a reader would leave none for the rest. Today the search holds
  `App::conn` while it reads the people view and the people's contents
  (`person_hits`, `counts`): the first `/suggest?q=a` held it 2.2 s.
- The kept views (snapshot, tags, duplicates) are built on a reader, under
  their own lock as now.

Tests: while one reader is busy with a long read, `/api/groups`, a thumbnail
and `/api/info` answer (no time limit: only that they are not behind it; the
long read can be a request the test holds open, e.g. a reader taken through a
test-only hook in `serve::Options`); for every cache keyed by the version, a
change through the API **and** a commit by another connection (a scan) are seen
at once; a write on a reader fails; the browser tests against the real server
as they are.

### Step 3: seeing where the time goes

- **Slow-request log.** Every library request that takes over 1 s prints one
  line: method, path with query, total time, and how long it waited for a
  connection. The incident would have printed one line naming the Face check
  request and, for each request behind it, how long it waited.
- **`bench-people.py run`** gets the Face check (stats, each filter,
  `similar`, faces and pets) and the first `/suggest`, each alone and under its
  probe (`/api/groups` every 100 ms), so the numbers of section 2 can be taken
  again before a release; they go into the pull request that changes them.

### Step 4: a busy line

A thin line under the top bar, shown when an API request (`api()`) has run
0.4 s and gone when none is left; quick ones (the status every 5 s while a
scan runs) never show it, pictures are not API requests (they have their grey
tile). **Not for the search suggestions** (`/suggest`, `/all/suggest`): the
dropdown shows its own grey rows after 120 ms (`showWaiting`), and two signs
for one wait are one too many. The suggestion request says so where it is
made (`api(path, { busy: false })`), not by a list of paths in `api()`, so a
new request with a waiting display of its own opts out the same way. A new
common class in `app.css` and a row in the `frontend` skill's table; no motion
with `prefers-reduced-motion`. A cold USB drive still takes seconds for the
first people view or snapshot. The guide gets one sentence in "Finding your
way around" (both languages). Tests (`faces-mocked.mjs`, fake clock,
`lib.hold`): no line for an answer within 0.4 s, the line at 0.4 s, gone with
the answer; a `/suggest` held past 0.4 s shows the grey rows and no line.

### Step 5: measure first

Only if `bench-people.py` or the slow-request log shows it:

- **Long reads in pieces.** Without WAL, a commit waits until every reader is
  done, and while it waits no new reader starts. So a long read on any
  connection still makes the next write, and every read after it, wait. The
  long reads left after step 1 are the people view (`View::load`, about 1 s on
  the benchmark library, more on a cold drive) and `similar` (every
  embedding). They can be read in pieces by row id, with `data_version`
  looked at before and after and the view read again when it moved; at most a
  few times, then in one piece, or a scan that commits every second would keep
  it from ever finishing.
- **Start-up order.** The page asks for the timeline only when folders, the
  status, own tags, people, groups and places have all come (`Promise.all`
  before `applyFilter`), so the photos wait for the slowest of them. Ask for
  the timeline at once and fill the sidebar as each part comes.
- **While `shoebox recognize` runs** in another process, every commit to
  `recognition.db` makes the next people request read the whole view again.
  Keep the view a few seconds while a recognize job runs, as `/api/info` keeps
  its counts.
- **Crops of decided faces on the Face check.** A face with a decision has no
  kept crop, so filters like "Not a face" decode the original for every crop.
  The other pages cut such faces out of the photo's thumbnail (`zoomFace`);
  the face list would need the box for that.

## 4. Considered and not done

| Idea | Why not |
|---|---|
| Stop SQL in the middle (progress handler) when the client leaves, a 20 s deadline, a "Try again" screen, an `AbortController` for every page | Machinery for requests that take minutes; after steps 1 and 2 there are none, and a rare slow one holds one reader, not the app. The cheap part stays: a request nobody waits for does not start |
| More lanes: a connection of its own for storing thumbnails, and for each kept view | More to keep in step for little gain; thumbnail stores are short, kept views build under their own lock |
| `Server-Timing` headers | The slow-request log answers the same question |
| WAL | Would let reads and commits overlap, but its `-shm` file on external exFAT drives and copies of `.shoebox` without the `-wal` file still speak against it. Look again only for libraries kept next to the program (index on the computer), if step 5 shows commits waiting |
| HTTP/2 | Browsers speak it only over TLS: certificates on every device (the iPad) for a local app |
| Pictures from a second host name, for six more connections | `Cross-Origin-Resource-Policy: same-origin`, the session cookie and the PIN would all need exceptions |
| HTTP timeouts (`tower::timeout`) | End the reply, not the blocking work |

## 5. Pitfalls

- **`data_version` per connection**: see step 2. A test for every cache.
- **Holding a reader while waiting** for a kept view or another lock starves
  the pool (step 2).
- **The rollback journal**: a pool makes reads wait for each other no more,
  but a long read still holds up commits (step 5).
- **Two writers** (the server and a scan, or the regrouping on
  `recognition.db`): a deferred transaction that read first fails at once with
  "database is locked"; changes that read before they write start with
  `BEGIN IMMEDIATE` so they wait instead.
- **`mmap_size` stays off**: on a drive that is pulled out, a read through a
  mapping ends the program (SIGBUS).
- **Memory**: each connection has its own page cache (2 MiB per database by
  default): five connections with three databases each at most 30 MiB.
- **A drive pulled out** during a request: the pool belongs to the library
  (`App`) and goes with it; requests answer "offline" as now.

## 6. Rules for CLAUDE.md (each added with the step that makes it true)

- Reads take a reader (`app.read()`), only `App::conn` writes; take kept views
  first, then a reader, and never hold a connection while waiting for
  something else. (step 2)

## 7. Open questions

- The pool's size on the old Intel MacBook (2 cores): 2 readers may be enough.
- Whether step 5 is needed at all: the slow-request log on the real drive
  decides.
