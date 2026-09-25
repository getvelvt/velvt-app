# Sourced, not executed. Builds Velvt databases the way the service does, for
# the measurement tests.
#
# The service applies every embedded migration it has not applied yet and
# records each one in `schema_migration` with the time it ran
# (`rust-service/src/persistence/sqlite.rs`, `run_migrations`). Replaying the
# shipped .sql files here keeps the fixtures moving with the schema instead of
# pinning a hand-written copy of it, and recording `created_at` lets a test
# stage an upgrade: migrate to one build's schema at one instant, write rows,
# then migrate to a later build's schema at a later instant.
#
# The last migration each shipped schema carries, by IPC protocol:
#   protocol 25  1.0.0 (0016)  and 1.0.1 (0017)
#   protocol 28  1.0.9 (0031)  first build on drift policy v2
#   protocol 30  1.0.11 (0036) the first with card_seen_at (0032, protocol 29)
# shellcheck shell=bash

FIXTURE_MIGRATIONS_PROTOCOL_25=17
FIXTURE_MIGRATIONS_PROTOCOL_28=31
FIXTURE_MIGRATIONS_PROTOCOL_30=36

# migrate_fixture_db DB LAST_VERSION APPLIED_AT
#   Applies every migration numbered <= LAST_VERSION that DB has not recorded,
#   and records each with created_at = APPLIED_AT (epoch seconds).
migrate_fixture_db() {
  local db="$1" last="$2" applied_at="$3"
  local migrations="${FIXTURE_MIGRATIONS_DIR:?set FIXTURE_MIGRATIONS_DIR}"
  sqlite3 "$db" "CREATE TABLE IF NOT EXISTS schema_migration (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      version INTEGER NOT NULL UNIQUE,
      name TEXT NOT NULL,
      created_at INTEGER NOT NULL DEFAULT (unixepoch())
  );"
  local migration name version done_already
  for migration in "$migrations"/*.sql; do
    name="$(basename "$migration")"
    version=$((10#${name%%_*}))
    (( version <= last )) || continue
    done_already="$(sqlite3 "$db" "SELECT COUNT(*) FROM schema_migration WHERE version = $version;")"
    [[ "$done_already" == "0" ]] || continue
    {
      cat "$migration"
      printf "\nINSERT INTO schema_migration(version, name, created_at) VALUES (%d, '%s', %d);\n" \
        "$version" "$name" "$applied_at"
    } | sqlite3 "$db" || { echo "FAIL: migration failed: $migration" >&2; return 1; }
  done
}
