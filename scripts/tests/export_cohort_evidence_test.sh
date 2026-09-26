#!/usr/bin/env bash
# Contract test for the participant-facing export.
#
# Three things are pinned here, and the first two pull in opposite directions.
#
# 1. The export must carry ENOUGH: block timings on every offer, the decision
#    log with `policy_version`, a row for every declared block (offer or not),
#    `card_seen_at`, invitation outcomes, the explain-tap counts and the
#    correction counts, so every measure pre-registered in `traction-summary.md`
#    can be computed from what a tester sends back.
#
# 2. The export must carry NOTHING ELSE. The script's own disclosure promises a
#    participant that no intention, app name, window title, URL or filename can
#    appear. That promise is tested by seeding every one of those fields with a
#    sentinel string and grepping every produced file for it. A promise in a
#    heredoc that nothing verifies is not a promise.
#
# 3. The export must still run on the databases testers actually have. Every
#    fixture is built by replaying the shipped migrations up to a real build's
#    schema: protocol 30 (1.0.11), protocol 28 (1.0.9, no `card_seen_at`), a
#    28 -> 30 upgrade (rows from before migration 0032 are "unknown", not
#    "unseen"), protocol 25 (1.0.1, no decision log at all), 1.0.0 (no
#    app-scoped corrections), and protocol 31 (develop) for the corrections
#    file. Older than protocol 25 must still stop with an error.
#
# The tester receives the script pasted or attached, with no execute bit, and
# runs `bash export_cohort_evidence.sh` under the bash 3.2 that ships with
# macOS. So the main case runs a non-executable copy, from its own folder,
# through /bin/bash where it exists.

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
export_script="$repo_root/scripts/export_cohort_evidence.sh"
analyze="$repo_root/scripts/analyze_cohort.py"
export FIXTURE_MIGRATIONS_DIR="$repo_root/rust-service/migrations"
# shellcheck source=lib/fixture_db.sh
source "$repo_root/scripts/tests/lib/fixture_db.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

command -v sqlite3 >/dev/null 2>&1 || { echo "ERROR: sqlite3 not on PATH" >&2; exit 1; }
[[ -f "$export_script" ]] || { echo "ERROR: missing $export_script" >&2; exit 1; }

tester_bash="bash"
if [[ -x /bin/bash ]]; then tester_bash="/bin/bash"; fi
# Local weeks are computed on the tester's clock; pin it so the week keys below
# are the same on every machine that runs this test.
export TZ=UTC

fail() { echo "FAIL: $*" >&2; exit 1; }

# 2027-01-15 08:00:00 UTC, a Friday. Its local week (UTC) starts 2027-01-11.
T0=1800000000
INSTALLED_AT=$((T0 - 86400))

S_INTENTION='ZZSENTINELINTENTIONZZ'
S_APPNAME='ZZSENTINELAPPNAMEZZ'
S_DISPLAY='ZZSENTINELDISPLAYLABELZZ'
S_LABEL='ZZSENTINELEVENTLABELZZ'
S_MAPNAME='ZZSENTINELMAPDISPLAYNAMEZZ'
S_OVERRIDE='ZZSENTINELOVERRIDENAMEZZ'
S_URLHOST='ZZSENTINELURLHOSTZZ'
# Written straight into a correction's category, around the IPC check that
# refuses it, to prove the exporter's closed set is what stops it.
S_CATEGORY='ZZSENTINELCATEGORYZZ'
SENTINELS=("$S_INTENTION" "$S_APPNAME" "$S_DISPLAY" "$S_LABEL" "$S_MAPNAME" "$S_OVERRIDE" "$S_URLHOST" "$S_CATEGORY")

# Every local-only text column that really holds raw identity on a Mac.
seed_sentinels() {
  sqlite3 "$1" <<SQL
INSERT INTO raw_event_buffer (event_id, stable_id, label, category,
    taxonomy_version, occurred_at, classification_tier, classification_status,
    classification_confidence, classification_source, local_display_label,
    local_name_suggestion)
VALUES ('evt-1','stable-1','communication:$S_LABEL','COMMUNICATION','mvp-1',
    $T0,'exact_match','classified','high','seed','$S_DISPLAY','$S_APPNAME');
INSERT INTO abstraction_map (key_hash, stable_id, label, category,
    taxonomy_version, display_name)
VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1','stable-1','communication:chat','COMMUNICATION','mvp-1','$S_MAPNAME');
INSERT INTO personal_app_override (app_key_hash, category, activity_name)
VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2','FOCUS_WORK','$S_OVERRIDE');
INSERT INTO personal_override (key_hash, category, activity_name)
VALUES ('ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc3','REFERENCE','$S_URLHOST');
-- Observation rows exist and must not be exported at all.
INSERT INTO work_block_observation (block_id, occurred_at, ended_at, category,
    classification_status, classification_confidence)
VALUES ('block-1', $((T0 + 10)), $((T0 + 400)), 'FOCUS_WORK', 'classified', 'high');
SQL
}

