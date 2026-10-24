# Phase 19: groups in the search

A group (Family, Friends, ...) can be searched. The search so far was AND only:
several people give the photos they are all in. A **group** chip is the other
kind: the photos of *anyone* in the group.

## Decisions

- **One chip, OR inside.** `group=<id>` in the URL (`group=<name>` over all
  drives, like `person=` and `tag=`). Photos with a confirmed face of any person
  in the group, pets included. Several groups, people, tags and every other chip
  still narrow it (AND).
- **Where it is offered.** The search box has a "Groups" row group
  (`/api/groups/search`, `/api/all/groups`): by name, counted within the rest of
  the search, groups in the search and groups that would show nothing left out.
  Clicking a group in the sidebar unfolds it *and* shows its photos; its arrow
  only folds and unfolds.
- **No empty groups.** A group is made from a person (`POST /api/groups` takes
  `name` and `person_id`; the person's menu has "New group…", the dialog "Move to
  group…" too). The "New group…" buttons of the Faces page and of the Groups
  dialog are gone. A group a person leaves (moved, no group, merged, deleted, or
  a new group) is deleted when nobody is left in it (`people::drop_if_empty`);
  other empty groups that exist already are left alone (the user removes them).
  A group that goes while it is in the search is dropped from the search.
- **Nothing about originals.** Groups live in `library.db` only; the filter
  only reads.
- **Icons.** Our own drawings (rule in CLAUDE.md): the group is three pawns, the
  middle one taller. The person chip (a single pawn), the cat (sitting), the
  paw, the map marker and the calendar of the timeline toolbar replace the
  emojis in the chips and suggestions in a second commit.
