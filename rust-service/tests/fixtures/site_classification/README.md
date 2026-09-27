# Site classification corpus

`corpus.jsonl` is a blind, hand-labelled sample of what people actually have in
front of them at work. It exists to measure how the abstraction engine
classifies browser tabs, and native windows alongside them, from the fields the
Swift client really sends. `tests/site_classification_measure.rs` reads it:

```sh
cargo test --test site_classification_measure -- --ignored --nocapture
```

That prints a report and writes JSON to the path in `VELVT_MEASURE_OUT`. The
test measures and does not gate on the numbers. It fails only when this file
breaks the rules in the last section, when the engine returns an error for a
visit, or when five passes in a row each hit a Tier 2 timeout (a pass that
hits one is thrown away and rerun, because it measured the machine's load), so
a run that finishes printed numbers that describe the engine.

## What is in it

1,734 visits, one JSON object per line:

| field       | meaning                                                                      |
|-------------|------------------------------------------------------------------------------|
| `persona`   | which persona the visit belongs to                                           |
| `app_name`  | the frontmost application, as macOS names it                                 |
| `bundle_id` | that application's bundle identifier                                         |
| `host`      | the focused tab's host for a browser window; `null` for a native app window  |
| `title`     | the window title                                                             |
| `seconds`   | how long the visit lasted                                                    |
| `truth`     | the category the person would give this time (below)                        |
| `split`     | `dev` or `test` (below)                                                      |

1,522 visits are browser tabs across 561 hosts (after removing a leading
`www.`) in 382 registrable domains; 212 are native app windows in 55
applications. Ten personas wrote
them, two per source file: backend engineer, data scientist, product designer,
frontend engineer, sales and marketing lead, support lead, product manager,
founder/operations, PhD researcher and technical writer. The organisations,
people and projects in titles are made up.

Repeated lines are repeated visits, not duplicates to remove. `seconds` is the
dwell as the author wrote it; the router caps a stored dwell at 30 minutes and
71 visits are longer, but the measurement weights by what the author wrote.

## How it was made

Each persona was written by an author who had not seen Velvt's classifier or
the change the corpus was first used to measure. The authors wrote down what
that person has open at work (the app, the host, the window title, how long)
and the truth for each visit, without being able to check what Velvt says
about it. That blindness is what makes the numbers mean anything, and it is
why the corpus is kept apart from any classifier work.

The five source files were validated line by line (exact field set, types, a
bare lowercase hostname or `null` for `host`, positive whole `seconds`, a
known `truth`) and merged in file-name order, keeping each file's line order,
with `split` added.

### Truth

`truth` is one of the seven categories a person can mean, or `UNSURE`:

- `FOCUS_WORK`, `PASSIVE_CONSUMPTION`, `SOCIAL_FEED`, `COMMUNICATION`,
  `TASK_MANAGEMENT`, `REFERENCE`, `SYSTEM` are the taxonomy's categories.
  `UNLOGGED` is never a truth: it is the engine saying it did not classify.
- Truth is what the person was doing, not what the site is. The same host can
  carry different truths: GitHub's sign-in page is `SYSTEM` and reading a
  pull request is `REFERENCE`; a YouTube video is `PASSIVE_CONSUMPTION` or
  `UNSURE` depending on the video.
- `SYSTEM` is overhead around the work: sign-in and SSO pages, password
  managers, account and device settings.
- `UNSURE` marks a visit whose honest category cannot be decided from the app,
  host and title, because it depends on what the person was doing on that page
  (a recorded walkthrough, a technical talk on YouTube, a retro page, a
  shared tracker sheet, a banking login).

The measurement leaves `UNSURE` out of precision and reports confident
answers on `UNSURE` visits separately, as overconfidence on genuinely mixed
pages.

### Split

`split` is assigned by registrable domain, so a domain is never in both
halves:

1. Normalize the host: lowercase it and strip one leading `www.`.
2. Take its registrable domain: the last two labels, or the last three under
   a two-label public suffix such as `co.uk` or `ac.uk` (the harness's
   `MULTI_LABEL_PUBLIC_SUFFIXES`). `acme.atlassian.net` and
   `other.atlassian.net` are both `atlassian.net`; a host with no more labels
   than that, such as `localhost`, is its own.
3. `split` is `dev` if the first byte of `sha256(registrable domain)` is even,
   otherwise `test`.
4. A native visit (`host` is `null`) is split the same way on
   `sha256(app_name)`.

The unit is the registrable domain because the engine decides a whole domain
at once: a site seed reaches every host under it, and leaving one out moves
all of them. Until 2026-09-27 the split was by exact host, so one such
decision reached hosts in both halves and a change justified on `dev` moved
`test` too; `split` was regenerated when the rule changed, and a baseline
from before then is not comparable with one after. A signal that reads a label
any site can carry (a `docs.` in front, an `.edu` at the end) still reaches
both halves: no split by site can hold that out.

Use `dev` when looking at examples while changing classification. Treat
`test` as held out: report it, do not tune against its domains. Because a
large domain lands wholly in one half (every `google.com` host, Figma and
GitHub are in `dev`; YouTube, Spotify and the `atlassian.net` workspaces are
in `test`), the two halves differ in make-up. Compare a change with its
baseline within a split, not `dev` against `test`.

## Never edit it to fit a classifier

The truth was written before, and without sight of, any classifier. Changing a
truth because the classifier disagrees turns the measurement into a
measurement of itself, and it stops meaning anything for every later change as
well. So:

- Do not change a `truth`, `title`, `host` or `seconds` to make a result look
  better or worse, and do not drop lines the classifier gets wrong.
- If a truth is plainly wrong on its own terms (a typo, or a category the
  title contradicts), fix it in a commit of its own that says why, decided
  without looking at what the classifier outputs for that line, and rerun the
  baseline.
- Grow the corpus only with new blind persona files, merged the same way,
  never with lines written to exercise a particular rule.

The test enforces the parts that can be checked mechanically: every line must
parse with exactly these fields (`bundle_id` and `host` may be `null` but not
missing), `truth` must be a category or `UNSURE`, `seconds` must be positive,
and `split` must follow the split rule.