# The blocks and offers every current-schema fixture shares. `origin` exists
# from 0022, so this runs on protocol 28 and 30 fixtures.
#   block-1  manual, completed, returned at +300s after an offer at +600s
#   block-2  manual, completed, 300s paused, Focus held the offer
#   block-3  invitation, completed, auto-demotion withheld the offer
#   block-4  manual, completed, no_response
#   block-5  manual, abandoned after 100s: no offer, one warm-up abstention
#   block-6  invitation, still active at export: no offer, no decision
#   block-7  manual, completed, offered past the planned end (clamp to 0)
seed_blocks_and_offers() {
  sqlite3 "$1" <<SQL
INSERT INTO work_block (block_id, phase, intention, purpose, intensity,
    planned_duration_seconds, started_at, total_paused_seconds, ended_at,
    intention_expires_at, origin)
VALUES
 ('block-1','completed','$S_INTENTION deep dive','deep_work','medium',
   3000, $T0, 0, $((T0 + 3000)), $((T0 + 86400)), 'manual'),
 ('block-2','completed','$S_INTENTION with a pause','study','light',
   3000, $((T0 + 10000)), 300, $((T0 + 13300)), $((T0 + 96400)), 'manual'),
 ('block-3','completed',NULL,'deep_work','intense',
   3000, $((T0 + 20000)), 0, $((T0 + 23000)), $((T0 + 106400)), 'invitation'),
 ('block-4','completed',NULL,'deep_work','medium',
   1800, $((T0 + 30000)), 0, $((T0 + 31800)), $((T0 + 116400)), 'manual'),
 ('block-5','abandoned','$S_INTENTION gave up','study','light',
   1500, $((T0 + 40000)), 0, $((T0 + 40100)), $((T0 + 126400)), 'manual'),
 ('block-6','active',NULL,'deep_work','medium',
   1500, $((T0 + 50000)), 0, NULL, $((T0 + 136400)), 'invitation'),
 ('block-7','completed',NULL,'deep_work','intense',
   3000, $((T0 + 60000)), 0, $((T0 + 63000)), $((T0 + 146400)), 'manual');

INSERT INTO work_block_intervention (block_id, offered_at, action_id,
    anchor_category, switch_count, window_seconds, outcome, outcome_at, salience)
VALUES
 ('block-1', $((T0 + 600)), 'protect_next_10', 'FOCUS_WORK', 3, 600,
   'returned', $((T0 + 900)), 'normal'),
 ('block-2', $((T0 + 10900)), 'protect_next_10', 'FOCUS_WORK', 4, 600,
   'delivery_suppressed_dnd', $((T0 + 10900)), 'normal'),
 ('block-3', $((T0 + 20700)), 'protect_next_10', 'REFERENCE', 3, 600,
   'withheld_demotion', $((T0 + 20700)), 'normal'),
 ('block-4', $((T0 + 30400)), 'protect_next_10', 'FOCUS_WORK', 3, 600,
   'no_response', $((T0 + 31800)), 'quiet'),
 ('block-7', $((T0 + 63600)), 'protect_next_10', 'REFERENCE', 5, 600,
   'was_focused', $((T0 + 63700)), 'normal');

INSERT INTO intervention_decision_log (decision_id, occurred_at, block_id,
    policy_version, anchor_category, switch_count, elapsed_seconds,
    remaining_seconds, gate_verdict, propensity, anchor_seen_within_600s, outcome_at)
VALUES
 ('d-01', $((T0 + 60)),    'block-1', 2, NULL,         0,   60, 2940, 'abstained_warmup', 1.0, 1, $((T0 + 660))),
 ('d-02', $((T0 + 600)),   'block-1', 2, 'FOCUS_WORK', 3,  600, 2400, 'offered',          1.0, 1, $((T0 + 1200))),
 ('d-03', $((T0 + 10900)), 'block-2', 2, 'FOCUS_WORK', 4,  600, 2400, 'suppressed_dnd',   1.0, 0, $((T0 + 11500))),
 ('d-04', $((T0 + 20700)), 'block-3', 2, 'REFERENCE',  3,  700, 2300, 'withheld_demotion',1.0, NULL, NULL),
 ('d-05', $((T0 + 30400)), 'block-4', 2, 'FOCUS_WORK', 3,  400, 1400, 'offered',          1.0, NULL, NULL),
 ('d-06', $((T0 + 40050)), 'block-5', 2, NULL,         0,   50, 1450, 'abstained_warmup', 1.0, NULL, NULL),
 ('d-07', $((T0 + 63600)), 'block-7', 2, 'REFERENCE',  5, 3600,    0, 'offered',          1.0, 0, $((T0 + 64200)));
SQL
}

seed_invitations_and_probe() {
  sqlite3 "$1" <<SQL
INSERT INTO initiation_invitation (invitation_id, offered_at, local_date,
    action_id, policy_version, backoff_policy_version, outcome, outcome_at)
VALUES
 ('inv-1', $((T0 + 19000)), '2027-01-15', 'soft_start_25', 1, 1, 'accepted', $((T0 + 19100))),
 ('inv-2', $((T0 + 45000)), '2027-01-15', 'soft_start_25', 1, 1, 'dismissed', $((T0 + 45060))),
 ('inv-3', $((T0 + 49900)), '2027-01-15', 'soft_start_25', 1, 1, 'offered', NULL);
INSERT INTO initiation_settings (id, invitations_enabled, updated_at) VALUES (1, 0, $T0);
INSERT INTO explain_probe_week (week_start_local_date, taps, updated_at)
VALUES ('2027-01-11', 2, $T0);
SQL
}

