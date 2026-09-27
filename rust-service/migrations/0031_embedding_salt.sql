-- The per-install key the hashed embedding features are computed under.
--
-- `semantic_embedding_cache` (0013) holds a 256-dimension sketch of
-- `"{application} [SEP] {window title}"`. The sketch cannot be inverted back
-- into a title, but until this migration it could be READ by anyone: the hash
-- family is written out in full in `abstraction/plugin.rs`, so a reader of the
-- source can embed a dictionary word and test whether its coordinates are
-- present in a stored row. A verifier reimplemented it from the source alone
-- and recovered 1,190 distinct real words from 448 of 512 rows of a live
-- database; on a synthetic title it returned exactly the four content words.
-- None of this was ever uploaded and none of it is uploadable -- no DTO has a
-- field the sketch or the salt could occupy -- so the exposure is to whoever
-- holds the file on disk. It is still the reason the claim that the schema
-- preserves "no way to recover the original raw string" was too strong.
--
-- Salting the feature hash with a value only this device holds does not make
-- the sketch safe to hand out. The salt sits in the same file as the sketch,
-- and it has to: the vectors must stay comparable across a restart, so the key
-- that produced them cannot be ephemeral. What it removes is the offline,
-- source-only attack -- the recovery has to be redone per device, with that
-- device's salt in hand, instead of once from a public source file.
--
-- That sentence is in the present tense because the reader exists, in the same
-- change as this file. `AbstractionMapRepo::embedding_salt` reads this row
-- inside a transaction and `main.rs` hands the value to
-- `EmbeddingSimilarityPlugin::builtin_salted`, so every sketch written after
-- this migration runs is computed under it. There is no unsalted path left to
-- fall back to: if the row cannot be read, startup disables Tier 2 rather than
-- passing `EmbeddingSalt::UNSALTED`, because an unsalted classifier writing
-- into a store this file describes as salted is the worse of the two failures.
--
-- Shipping the table without the reader would have cost the wipe below twice:
-- once here, and again when the wiring landed, since moving from the zero salt
-- to a real one is another change of vector space. The two are one commit for
-- that reason.
--
-- CONSEQUENCE, PLAINLY. A salt is a different vector space. Every vector
-- computed under the old one is meaningless against every vector computed under
-- the new one, so both stores that hold vectors are emptied below, and this is
-- what the user loses:
--
--   * `personal_semantic_prototype` is emptied. Learned prototypes reset and
--     `correction_count` resets with them, so a window the classifier had
--     learned by example has to be corrected again before it generalizes.
--   * Explicit corrections are NOT lost. `personal_override` and
--     `personal_app_override` (0017) are exact-match rungs that hold a category,
--     not a vector; they do not live in this space and are not touched here.
--   * `semantic_embedding_cache` is emptied. It is a cache: the cost is one
--     re-embedding the next time each window is observed, and nothing else.
--
-- The salt is generated once, here, by the database itself, and never leaves
-- the device. Structurally, not by filtering: no type in `upload::dto` has a
-- field it could occupy -- `BatchEventPayload` serializes an event id, a
-- timestamp, an abstraction type, a tier and a duration -- and a grep for
-- salt, embedding, prototype, vector and sketch across
-- `rust-service/src/upload/` returns nothing at all. (`semantic` returns one
-- hit, the English word inside "semantics" in a doc comment.) It cannot reach a
-- log either:
-- `EmbeddingSalt` implements `Debug` by hand to print `EmbeddingSalt(redacted)`
-- and implements no serializer at all, so there is no `?` or `%` field that
-- would carry it into a tracing line or a crash report.

CREATE TABLE embedding_salt (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    salt BLOB NOT NULL CHECK(length(salt) = 32),
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO embedding_salt(id, salt) VALUES (1, randomblob(32));

DELETE FROM semantic_embedding_cache;
DELETE FROM personal_semantic_prototype;
