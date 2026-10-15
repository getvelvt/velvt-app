-- What became of the OS notification for each drift offer (protocol 35).
--
-- WHY. Swift posts the offer as a notification and, until this migration,
-- wrote what happened only to the unified log on the tester's Mac. The
-- service counted every offer that was not withheld as delivered, so an offer
-- that never rang (notifications turned off, an alert style of None, a quiet
-- offer, an offer withdrawn while the permission check was in flight) was
-- stored exactly like one that did.
--
-- `delivery_channel` is the channel Swift reported, from a closed vocabulary:
--   notification_posted  the notification centre accepted the request and the
--                        alert style for Velvt shows it on screen
--   alert_style_none     accepted, but the alert style for Velvt is None:
--                        listed in Notification Center, never shown on screen
--   permission_denied    notifications are turned off for Velvt
--   permission_unknown   permission never answered, or could not be read
--   rejected             the notification centre refused the request
--   withdrawn            the offer was over before the notification centre
--                        had it: it ended while the attempt was still
--                        waiting on the permission check, or the post landed
--                        after it ended and was taken down at once
--   quiet                a quiet offer: in-app card only, by design
--   stale                not posted: `offered_at` (the time of the evidence
--                        the gate decided on, not of the decision) was more
--                        than two minutes past when the app first saw it
-- The two posted channels are final: a later report for the same offer is
-- ignored once either is stored. `withdrawn` and `stale` fill only an empty
-- channel, so neither replaces what an attempt found, nor what an earlier
-- process reported before a relaunch. Any other channel is replaced by the
-- next report, so an offer that met a denied permission and was posted once
-- the person turned notifications on reads `notification_posted`.
--
-- `delivery_reported_at` is when the service received the channel it holds.
--
-- Deliberately nullable columns and NOT new `outcome` values, for the reasons
-- 0032 gives for `card_seen_at`: `outcome` is a closed vocabulary of answers,
-- delivery is a fact orthogonal to the answer, and widening the outcome CHECK
-- would force a table rebuild. No existing definition changes: the
-- pre-registered delivered denominator still counts what it counted, and the
-- channel is reported beside it. NULL means nothing was reported, which
-- includes every row written before this migration and every offer Swift never
-- saw (a decision held for Focus or withheld under demotion). It does not mean
-- "not shown".
--
-- Local only, like the rest of this table: no upload path reads it.

ALTER TABLE work_block_intervention ADD COLUMN delivery_channel TEXT
    CHECK(delivery_channel IS NULL OR delivery_channel IN (
        'notification_posted',
        'alert_style_none',
        'permission_denied',
        'permission_unknown',
        'rejected',
        'withdrawn',
        'quiet',
        'stale'
    ));

ALTER TABLE work_block_intervention ADD COLUMN delivery_reported_at INTEGER;