run_export() { # DB OUT_CSV LOG
  VELVT_DATABASE_PATH="$1" "$tester_bash" "$export_script" "$2" > "$3" 2>&1 \
    || fail "exporter exited non-zero on $1:$(printf '\n')$(cat "$3")"
}

field_of() { # CSV BLOCK_ID COLUMN -> value
  python3 - "$1" "$2" "$3" <<'PY'
import csv, sys
for row in csv.DictReader(open(sys.argv[1])):
    if row["block_id"] == sys.argv[2]:
        print(row[sys.argv[3]])
        break
else:
    raise SystemExit(f"{sys.argv[2]} not in {sys.argv[1]}")
PY
}

meta_of() { # META KEY
  python3 - "$1" "$2" <<'PY'
import csv, sys
meta = {r["key"]: r["value"] for r in csv.DictReader(open(sys.argv[1]))}
print(meta[sys.argv[2]])
PY
}

assert_arity() {
  python3 - "$1" <<'PY' || fail "header/row arity mismatch in $1"
import csv, sys
rows = list(csv.reader(open(sys.argv[1])))
width = len(rows[0])
for index, row in enumerate(rows[1:], start=2):
    if len(row) != width:
        raise SystemExit(f"row {index}: {len(row)} fields, header has {width}")
PY
}

assert_no_sentinel() { # FILE...
  local file sentinel
  for file in "$@"; do
    for sentinel in "${SENTINELS[@]}"; do
      if grep -qF -- "$sentinel" "$file"; then
        echo "--- offending file: $file ---" >&2
        cat "$file" >&2
        fail "sentinel leaked into $(basename "$file"): $sentinel"
      fi
    done
  done
}

OFFERS_HEADER='block_id,purpose,intensity,planned_duration_seconds,block_phase,started_at,ended_at,total_paused_seconds,offered_at,remaining_seconds_at_offer,action_id,anchor_category,switch_count,window_seconds,salience,outcome,outcome_at,seconds_to_outcome,returned_within_10min,wrong_intervention,card_seen_at,card_seen'
DECISIONS_HEADER='decision_id,occurred_at,block_id,policy_version,anchor_category,switch_count,elapsed_seconds,remaining_seconds,gate_verdict,propensity,anchor_seen_within_600s,outcome_at,block_started_at,block_ended_at,block_total_paused_seconds,block_planned_duration_seconds'
BLOCKS_HEADER='block_id,origin,phase,started_at,ended_at,total_paused_seconds,planned_duration_seconds'
INVITATIONS_HEADER='invitation_id,offered_at,action_id,policy_version,backoff_policy_version,outcome,outcome_at'
EXPLAIN_HEADER='week_start_local_date,taps,delivered_interventions,blocks_declared'
CORRECTIONS_HEADER='scope,category,rules,corrections'

# ===========================================================================
# A. Protocol 30 (1.0.11), fresh install. The tester's path: a pasted copy
#    with no execute bit, run from its own folder, default output names.
# ===========================================================================
db30="$work/p30.sqlite3"
migrate_fixture_db "$db30" "$FIXTURE_MIGRATIONS_PROTOCOL_30" "$INSTALLED_AT"
seed_blocks_and_offers "$db30"
seed_invitations_and_probe "$db30"
seed_sentinels "$db30"
# block-1's card was drawn; block-4's never was (no_response, unseen); block-7's
# was drawn and then answered "I was focused".
sqlite3 "$db30" "UPDATE work_block_intervention SET card_seen_at = $((T0 + 605)) WHERE block_id IN ('block-1');
                 UPDATE work_block_intervention SET card_seen_at = $((T0 + 63605)) WHERE block_id = 'block-7';"

pasted="$work/Downloads"
mkdir -p "$pasted"
cp "$export_script" "$pasted/export_cohort_evidence.sh"
chmod 644 "$pasted/export_cohort_evidence.sh"
[[ ! -x "$pasted/export_cohort_evidence.sh" ]] || fail "the pasted copy should not be executable"
( cd "$pasted" && VELVT_DATABASE_PATH="$db30" "$tester_bash" export_cohort_evidence.sh ) \
  > "$work/stdout30.txt" 2>&1 \
  || fail "bash export_cohort_evidence.sh failed from a pasted copy:$(printf '\n')$(cat "$work/stdout30.txt")"

stem="$pasted/velvt-cohort-$(date -u +%Y-%m-%d)"
for suffix in "" -decisions -blocks -invitations -explain -corrections -meta; do
  [[ -f "$stem$suffix.csv" ]] || fail "missing $(basename "$stem$suffix.csv")"
done

# 1. Headers are exactly the column lists, in order, and every row agrees.
check_header() {
  local actual
  actual="$(head -1 "$1")"
  [[ "$actual" == "$2" ]] || {
    echo "expected: $2" >&2
    echo "actual:   $actual" >&2
    fail "header drifted in $(basename "$1")"
  }
  assert_arity "$1"
}
check_header "$stem.csv" "$OFFERS_HEADER"
check_header "$stem-decisions.csv" "$DECISIONS_HEADER"
check_header "$stem-blocks.csv" "$BLOCKS_HEADER"
check_header "$stem-invitations.csv" "$INVITATIONS_HEADER"
check_header "$stem-explain.csv" "$EXPLAIN_HEADER"
check_header "$stem-corrections.csv" "$CORRECTIONS_HEADER"

