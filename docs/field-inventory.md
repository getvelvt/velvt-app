# Field and retention inventory

This file covers the tables added for the optional question after a block,
and any later table that holds a person's own answers. Tables that existed
before it, such as `work_block` and `work_block_category_correction`, are
described in `PRIVACY.md` and are not listed here. For each covered table this
file lists every column, the class of value the column may hold, how long a
row is kept, and what each in-app destructive action does to it. `PRIVACY.md`
is still the reference for what every table in the database holds. This file
adds field classes, and it is checked rather than only written down.

`rust-service/tests/field_inventory.rs` reads this file and holds it to the
schema and to the app:

- **A `planned` table must not exist yet.** A table marked `planned` must not
  be in the migrated schema. The migration that creates it changes the status
  to `migrated` in the same commit.
- **A `migrated` table must match this file exactly.**
  - It has exactly the columns listed here, with the listed type and
    nullability.
  - Every `key`, `choice`, `flag` and `singleton` column refuses a typed
    sentence.
  - Every value listed for a `choice` column is accepted.
- **The listed reach must be the observed reach.** For a `migrated` table, the
  reach listed here must be what the test observes. The test runs each action
  through the service against a database that holds a row in every table.

The same test pins what the four actions do to every other table (`REACH`). So
a new table of any kind fails it until someone records what each action does
to that table. Where an action removes or rewrites only some rows, `REACH` also
names which of the test's seeded rows change, and every other row must stay
exactly as it was.

## Field classes

The list is closed, and it has no class for free text. A covered table holds
nothing a person typed, whether required or optional. It also holds no app
name, window title, web address, hostname, site name, intention or category.
A column that fits none of these classes is not added to a covered table.

| Class | What the column may hold |
|---|---|
| `key` | An identifier Velvt generated on this Mac, as a primary key or a reference to one. The database refuses any other value. |
| `choice` | One value from a closed list, fixed by a CHECK constraint. |
| `flag` | 0 or 1, fixed by a CHECK constraint. |
| `singleton` | The fixed id 1 of a one-row table. |
| `timestamp` | Unix seconds, UTC, written by the service. |
| `version` | A version number compiled into the build, written by the service. |

SQLite stores whatever it is given in a column that has no CHECK. So the test
writes a sentence into every `key`, `choice`, `flag` and `singleton` column of
a migrated table, and requires the database to refuse it. `timestamp` and
`version` columns are integers that only the service writes.

## Tables

Neither table is uploaded, and no upload path reads them.

| Table | Status | Retention | Reset Corrections | Clear Local Work Blocks | Log Out | Delete Account |
|---|---|---|---|---|---|---|
| `work_block_self_rating` | planned | Removed with its block, and by Clear Local Work Blocks. No sweep of its own. | kept | removed | kept | kept |
| `rating_prompt_settings` | planned | Singleton, with no sweep. It survives Clear Local Work Blocks because it is a setting the person chose, like `initiation_settings`. | kept | kept | kept | kept |

**What each action does.** Each value is what the action does to the rows
already in the table:

- `kept` means every row is unchanged.
- `removed` means no row is left.
- `removed in part` means some rows are deleted and the rest are unchanged.
- `rewritten:` means the same rows remain, and only the named columns change.

**The four actions.**

- Reset Corrections removes every correction, and every app and site rule
  taught on this Mac.
- Clear Local Work Blocks is the broadest local deletion in the app.
- Log Out is signing out.
- Delete Account is a request to the cloud.

No in-app action deletes all local data. Deleting `~/.velvt/` removes every
table (`PRIVACY.md`, "How to delete all local data").

**Why an answer survives Reset Corrections.** An answer about a block is not a
correction. Reset Corrections resets classification rules, and its
confirmation says so. An answer goes with its block instead: Clear Local Work
Blocks removes both.

## Columns

| Table | Column | Type | Null | Class | Holds |
|---|---|---|---|---|---|
| `work_block_self_rating` | `block_id` | TEXT | no | key | The block the answer is about. It is the primary key and references `work_block`, so the row goes when the block goes. It needs `NOT NULL` like every other text primary key, because SQLite allows NULL in a text primary key without it. |
| `work_block_self_rating` | `prompt_version` | INTEGER | no | version | The version of the question's wording the block was offered under. |
| `work_block_self_rating` | `eligible_at` | INTEGER | no | timestamp | When the block ended and the question became available. The row is written then, so a question that was never shown is still recorded as missing. |
| `work_block_self_rating` | `shown_at` | INTEGER | yes | timestamp | When the question was first on screen, or NULL if it never was. |
| `work_block_self_rating` | `answer` | TEXT | yes | choice | One of `all`, `most`, `half`, `less_than_half` or `little`, or NULL if there is no answer. Not now, closing and silence are never stored as an answer. |
| `work_block_self_rating` | `answered_at` | INTEGER | yes | timestamp | When the answer was given. It is NULL exactly when `answer` is. |
| `work_block_self_rating` | `not_now_at` | INTEGER | yes | timestamp | When Not now was tapped. |
| `work_block_self_rating` | `expired_at` | INTEGER | yes | timestamp | When the question stopped being available. |
| `work_block_self_rating` | `card_seen_before_answer` | INTEGER | no | flag | 1 if the answer was given after the block's result card had been on screen. |
| `rating_prompt_settings` | `id` | INTEGER | no | singleton | Always 1. |
| `rating_prompt_settings` | `enabled` | INTEGER | no | flag | Whether the question is asked. It is 0 until the person turns it on. |
| `rating_prompt_settings` | `opted_in_at` | INTEGER | yes | timestamp | When the question was last turned on. |
| `rating_prompt_settings` | `opted_out_at` | INTEGER | yes | timestamp | When the question was last turned off. |
