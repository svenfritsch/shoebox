# Responsive on large libraries (planned)

October 2026. Steps 1 to 4 are merged (why, and what was not done: "No id
lists in SQL" and "Reads on a pool of connections" in
[decisions.md](../decisions.md)); the other is not built. Linked from
[open.md](../open.md). Each step, when it is merged: its part of this file deleted; the file goes with the last step.

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
2. **One connection for everything.** Most handlers locked `App::conn` (one
   `Mutex<Connection>` per library, now `App::writer` for writes only, `library.db` with `thumbs.db` and
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

After step 2 (`bench-people.py probe`, same library and machine, two runs
each; "before" is the build without the pool). The probe's own request takes
2 ms.

| Request | Took before | Probe's longest wait before | Took after | Probe's longest wait after |
|---|---|---|---|---|
| `/suggest?q=a`, first after the start | 2.7 to 3.3 s | as long | 1.8 to 1.9 s | 2 ms |
| `/faces/stats` | 0.37 to 0.55 s | 0.26 to 0.55 s | 0.41 to 0.42 s | 2 to 4 ms |
| `/faces/stats?kind=pets` | 0.20 s | 0.19 to 0.20 s | 0.19 s | 2 ms |
| `/faces/{id}/similar?limit=24` | 0.27 to 0.35 s | as long | 0.27 to 0.29 s | 2 ms |
| `/duplicates` | 0.37 to 0.39 s | 0.21 s | 0.23 to 0.29 s | 2 ms |
| `/move` of 5,000 photos | 8.5 to 9.5 s | as long | 8.9 to 9.6 s | 0.73 to 0.83 s (95 % within 0.18 to 0.33 s) |

What is left during the move is the rollback journal (step 5): every photo's
group is a synced commit, and a read that runs into one waits for it.

After step 3 (`bench-people.py probe`, same library and machine, with the
script that times every Face check filter: one pass with each request alone,
one with the probe, each on a fresh server; the build without the slow-request
log gave the same within noise; the move's probe wait was 0.73, 0.93 and 1.93 s in three runs, so
read it as 0.7 to 2 s on this VM).

| Request | Alone | Under the probe | Probe's longest wait |
|---|---|---|---|
| `/suggest?q=a`, first after the start | 1.90 s | 1.94 s | 2 ms |
| `/suggest?q=an` | 11 ms | 13 ms | 2 ms |
| `/faces/stats` | 379 ms | 358 ms | 2 ms |
| `/faces/stats?kind=pets` | 180 ms | 198 ms | 2 ms |
| `/faces`, all faces / small / large | 89 / 33 / 80 ms | 91 / 38 / 91 ms | 2 to 3 ms |
| `/faces`, rotated / not a face / pet face | 33 / 30 / 29 ms | 35 / 33 / 34 ms | 2 to 4 ms |
| `/faces`, not matched yet | 55 ms | 53 ms | 8 ms |
| `/faces?kind=pets`, all / small / large | 7 / 6 / 5 ms | 8 / 8 / 9 ms | 2 to 3 ms |
| `/faces?kind=pets`, not a pet / not matched yet | 3 / 4 ms | 7 / 8 ms | 2 ms |
| `/faces/{id}/similar?limit=24` | 378 ms | 324 ms | 2 ms |
| `/duplicates` | 236 ms | 274 ms | 2 ms |
| `/move` of 5,000 photos | 8.71 s | 8.37 s | 932 ms |

The slow-request log printed three lines in each pass: the first `/info`
after the start (1.8 to 2.0 s), the first `/suggest` and the move, each
waiting 0 ms for a reader, the writer and the people data (they are the long
work, nothing held them up).

## 3. Steps

Each step is a pull request of its own and helps alone.

| Step | What | Fixes |
|---|---|---|
| 5 | Measure first: the people view and `similar` read in pieces, and smaller items | commits waiting behind long reads |

### Step 5: measure first

Only if `bench-people.py` or the slow-request log shows it:

- **Reads behind a move's commits.** A move commits every photo's group on its
  own (synced on exFAT); a read that runs into a commit waits, and SQLite's
  busy handler backs off up to 100 ms at a time: during a move of 5,000 photos
  the probe waited 0.7 to 0.8 s at most (section 2). Fewer, larger commits
  would shorten that, if the real drive shows it.
- **Long reads in pieces.** Without WAL, a commit waits until every reader is
  done, and while it waits no new reader starts. So a long read on any
  connection still makes the next write, and every read after it, wait. The
  long reads left after step 1 are the people view (`View::load`, about 1 s on
  the benchmark library, more on a cold drive) and `similar` (every
  embedding). They can be read in pieces by row id, with `data_version`
  looked at before and after and the view read again when it moved; at most a
  few times, then in one piece, or a scan that commits every second would keep
  it from ever finishing.
- **Start-up order.** The first `/info` after a start takes about 1.9 s on the
  benchmark library (slow-request log, section 2), as long as the first
  `/suggest`; find out what it builds. The page asks for the timeline only
  when folders, the status, own tags, people, groups and places have all come
  (`Promise.all` before `applyFilter`), so the photos wait for the slowest of
  them. Ask for the timeline at once and fill the sidebar as each part comes.
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

- **The rollback journal**: a pool makes reads wait for each other no more,
  but a long read still holds up commits (step 5).
- **Two writers** (the server and a scan, or the regrouping on
  `recognition.db`): a deferred transaction that read first fails at once with
  "database is locked"; changes that read before they write start with
  `BEGIN IMMEDIATE` so they wait instead.

## 6. Open questions

- Whether step 5 is needed at all: the slow-request log on the real drive
  decides.
