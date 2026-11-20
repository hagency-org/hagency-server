# Pool supply observations and recipient delivery

Status: proposed cross-repository contract. Server #73 and #75 and Desktop #187 and #189 cover retained allocation evidence and authenticated reads. This document specifies the remaining provider-wide source and unsolicited delivery.

## Reproduce the missing distinction

Publish a pool with two approved Agents A and B. Exhaust A's allocation by settling a call at its ceiling. Read the pool directory and each Agent's current allowance. A's confirmed lower bound proves its own exhaustion. B can still have a valid reservation. The directory currently emits an absent native available-token balance. A null balance, failure to allocate more, and exhaustion of A all leave the usability of B undetermined. A workspace-wide outage alert computed from any of these three inputs would incorrectly reach B's users.

A second case uses two resources under the same Codex login with different selected models. A window can be associated with a named limit bucket. A 100% observation for that bucket needs a proven account, model and resource binding before H1 can assign it to either grant. Expiry of a reported reset time permits a fresh read. A successful request or a fresh positive provider observation is required to announce restoration.

## Sources and affected operations

| Authoritative input | Required binding | Affected operation |
|---|---|---|
| Known settled consumption meets an allocation ceiling | Current allocation, applied revision and period | This allocation's new starts |
| A native capacity refusal for a new allocation or renewal | Exact resource, current grant and operation | The refused new allocation or renewal |
| Explicit provider refusal with a documented scope and a bounded lifetime | Current account proof, local resource revision, selected model or an explicitly account-wide scope, current executor device and generation | Starts on grants covered by that scope |
| Provider timeout, stale observation, missing balance or unspecified bucket-to-model mapping | Insufficient evidence | Show availability as unknown; preserve every reservation |

The owner executor already verifies the canonical resource binding before collecting provider quota. Extend that path to publish only a scope, a closed reason, an observation revision and its freshness window. Keep account subjects, arbitrary provider strings and raw provider payloads on the device. On the server, verify the enrolled executor, current resource owner, contribution and generation. A restarted process must recover its observation epoch. A delayed update from an old generation, stale evidence, or a guessed model/bucket mapping must leave the current state unchanged. An observation reports availability and confers no start, retry, quota or cancellation authority.

## Persist and deliver

Store the authoritative state transition and an outbox invalidation in one transaction. Allocate the revision under the same per-scope lock. Coalesce repeated evidence with the same scope, operation and state. Model freshness and recovery separately so expiry changes the status to unknown. A later confirmed recovery supersedes an older pending outage.

Expand a confirmed supply scope to current grants and allocations. Check current owner or manager authority for private application details. For other users, check the original bound room generation and current Matrix membership, including the Agent's presence. Deliver only the affected Agent and generic status. Check entitlement once when expanding and again before delivery or read. A user who has left the room must not learn whether a previously observed pool has since recovered. Release database connections before Matrix I/O and fence the delivery result against the saved revision afterwards.

Use a durable recipient key derived from the supply scope, state revision, recipient and destination. Retry with the same transport transaction ID. A disconnected client resumes from an opaque authenticated cursor and re-reads current state. Desktop orders by revision, replaces the prior banner on recovery and respects system notification preferences. The owner or manager may open the original authenticated resource screen. A room-only participant never gets the private application ID. A button opening that screen does not apply, charge, replay work or raise capacity.

## Release evidence

1. Use one requester with two allocations and another requester sharing just one Agent. Exhaust one allocation; verify the unaffected allocation still replies.
2. Prevent new grants with all existing reservations still usable. Verify only the affected application/renewal surfaces show the refusal.
3. Simulate a provider refusal for one known model bucket, then an explicitly account-wide refusal. Use another provider account and a different model as negative controls. Capture the source, mapping and affected grant count without collecting credentials.
4. Repeat, reorder and delay the original observation across an executor restart. Inject loss of the recipient transport acknowledgment. Verify one visible transition per revision and no duplicate model call.
5. Remove membership or revoke a binding between enqueue and delivery. Verify the old recipient receives no private state. Return connectivity and verify a still-entitled account receives the latest state.
6. Apply a fresh positive observation and obtain an actual response using an existing approved allocation. Keep the observation and the successful execution as distinct evidence.

A resource owner manually pausing a whole pool would introduce a separate product action and audit reason. The proposed automatic source can be evaluated independently.
