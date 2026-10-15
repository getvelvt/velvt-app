-- Which account each queued upload belongs to.
--
-- WHY. Nothing in the upload queue said whose it was. An event acked while
-- account A was signed in sat in the in-memory batch, or in a pending or
-- failed `upload_batch`, and was sent by whichever bearer token the retry loop
-- held next. After Log Out and a sign-in to account B on the same Mac, A's
-- activity was stored in B's account; after A was deleted on another Mac, A's
-- queue here was stored in the next account to sign in.
--
-- `upload_batch.owner_user_id` is the account id the cloud gave in
-- `auth_success` for the session the batch was recorded under. A batch is sent
-- only while that account is signed in, and is discarded, not sent, once
-- another account is. NULL for every row written before this migration: the
-- service cannot tell whose those were, and they are sent under the next
-- session as before, which is the behaviour they were queued under.
--
-- `raw_event_buffer.upload_owner_user_id` is the same id on an upload-eligible
-- event, written when the event is recorded. An event acked but not yet in a
-- batch when the service stopped is re-batched at the next start
-- (`UploadBatcher::recover_unbatched`), before any session exists; this is how
-- it keeps its owner. NULL on every row that is not upload-eligible and on
-- every row written before this migration.
--
-- The id is the cloud's opaque account identifier. It is not drawn from the
-- Mac and is sent nowhere: the cloud already knows it from the bearer token,
-- and no outbound payload carries it (`BatchPayload` skips it when
-- serializing). It is not in any index: the queue is small and is read by
-- status, and ownership is decided row by row as each batch is read.

ALTER TABLE upload_batch ADD COLUMN owner_user_id TEXT;

ALTER TABLE raw_event_buffer ADD COLUMN upload_owner_user_id TEXT;