# 2. The offer rows carry the right arithmetic and card_seen buckets.
python3 - "$stem.csv" <<'PY' || fail "per-offer rows are wrong"
import csv, sys
rows = {r["block_id"]: r for r in csv.DictReader(open(sys.argv[1]))}
T0 = 1800000000
expected = {
    # started_at, ended_at, total_paused_seconds, remaining_seconds_at_offer, card_seen_at, card_seen
    "block-1": (T0, T0 + 3000, 0, 2400, T0 + 605, "seen"),
    "block-2": (T0 + 10000, T0 + 13300, 300, 2400, "", "unseen"),
    "block-3": (T0 + 20000, T0 + 23000, 0, 2300, "", "unseen"),
    "block-4": (T0 + 30000, T0 + 31800, 0, 1400, "", "unseen"),
    "block-7": (T0 + 60000, T0 + 63000, 0, 0, T0 + 63605, "seen"),
}
problems = []
if sorted(rows) != sorted(expected):
    problems.append(f"rows: {sorted(rows)}")
for block, want in expected.items():
    row = rows.get(block)
    if row is None:
        continue
    got = (row["started_at"], row["ended_at"], row["total_paused_seconds"],
           row["remaining_seconds_at_offer"], row["card_seen_at"], row["card_seen"])
    if got != tuple(str(v) for v in want):
        problems.append(f"{block}: expected {want}, got {got}")
if problems:
    raise SystemExit("\n".join(problems))
PY

# 3. The decision log: every row, abstentions included, policy_version kept,
#    block timings joined, NULL outcome left empty rather than imputed.
python3 - "$stem-decisions.csv" <<'PY' || fail "decision-log rows are wrong"
import csv, sys
rows = {r["decision_id"]: r for r in csv.DictReader(open(sys.argv[1]))}
T0 = 1800000000
assert sorted(rows) == [f"d-0{i}" for i in range(1, 8)], sorted(rows)
assert {r["policy_version"] for r in rows.values()} == {"2"}, rows
assert rows["d-01"]["gate_verdict"] == "abstained_warmup", rows["d-01"]
assert rows["d-05"]["anchor_seen_within_600s"] == "", rows["d-05"]
assert rows["d-02"]["anchor_seen_within_600s"] == "1", rows["d-02"]
assert rows["d-02"]["propensity"] == "1.0", rows["d-02"]
assert rows["d-03"]["block_total_paused_seconds"] == "300", rows["d-03"]
assert rows["d-06"]["block_ended_at"] == str(T0 + 40100), rows["d-06"]
PY

# 4. Every declared block, including the ones that never produced an offer.
python3 - "$stem-blocks.csv" <<'PY' || fail "per-block rows are wrong"
import csv, sys
rows = {r["block_id"]: r for r in csv.DictReader(open(sys.argv[1]))}
assert sorted(rows) == [f"block-{i}" for i in range(1, 8)], sorted(rows)
assert rows["block-5"]["phase"] == "abandoned", rows["block-5"]
assert rows["block-6"]["origin"] == "invitation", rows["block-6"]
assert rows["block-6"]["ended_at"] == "", rows["block-6"]
assert rows["block-2"]["total_paused_seconds"] == "300", rows["block-2"]
PY

# 5. Invitations, the explain weeks and the meta file.
python3 - "$stem-invitations.csv" "$stem-explain.csv" "$stem-meta.csv" <<'PY' || fail "invitations/explain/meta are wrong"
import csv, sys
invitations = list(csv.DictReader(open(sys.argv[1])))
assert [r["outcome"] for r in invitations] == ["accepted", "dismissed", "offered"], invitations
assert "local_date" not in invitations[0], "local_date would give away the time zone"
explain = list(csv.DictReader(open(sys.argv[2])))
# block-1 .. block-7 all start in the week of Monday 2027-01-11 (UTC here).
# Delivered = offers not held by Focus or demotion: block-1, block-4, block-7.
assert explain == [{"week_start_local_date": "2027-01-11", "taps": "2",
                    "delivered_interventions": "3", "blocks_declared": "7"}], explain
meta = {r["key"]: r["value"] for r in csv.DictReader(open(sys.argv[3]))}
assert meta["schema_version"] == "36", meta
assert meta["decision_log"] == "present", meta
assert meta["invitations"] == "present", meta
assert meta["explain_probe"] == "present", meta
assert meta["card_seen_recorded_since"] == str(1800000000 - 86400), meta
assert meta["invitations_enabled"] == "0", meta
assert meta["exported_at"].isdigit(), meta
assert meta["export_format"] == "3", meta
assert meta["corrections"] == "present", meta
PY

# 5b. The corrections file: the two rules seed_sentinels wrote, as counts, and
#     nothing that names them. An app rule is written with correction_count 1.
[[ "$(tail -n +2 "$stem-corrections.csv")" == "$(printf 'app,FOCUS_WORK,1,1\nwindow,REFERENCE,1,')" ]] \
  || fail "corrections rows are wrong:$(printf '\n')$(cat "$stem-corrections.csv")"

