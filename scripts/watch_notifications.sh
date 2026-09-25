#!/bin/bash
# Watch what actually happens to every Velvt notification delivery attempt.
#
# Layer 3 of the delivery path — whether macOS displayed the notification — is
# the one the database cannot answer. `work_block_intervention` is written by
# Rust when the offer is *recorded*; Swift's delivery outcome goes only to the
# unified log and is never written back. So an offer that never rang is stored
# exactly like one that did.
#
# `OSLogNotificationDeliveryReporter` (swift-client/Sources/VelvtMac/Delivery/
# NotificationDeliveryReporter.swift) emits one line per attempt under
# subsystem com.velvt.mac, category NotificationDelivery. This reads it.
#
# Usage:
#   ./scripts/watch_notifications.sh            # live stream (leave running)
#   ./scripts/watch_notifications.sh 2h         # replay the last 2 hours
#   ./scripts/watch_notifications.sh 3d         # replay the last 3 days
#
# Note: the unified log keeps roughly a few days. Anything older is gone, and an
# empty replay means "no record", never "no delivery".

set -uo pipefail

SUBSYSTEM="com.velvt.mac"
PREDICATE="subsystem == \"$SUBSYSTEM\""

cat <<'BANNER'
Watching Velvt notification delivery.

What you are looking for, verbatim as the app emits it:

  notification_delivered                     the request reached
                                             UNUserNotificationCenter and was
                                             accepted. THIS is layer 3 passing.
  notification_suppressed_by_salience        a quiet offer: in-app card only,
                                             by design, not a fault
  error_code=notification_permission_blocked notifications were not authorised;
                                             status= says which
  error_code=notification_centre_rejected    the centre refused the request
  error_code=notification_add_rejected       the same refusal from the scheduler,
                                             carrying domain= and code=

Nothing at all means the drift gate abstained and no offer was ever made — a
different problem, one layer earlier. Check that with:

  sqlite3 ~/.velvt/velvt-service.sqlite3 \
    "SELECT gate_verdict, COUNT(*) FROM intervention_decision_log GROUP BY 1;"

BANNER

if [ $# -eq 0 ]; then
    echo "Streaming live. Ctrl-C to stop."
    echo "Now declare a work block and drift out of it."
    echo
    exec log stream --predicate "$PREDICATE" --style compact --level info
fi

WINDOW="$1"
echo "Replaying the last $WINDOW."
echo
log show --predicate "$PREDICATE" --last "$WINDOW" --style compact --info 2>/dev/null \
    | grep -v '^Timestamp' \
    | grep -v '^Filtering' \
    || true

echo
echo "--- delivery attempts only ---"
log show --predicate "$PREDICATE" --last "$WINDOW" --style compact --info 2>/dev/null \
    | grep -E 'notification_delivered|notification_suppressed_by_salience|notification_permission_blocked|notification_centre_rejected|notification_add_rejected' \
    || echo "(no delivery attempts recorded in this window)"