# 6. The privacy promise, over every file the tester sends, and their terminal.
assert_no_sentinel "$stem".csv "$stem"-*.csv
for sentinel in "$S_INTENTION" "$S_APPNAME" "$S_DISPLAY" "$S_LABEL"; do
  grep -qF -- "$sentinel" "$work/stdout30.txt" && fail "sentinel leaked into the script's own output: $sentinel"
done
# The sentinels really were in the database, or the greps above pass for the
# wrong reason and this test proves nothing.
for probe in \
  "SELECT COUNT(*) FROM work_block WHERE intention LIKE '%$S_INTENTION%';" \
  "SELECT COUNT(*) FROM raw_event_buffer WHERE local_name_suggestion = '$S_APPNAME';" \
  "SELECT COUNT(*) FROM abstraction_map WHERE display_name = '$S_MAPNAME';"; do
  count="$(sqlite3 "$db30" "$probe")"
  [[ "$count" -ge 1 ]] || fail "seeding failed, the leak test would be vacuous: $probe"
done
# And no file carries observation rows: the one observation is at T0+10.
grep -qF "$((T0 + 10))" "$stem".csv "$stem"-*.csv && fail "an observation timestamp was exported"

# 7. The disclosure the participant reads names what the files contain. It is
#    hard-wrapped, so match against a whitespace-folded copy.
folded="$(tr '\n' ' ' < "$work/stdout30.txt" | tr -s ' ')"
for phrase in "block start and end times" "total time paused" \
              "how much of the block was left when the offer fired" \
              "when the nudge card was first on screen" \
              "Times are plain epoch seconds." \
              "the policy version" "every session you started" \
              "named by the date of its Monday" \
              "It does not say which apps or windows they were." \
              "Please send every file listed above."; do
  grep -qF -- "$phrase" <<<"$folded" || fail "disclosure omits: $phrase"
done
grep -qF "Does NOT contain: your block intentions, app names, window titles, URLs, filenames, or anything you typed or read." \
  <<<"$folded" || fail "the negative disclosure went missing"

# 8. The analyser reads the folder the tester's files landed in.
participant="$work/cohort/p30"
mkdir -p "$participant"
cp "$stem".csv "$stem"-*.csv "$participant/"
python3 "$analyze" --json "$participant" > "$work/analysis30.json"
python3 - "$work/analysis30.json" <<'PY' || fail "exporter/analyser round trip (protocol 30)"
import json, sys
r = json.load(open(sys.argv[1]))
assert r["data_quality"]["malformed"] == [], r["data_quality"]
assert r["participants"]["analysed"] == 1, r["participants"]
assert r["policy"]["decisions_by_policy_version"] == {"2": 7}, r["policy"]
assert r["policy"]["interventions_by_attribution"] == {"2": 5}, r["policy"]
d = r["decisions_recorded"]
assert (d["total"], d["delivered"], d["withheld"]) == (5, 3, 2), d
assert r["primary_outcome"]["eligible_decision_points"] == 3, r["primary_outcome"]
assert r["card_seen"]["no_response"] == {"seen": 0, "unseen": 1, "unknown": 0}, r["card_seen"]
assert r["blocks_per_participant"]["per_participant"]["p30"]["blocks_declared"] == 7, r["blocks_per_participant"]
inv = r["invitation_acceptance"]
assert (inv["accepted"], inv["terminal"], inv["unresolved_offered_excluded"]) == (1, 2, 1), inv
assert inv["participants_with_invitations_off"] == ["p30"], inv
ex = r["explain_tap_rate"]
assert (ex["taps"], ex["delivered_interventions"]) == (2, 3), ex
corr = r["corrections_per_participant"]["per_participant"]["p30"]
assert (corr["app_scoped_corrections"], corr["applications_with_an_app_rule"],
        corr["window_rules"]) == (1, 1, 1), corr
PY

# 9. There is still no `SELECT *` anywhere in the exporter's executable text.
if sed 's/[[:space:]]*#.*$//' "$export_script" | grep -qiE 'select[[:space:]]+\*'; then
  fail "a SELECT * appeared in the exporter"
fi

# ===========================================================================
# B. Protocol 28 (1.0.9): no card_seen_at column. The export must still work
#    and say "unknown", never fail and never say "unseen".
# ===========================================================================
db28="$work/p28.sqlite3"
migrate_fixture_db "$db28" "$FIXTURE_MIGRATIONS_PROTOCOL_28" "$INSTALLED_AT"
seed_blocks_and_offers "$db28"
seed_invitations_and_probe "$db28"
seed_sentinels "$db28"
[[ "$(sqlite3 "$db28" "SELECT COUNT(*) FROM pragma_table_info('work_block_intervention') WHERE name = 'card_seen_at';")" == "0" ]] \
  || fail "the protocol 28 fixture should not have card_seen_at"
out28="$work/p28/export.csv"
mkdir -p "$work/p28"
run_export "$db28" "$out28" "$work/stdout28.txt"
check_header "$out28" "$OFFERS_HEADER"
check_header "$work/p28/export-decisions.csv" "$DECISIONS_HEADER"
check_header "$work/p28/export-blocks.csv" "$BLOCKS_HEADER"
python3 - "$out28" "$work/p28/export-meta.csv" <<'PY' || fail "protocol 28 card_seen handling"
import csv, sys
rows = list(csv.DictReader(open(sys.argv[1])))
assert len(rows) == 5, rows
assert {r["card_seen_at"] for r in rows} == {""}, rows
assert {r["card_seen"] for r in rows} == {"unknown"}, rows
meta = {r["key"]: r["value"] for r in csv.DictReader(open(sys.argv[2]))}
assert meta["schema_version"] == "31", meta
assert meta["card_seen_recorded_since"] == "", meta
assert meta["decision_log"] == "present", meta
PY
assert_no_sentinel "$work/p28"/*.csv
python3 "$analyze" --json "$work/p28" > "$work/analysis28.json"
python3 - "$work/analysis28.json" <<'PY' || fail "exporter/analyser round trip (protocol 28)"
import json, sys
r = json.load(open(sys.argv[1]))
assert r["data_quality"]["malformed"] == [], r["data_quality"]
assert r["card_seen"]["no_response"] == {"seen": 0, "unseen": 0, "unknown": 1}, r["card_seen"]
assert r["card_seen"]["delivered"] == {"seen": 0, "unseen": 0, "unknown": 3}, r["card_seen"]
assert r["decisions_recorded"]["delivered"] == 3, r["decisions_recorded"]
PY

# ===========================================================================
# C. 28 -> 30 upgrade. NULL on a row offered before this Mac applied 0032 is
#    "unknown"; NULL after it is "unseen"; a timestamp is "seen".
# ===========================================================================
dbup="$work/upgrade.sqlite3"
UPGRADED_AT=$((T0 + 65000))
migrate_fixture_db "$dbup" "$FIXTURE_MIGRATIONS_PROTOCOL_28" "$INSTALLED_AT"
seed_blocks_and_offers "$dbup"          # every offer here is before the upgrade
migrate_fixture_db "$dbup" "$FIXTURE_MIGRATIONS_PROTOCOL_30" "$UPGRADED_AT"
sqlite3 "$dbup" <<SQL
INSERT INTO work_block (block_id, phase, purpose, intensity, planned_duration_seconds,
    started_at, total_paused_seconds, ended_at, intention_expires_at, origin)
VALUES
 ('block-8','completed','deep_work','medium',1800,$((T0 + 70000)),0,$((T0 + 71800)),$((T0 + 156400)),'manual'),
 ('block-9','completed','deep_work','medium',1800,$((T0 + 80000)),0,$((T0 + 81800)),$((T0 + 166400)),'manual');
INSERT INTO work_block_intervention (block_id, offered_at, action_id, anchor_category,
    switch_count, window_seconds, outcome, outcome_at, salience, card_seen_at)
VALUES
 ('block-8',$((T0 + 70400)),'protect_next_10','FOCUS_WORK',3,600,'no_response',$((T0 + 71800)),'normal',NULL),
 ('block-9',$((T0 + 80400)),'protect_next_10','FOCUS_WORK',3,600,'no_response',$((T0 + 81800)),'normal',$((T0 + 80410)));
INSERT INTO intervention_decision_log (decision_id, occurred_at, block_id, policy_version,
    anchor_category, switch_count, elapsed_seconds, remaining_seconds, gate_verdict, propensity)
VALUES
 ('d-08',$((T0 + 70400)),'block-8',2,'FOCUS_WORK',3,400,1400,'offered',1.0),
 ('d-09',$((T0 + 80400)),'block-9',2,'FOCUS_WORK',3,400,1400,'offered',1.0);
SQL
mkdir -p "$work/upgrade"
run_export "$dbup" "$work/upgrade/export.csv" "$work/stdoutup.txt"
python3 - "$work/upgrade/export.csv" <<'PY' || fail "card_seen after a 28 -> 30 upgrade"
import csv, sys
rows = {r["block_id"]: r["card_seen"] for r in csv.DictReader(open(sys.argv[1]))}
assert rows["block-4"] == "unknown", rows   # no_response before the upgrade
assert rows["block-1"] == "unknown", rows   # delivered before the upgrade
assert rows["block-8"] == "unseen", rows    # no_response after the upgrade
assert rows["block-9"] == "seen", rows
PY
python3 "$analyze" --json "$work/upgrade" > "$work/analysisup.json"
python3 - "$work/analysisup.json" <<'PY' || fail "analysis after a 28 -> 30 upgrade"
import json, sys
r = json.load(open(sys.argv[1]))
assert r["card_seen"]["no_response"] == {"seen": 1, "unseen": 1, "unknown": 1}, r["card_seen"]
PY

# ===========================================================================
# D. Protocol 25 (1.0.1): no decision log, no origin, no invitations, no
#    probe. The export still runs, writes what exists, says what is missing,
#    and removes stale companions from an earlier run in the same folder.
# ===========================================================================
db25="$work/p25.sqlite3"
migrate_fixture_db "$db25" "$FIXTURE_MIGRATIONS_PROTOCOL_25" "$INSTALLED_AT"
sqlite3 "$db25" <<SQL
INSERT INTO work_block (block_id, phase, intention, purpose, intensity,
    planned_duration_seconds, started_at, total_paused_seconds, ended_at, intention_expires_at)
VALUES ('old-1','completed','$S_INTENTION','deep_work','medium',3000,$T0,0,$((T0 + 3000)),$((T0 + 86400)));
INSERT INTO work_block_intervention (block_id, offered_at, action_id, anchor_category,
    switch_count, window_seconds, outcome, outcome_at, salience)
VALUES ('old-1',$((T0 + 600)),'protect_next_10','FOCUS_WORK',4,600,'returned',$((T0 + 700)),'normal');
SQL
mkdir -p "$work/p25"
echo "stale" > "$work/p25/export-decisions.csv"
echo "stale" > "$work/p25/export-explain.csv"
run_export "$db25" "$work/p25/export.csv" "$work/stdout25.txt"
[[ ! -e "$work/p25/export-decisions.csv" ]] || fail "a stale decisions file survived"
[[ ! -e "$work/p25/export-explain.csv" ]] || fail "a stale explain file survived"
[[ ! -e "$work/p25/export-invitations.csv" ]] || fail "an invitations file was written with no table"
check_header "$work/p25/export.csv" "$OFFERS_HEADER"
check_header "$work/p25/export-blocks.csv" "$BLOCKS_HEADER"
[[ "$(field_of "$work/p25/export-blocks.csv" old-1 origin)" == "" ]] || fail "origin invented on a pre-0022 database"
[[ "$(field_of "$work/p25/export.csv" old-1 card_seen)" == "unknown" ]] || fail "card_seen on a pre-0032 database"
[[ "$(meta_of "$work/p25/export-meta.csv" decision_log)" == "absent" ]] || fail "meta: decision_log"
[[ "$(meta_of "$work/p25/export-meta.csv" invitations)" == "absent" ]] || fail "meta: invitations"
[[ "$(meta_of "$work/p25/export-meta.csv" invitations_enabled)" == "" ]] || fail "meta: invitations_enabled"
grep -qF "no decisions file was" <(tr '\n' ' ' < "$work/stdout25.txt" | tr -s ' ') \
  || fail "the tester was not told the decision log is missing"
assert_no_sentinel "$work/p25"/*.csv
python3 "$analyze" --json "$work/p25" > "$work/analysis25.json"
python3 - "$work/analysis25.json" <<'PY' || fail "a pre-0026 export was analysed"
import json, sys
r = json.load(open(sys.argv[1]))
assert r["participants"]["analysed"] == 0, r["participants"]
reason = r["participants"]["excluded_whole"]["p25"]
assert "predates migration 0026" in reason, reason
assert r["decisions_recorded"]["total"] == 0, r["decisions_recorded"]
PY

# ===========================================================================
# F. Corrections (2026-08-17 measure 3) on protocol 28 (1.0.9) and protocol 31
#    (develop, with salted keys and the egress ledger). The same rows on both:
#    five app rules and four window rules, one of each with a category nothing
#    could have written through the IPC layer, plus a block-scoped "Wrong
#    category" reply that is not a classification rule and is not counted.
#    Only columns 0017 already had are written, so one seed fits both schemas.
# ===========================================================================
seed_corrections() {
  sqlite3 "$1" <<SQL
INSERT INTO personal_app_override (app_key_hash, category, activity_name, correction_count)
VALUES
 ('dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd01','FOCUS_WORK',NULL,3),
 ('dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd02','FOCUS_WORK','$S_OVERRIDE two',1),
 ('dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd03','SOCIAL_FEED',NULL,2),
 ('dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd04','$S_CATEGORY',NULL,1);
INSERT INTO personal_override (key_hash, category, activity_name)
VALUES
 ('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee01','FOCUS_WORK',NULL),
 ('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee02','FOCUS_WORK','$S_URLHOST two'),
 ('eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee03','$S_CATEGORY',NULL);
INSERT INTO work_block_category_correction (block_id, category, counts_as_category, corrected_at)
VALUES ('block-4','REFERENCE','FOCUS_WORK',$((T0 + 30500)));
SQL
}

for proto in 28 31; do
  case "$proto" in
    28) last="$FIXTURE_MIGRATIONS_PROTOCOL_28" ;;
    31) last="$FIXTURE_MIGRATIONS_PROTOCOL_31" ;;
  esac
  dbc="$work/corrections$proto.sqlite3"
  migrate_fixture_db "$dbc" "$last" "$INSTALLED_AT"
  seed_blocks_and_offers "$dbc"
  seed_sentinels "$dbc"
  seed_corrections "$dbc"
  outdir="$work/cohort-corrections/c$proto"
  mkdir -p "$outdir"
  run_export "$dbc" "$outdir/c$proto.csv" "$work/stdoutc$proto.txt"
  check_header "$outdir/c$proto-corrections.csv" "$CORRECTIONS_HEADER"
  # seed_sentinels adds one FOCUS_WORK app rule (count 1) and one REFERENCE
  # window rule to the rows above.
  expected="$(printf '%s\n' \
    'app,FOCUS_WORK,3,5' \
    'app,SOCIAL_FEED,1,2' \
    'app,unrecognized,1,1' \
    'window,FOCUS_WORK,2,' \
    'window,REFERENCE,1,' \
    'window,unrecognized,1,')"
  actual="$(tail -n +2 "$outdir/c$proto-corrections.csv")"
  [[ "$actual" == "$expected" ]] || {
    echo "expected:" >&2; echo "$expected" >&2
    echo "actual:" >&2; echo "$actual" >&2
    fail "protocol $proto corrections rows"
  }
  [[ "$(meta_of "$outdir/c$proto-meta.csv" corrections)" == "present" ]] \
    || fail "protocol $proto meta: corrections"
  [[ "$(meta_of "$outdir/c$proto-meta.csv" schema_version)" == "$last" ]] \
    || fail "protocol $proto fixture is not at migration $last"
  # The sentinel category and both typed names really are in the database.
  [[ "$(sqlite3 "$dbc" "SELECT COUNT(*) FROM personal_app_override WHERE category = '$S_CATEGORY';")" == "1" ]] \
    || fail "seeding failed, the category leak test would be vacuous"
  assert_no_sentinel "$outdir"/*.csv "$work/stdoutc$proto.txt"
  # No key digest leaves either: none of the 64-character keys seeded here.
  grep -qE '[0-9a-e]{64}' "$outdir/c$proto-corrections.csv" && fail "a key digest was exported"

  python3 "$analyze" --json "$outdir" > "$work/analysisc$proto.json"
  python3 - "$work/analysisc$proto.json" "c$proto" <<'PY' || fail "exporter/analyser round trip (corrections, protocol $proto)"
import json, sys
r = json.load(open(sys.argv[1]))
name = sys.argv[2]
assert r["data_quality"]["malformed"] == [], r["data_quality"]
c = r["corrections_per_participant"]
assert c["participants_measured"] == 1, c
entry = c["per_participant"][name]
# 3 + 1 + 1 (FOCUS_WORK) + 2 (SOCIAL_FEED) + 1 (unrecognized): the block-scoped
# reply is not among them.
assert entry["app_scoped_corrections"] == 8, entry
assert entry["applications_with_an_app_rule"] == 5, entry
assert entry["window_rules"] == 4, entry
assert c["app_scoped_corrections_by_category"] == {
    "FOCUS_WORK": 5, "SOCIAL_FEED": 2, "unrecognized": 1}, c
assert c["not_measurable_for"] == [], c
PY
done

# The exporter's closed category set, the analyser's and the service's must be
# one set. The service's is the match in `override_label_for_category`, the
# only gate every correction path goes through.
python3 - "$repo_root/rust-service/src/abstraction/engine.rs" "$export_script" "$analyze" <<'PY' \
  || fail "the correction category sets drifted apart"
import ast, re, sys
engine, exporter, analyzer = (open(p).read() for p in sys.argv[1:])
body = engine.split("fn override_label_for_category", 1)[1].split("\n}\n", 1)[0]
service = set(re.findall(r'"([A-Z_]+)"\s*=>', body))
listed = exporter.split("WHEN r.category IN (", 1)[1].split(")", 1)[0]
exported = set(re.findall(r"'([A-Z_]+)'", listed))
tree = ast.parse(analyzer)
analysed = next(
    set(ast.literal_eval(node.value))
    for node in tree.body
    if isinstance(node, ast.Assign)
    and any(getattr(t, "id", None) == "CORRECTION_CATEGORIES" for t in node.targets)
)
assert len(service) == 8, service
assert exported == service, (exported ^ service)
assert analysed == service | {"unrecognized"}, (analysed ^ (service | {"unrecognized"}))
PY

# ===========================================================================
# G. 1.0.0 (migration 0016) had no app-scoped corrections, so measure 3 does
#    not exist there: no corrections file, `absent` in the meta file, and a
#    stale one from an earlier run in the same folder is removed.
# ===========================================================================
db16="$work/p16.sqlite3"
migrate_fixture_db "$db16" 16 "$INSTALLED_AT"
sqlite3 "$db16" <<SQL
INSERT INTO work_block (block_id, phase, purpose, intensity,
    planned_duration_seconds, started_at, total_paused_seconds, ended_at, intention_expires_at)
VALUES ('old-1','completed','deep_work','medium',3000,$T0,0,$((T0 + 3000)),$((T0 + 86400)));
SQL
mkdir -p "$work/p16"
echo "stale" > "$work/p16/export-corrections.csv"
run_export "$db16" "$work/p16/export.csv" "$work/stdout16.txt"
[[ ! -e "$work/p16/export-corrections.csv" ]] || fail "a corrections file was left on a 1.0.0 database"
[[ "$(meta_of "$work/p16/export-meta.csv" corrections)" == "absent" ]] || fail "meta: corrections on 1.0.0"

# ===========================================================================
# E. Older than protocol 25 still stops, loudly.
# ===========================================================================
db15="$work/p15.sqlite3"
migrate_fixture_db "$db15" 15 "$INSTALLED_AT"
if VELVT_DATABASE_PATH="$db15" "$tester_bash" "$export_script" "$work/p15.csv" > "$work/stdout15.txt" 2>&1; then
  fail "a pre-protocol-25 database was exported"
fi
grep -qF "predates protocol 25" "$work/stdout15.txt" || fail "the pre-protocol-25 error went missing"

echo "export_cohort_evidence_test.sh: OK"
