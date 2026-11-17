# Parent-grant settlement, renewal and resource reconciliation

Date: 2026-10-16. Status: central close/settlement protocol implemented behind a
separate default-off gate; native settlement, renewal and resource amendment
remain unavailable. No running backend or deployment configuration was changed.

The smallest useful next implementation is **permanently fence one owner grant,
release only capacity proven unused by the native ledger, and publish that exact
settlement**. Start with the current children that cannot execute agents. Keep
unsupported legacy/execution liabilities held. Resource identity reconciliation
is a prerequisite to creating real grants against existing contributions.

## Current constraints verified in source

- Native `owner_capacity_grants` stores `allocated_tokens`, immutable `receipt`
  and `active`. It has no parent close, refund or renewal operation.
- `owner_resources::capacity::additional_commitments` always contributes the
  full parent `allocated_tokens` to the existing resource/account budget engine,
  including after ACL revocation or UTC month rollover. Merely setting `active=0`
  must never make the budget engine forget a liability.
- `owner_project_reservations` are children of that already-counted parent. They
  cannot execute or attach agents today, so their current release can safely
  retain zero. That assumption must become a checked schema/protocol invariant
  (`execution_enabled=false` and no attachment/dispatch rows) before adding execution; it must not remain an implicit comment.
- Central `revoke_management_grant` fences new allocation and queues child
  releases. It does not release the native parent. Central `state=revoked` and
  expired authorization are not proof that physical capacity is free.
- `recover_capacity_grant` returns an immutable historical receipt for the exact
  profile/installation. It intentionally does not revive authority or renew a hold.

Relevant implementation: `hagency-client/native/hagency-store/src/domain/owner_resources/`
(`capacity.rs` and its tests), native migration `073-owner-capacity-reservations.sql`,
and server `management_allocations.rs`. Legacy final usage remains in
`coordinator/settlements.rs`; its observations are lower bounds until explicitly
reconciled and it retains late usage. Do not treat that legacy path as an already
implemented settlement adapter for these new parents.

## Accounting invariant

Keep the original grant amount `A` and original receipt immutable. Add a current
parent liability `H` that is counted by the **same** existing budget engine.
Initially `H=A`. Closing the grant does not itself change `H`.

After a durable native fence, the ledger may reduce `H` to the sum of disjoint
liabilities: known consumed usage, unresolved/running reservations, and any
legacy/unknown commitments assigned to this parent. A child hold and the usage
inside it are not added twice. Children still consume their parent; they never
become a second top-level account commitment.

Only `max(A-H,0)` is known unused capacity returned from this grant. Do not call
`A-sum(central child projections)` available: central deliveries/results can be
missing or stale. `H` may exceed `A` if verified late usage reveals an overrun;
retain and charge that excess, block further overcommitment, and publish a new
correction. A previous refund display is not permission to ignore later usage.

Once the parent is fenced, the native ledger can release unassigned slack even
while a child remains unresolved, provided the child's **entire** possible hold
stays in `H`. If complete liability enumeration is unavailable, retain the full
original hold (or the greater known liability). No timeout estimates a refund.
UTC rollover alone never changes these amounts; old-period liability remains
separate until the accounting policy has authoritative evidence to settle it.

## Native close and settlement transaction

Introduce a non-deserializable `VerifiedGrantClosureDelivery`, or an OS-owner
`OwnerGrantCloseConsent` for a local-first withdrawal. Bind it to the existing
registry, authenticated profile, exact original grant digest and enrolled stable
installation. Current generation may be newer for cleanup; it cannot move the
grant to another profile/device or confer new allocation rights.

The owner can withdraw locally even while offline. A central owner withdrawal or
administrator contribution revocation can also deliver this reducing operation
automatically. Neither path requires a second human vote on each project. A
coordinator has only the authority granted to it; coordinator status alone does
not authorize owner-level parent changes. Only native code constructs the proof.

In one serialized SQLite writer transaction:

1. Resolve the original command ID/digest first. Exact replay returns the exact
   old receipt, including after expiry/restart. A changed payload conflicts.
2. Verify original grant ownership/binding and persist an irreversible parent
   fence with its own `fenceRevision`. Fence checks must guard every current and
   future reserve, increase and agent-attachment path. Existing cleanup and
   historical receipt reads continue after ACL/authorization expiry.
3. Enumerate all child/legacy/execution liabilities under that fence. A writer
   lock excludes a concurrent reserve; an old central reserve arriving afterward
   is refused permanently, including one whose allocation ID was never seen.
4. For today's non-executable children, mark known reserved children retired
   with durable tombstones and zero retained usage. Preserve each original reserve
   command/digest and receipt. Store any cleanup receipts/publications atomically;
   do not rewrite a prior `reserved` receipt into `released`.
5. Recompute `H`. For unsupported/executable/unknown children, keep the entire
   unresolved amount, including relevant runner/stop/usage fences. Apply only the
   proven reduction. Save balance, ledger revision, immutable result and pending
   publication in the same commit before making capacity reusable elsewhere.

A persistent parent fence rejects unseen delayed reserves without needing an
unbounded list of central command IDs. Central child cleanup still resolves each
original command through its own durable receipt; an aggregate parent proof must
not fabricate per-child executor results. Child receipts missing centrally stay
“reconciling” and cannot regain authority from an old notification.

Each separately identified native command has a distinct monotonically increasing
ledger revision, even if several logical changes share one SQLite commit. Parent
fence/receipt revisions cannot be reused for a child command receipt. Internal
child tombstones made by parent cleanup do not invent independent child-command
receipts; a later exact child cleanup command records its own revision. Supporting
batched receipts with a shared revision would need an explicit new inclusion proof.

A first `grant.close` can retain unknown liabilities. A later `grant.settle` uses
a **new** command ID and the same permanent fence, with the expected previous
settlement digest. It can release more only when the native ledger has stronger
proof. Original close replay always returns its original result. Trusted late
usage may increase `H` through a new correction receipt; it never renews authority.

## Minimum closed wire contract

Use a new capability `hagency.parent_settlement.v1`, independent of the existing
reservation capability. Both server support and the actual native adapter must
be present before exposing Close/Renew. Original grant/child DTOs stay unchanged.
All IDs/digests are bounded, numeric values are JavaScript-safe integers, receipts
fit the existing management transport budget, and hashes use the shared canonical
encoding. No provider credentials, private paths or account evidence enter chat.

```text
GrantClosureCommand {
  version: 1,
  commandId, operation: "grant.close" | "grant.settle",
  grantId, grantReceiptDigest,
  previousSettlementDigest: string | null,
  reason: "owner_withdrawal" | "administrator_withdrawal" |
          "contribution_revoked" | "settlement_progress",
  requestedAtMs
}

NativeGrantSettlementReceipt {
  version: 1, commandId, commandDigest,
  grantId, grantReceiptDigest, enrollmentId, periodKey,
  fenceRevision, ledgerRevision,
  state: "retained" | "closed",
  retainedTokens, knownConsumedTokens, unresolvedTokens,
  liabilityCount, liabilityDigest
}
```

`retainedTokens = knownConsumedTokens + unresolvedTokens`, with no double
counting. When attribution is unknown, place the entire conservative hold in
`unresolvedTokens`. `closed` requires zero unresolved liability; known spent
usage can remain retained. The amount returned is derived against the immutable
original grant; it is not supplied by the caller. `liabilityDigest` commits to a
bounded paged ledger manifest including child IDs, original binding/receipt
identities, retained amounts and unresolved fences. Private usage evidence stays
local; its digest/reference may be included in the native audit, never raw data.

Native advances `ledgerRevision` on each new close/settlement/correction, retaining
exact historical replay. Central rejects contradictory revisions/amounts while
allowing older exact receipts to acknowledge history without replacing a newer
balance. A command digest alone never proves that capacity was released.

The close operation is permanent cleanup: its delivery lease is finite, but grant
expiry does not disable cleanup. A historical-recovery response cannot construct
new execution/settlement authority. Local-first closure creates a durable
publication under owner-native consent; an authenticated central acknowledgement
removes only the exact receipt digest from its pending queue.

## Central transitions and smallest route additions

Preserve the original immutable grant receipt and existing revocation history.
Add a separate lifecycle/balance projection and durable parent command/result
records; do not overload `management_executor_commands.allocation_id` with a fake
project or nullable authority shortcut.

| State | Evidence and behavior |
| --- | --- |
| `held` | Original native grant accepted; current hold initially equals its amount. |
| `awaiting_native_close` | Original grant revoked or expired before this cleanup protocol was available; no native receipt means the original hold remains. |
| `closing` | Owner withdrawal/admin contribution revocation and exact cleanup outbox commit atomically; all new coordinator decisions/deliveries denied. Hold remains unchanged. |
| `outcome_unknown` | Cleanup may have run, but its exact receipt is missing. Keep the hold and recover the same command. |
| `retained` | Native permanent fence accepted; some known unused capacity released, unresolved liabilities still held. |
| `closed` | Native permanent fence and complete liability reconciliation accepted. Show any spent retained amount separately; not necessarily a zero balance. |
| `awaiting_reconciliation` | A child balance arrived with a newer native ledger revision than the parent proof; conservatively retain it until a new aggregate proof reconciles it. |

The implemented default-off gate extends existing `POST grants/{id}/revoke` to
enqueue one parent close atomically alongside child cleanup. Its existing grant
response remains `revoked`; the separate settlement projection is `closing` and
never claims a refund. Contribution revocation queues the same logical close,
including when it races an individual grant withdrawal.

Local owner closure would publish through proposed `POST grants/{id}/settlements` with
a publication `commandId`, expected central lifecycle revision, original
`GrantClosureCommand` and its closed native receipt. The server recomputes the
command digest and binds it to the exact grant/receipt; a local-first digest
without its original command is insufficient. This local-first publication route
is deliberately **not implemented**, because its native authority adapter does
not exist yet.

For central-initiated cleanup, the implementation provides typed host-private
`POST grant-executor-commands/poll`, `/{id}/recover`, and `/{id}/result`; retain
exact owner/installation selection and fresh finite upload tickets. Distinct
parent routes avoid changing the validated project executor envelope.

The complete implemented routes, under `/hagency/api/v1/management`, are:

| Route | Input and result |
| --- | --- |
| `GET grants/{id}/settlement` | Current `{settlement}`; owner/admin/coordinator visibility, no authority inferred from visibility. |
| `POST grants/{id}/close` | `{commandId, expectedGrantRevision}`; owner or designated server admin withdraws authority; returns `{settlement}`. |
| `POST grants/{id}/settle` | `{commandId, expectedRevision}` of the parent lifecycle; asks for a new aggregate proof after the last command has an exact receipt. |
| `POST parent-commands/{id}/recover` | `{operation: "parent.close" \| "parent.settle"}`; original actor and stable installation recover the current projection. |
| `POST grant-executor-commands/poll` | Optional `{after}` cursor; at most one exact original owner/installation delivery. |
| `POST grant-executor-commands/{id}/recover` | `{commandDigest}`; read/republication ticket with `executionAllowed=false`. |
| `POST grant-executor-commands/{id}/result` | `{commandDigest, token, receipt}`; exact command/receipt acknowledgement and current projection. |

`enable_parent_settlement` defaults to false independently of project reservations.
No parent capability is advertised in management session or mini-app UI, even
when test policy enables its host-private routes. Close and Settle are reductions
or accounting operations; coordinator status alone does not authorize them.
Assigned project administrators retain sole human agent-approval authority once
project capacity is allocated; this protocol adds no second agent approval.

Schema v5 preserves original grant receipts byte-for-byte and uses separate
parent lifecycle/command tables. A unique logical-close index and the management
transaction lock prevent duplicate parent fences. Failed command or notification
insertion rolls back revocation, child cleanup and parent enqueue together.

The native fence revision must be newer than the original grant and every known
successful child reserve. A late successful reserve receipt must precede that
fence; it cannot resurrect authority. A later child release with positive retained
liability is added conservatively to the last parent amount and marked unresolved,
until a newer native aggregate receipt accounts for it. This can temporarily
overstate a hold but cannot manufacture free capacity. New aggregate receipts
advance the ledger while preserving the permanent fence and all previously known
consumed usage. Missing or conflicting proof never lowers the last known hold.
Parent/child revision collisions are rejected in both arrival orders, including
against historical parent receipts, so equality cannot hide a positive liability.

Closed DTO definitions are in `management_parent_types.rs`; canonical cross-repo
vectors are in `tests/fixtures/management-parent-settlement-v1.json`. The native
agent independently verified both SHA-256 values against the shared canonical
encoding. The fixture is synthetic and contains no credentials.

The result transaction stores the exact native receipt, latest balance/lifecycle,
audit and reference-only notification together. Old reports acknowledge their
own command but cannot overwrite newer settlement. Server revocation fences first;
native revocation fences first locally. There is no distributed atomic success
claim between them. Offline/missing native state stays unresolved.

## Renewal is a new grant, not reactivation

Initial scope: explicit owner-native renewal only, no automatic monthly rollover.
Create a new grant ID/receipt/period after current profile/resource/provider proof
and owner consent. Retire the old authority permanently. The old remaining `H`
continues to count against the same resource and quota account; a new parent is
limited by real headroom, not by an assumption that the old month is free.

Record `replacesGrantId` and the acknowledged prior settlement digest in a separate
renewal command/publication, leaving existing closed grant DTOs immutable. Publish
closure before the successor so the central single-active-grant constraint is
respected. A lost acknowledgement recovers the original local hold/receipt;
never allocate another successor to compensate for a timeout. One semantic
successor per predecessor/target period prevents alternate command IDs doubling
capacity.

Proposed native-only `POST grants/{oldId}/renewals` takes a publication command ID,
expected contribution revision and a closed renewal receipt containing the
original owner-renewal command/digest, predecessor grant/digest, accepted prior
settlement digest and the new unchanged-shape `NativeGrantReceipt`. Native stores
that link with the new hold atomically. Central validates the original enrolled
owner/device, current admitted contribution, immutable predecessor link and
single semantic successor, then records the successor/link/publication receipt
atomically. A revoked contribution cannot be revived by renewal. In the initial
zero-execution slice, require a fully closed predecessor; broader partial-renewal
policy can follow once native retained-liability accounting is implemented.

This renewal does not carry old project reservations into a new grant/period.
The previous coordinator decision freezes the old grant and period. New-period
allocation requires an explicit new decision or a future, explicit standing
renewal policy; it cannot be inferred. Server project admission can remain valid
when its project intent is unchanged. Agent creation inside allocated capacity
continues to belong to the assigned project administrator, without an additional
Hagency owner/coordinator vote.

## Existing contribution identity reconciliation

Existing contributions contain per-profile `resourceId` and configuration hash;
new canonical owner resources can have different IDs/hashes. Existing
`resume_contribution` deliberately requires the same resource identity and only
advances execution generation. It is not an amendment endpoint.

Before proposing any amendment, trusted native code must establish:

- The original and target resources refer to the same verified provider issuer
  and stable quota-account subject. A path, `CODEX_HOME`, model name, Matrix user
  or label is not account identity. If proof is unavailable, stop the equivalence
  path and require explicit new-resource onboarding/admission.
- The authenticated original Matrix profile and stable installation still own
  the contribution. The OS owner explicitly links that profile to the canonical
  resource through its ACL; other profiles gain no access automatically.
- Runtime/model/capability/ceiling semantics are unchanged, or are presented as
  an actual resource change needing appropriate readmission. Different opaque
  IDs or hashes alone cannot establish either equality or difference.
- The original profile ledger is durably fenced against further allocations and
  has no live, queued, leased, unknown or retained legacy/provider liability for
  this resource. A read-only empty snapshot without a writer fence is insufficient.
  Do not silently import/merge a populated old ledger. Other processes/older
  clients must be unable to reopen a writable stale authority path; if that
  cannot be enforced, migration remains blocked.

The smallest central amendment path should initially accept **only a proven
representation change with no existing native parent-grant/command history for
that contribution**. Existing project intent records may remain. Propose:

```text
POST contributions/{id}/resource-amendments
{
  commandId, expectedContributionRevision,
  oldResourceId, oldResourceRevision,
  newResourceId, newResourceRevision,
  nativeReconciliationReceipt
}
```

The receipt freezes a stable migration ID, original profile/installation, old
resource snapshot, target canonical registry/resource, old/new semantic digest,
source-fence proof, zero-liability proof, explicit ACL-consent receipt and native
ledger revision. Native provider/account evidence remains private. Only the
trusted host supplies this publication; Splash cannot manufacture the proof.

Central authenticates the same owner/stable device, rechecks expected old binding
and zero grant/command history in one transaction, rejects an existing target
identity collision, records the immutable mapping, updates resource ID/hash (and
current generation when explicitly bound), advances a dedicated binding revision
and clears connection verification. Preserve admission only for the verified
semantic-equivalence case; preserve project intent because it references the same
contribution and no executable command exists. No new parent hold is created by
this amendment. Fresh native verification/enrollment follows before publishing
a grant. Unknown responses recover the original amendment; they do not repeat
migration or mint aliases. Different owners/devices are a new contribution flow.

Native source fencing and target mapping span different databases. Use a durable
forward-only migration journal and deterministic process/ledger lock order:
commit the source fence first, then record the explicit target ACL/mapping, then
publish the proof. A crash leaves the source fenced and resumes that migration
ID; it never activates a second ceiling or unfreezes the old source to retry.
Do not pretend these two SQLite databases and PostgreSQL form one transaction.

This is **not implemented** and is not safe for a resource with existing grants,
agents or unresolved legacy state. Those need a separate explicit migration that
preserves original liabilities, histories and cleanup bindings. Until then, keep
them fenced/visible in their original ledger instead of relabeling them as new
empty capacity.

## Acceptance before exposing the feature

1. Close versus reserve races across processes: exactly one serialized order;
   delayed unseen reserve is refused after the permanent fence.
2. Current non-executable parent with zero/multiple children: exact unused amount
   returns once; child receipts/tombstones remain recoverable. Alternate close,
   renewal and allocation IDs cannot recreate authority or double the refund.
3. Partial/unknown/legacy liability retains the full uncertain portion; refused
   child release, missing receipt, UTC rollover and revoked ACL do not free it.
4. Kill after native close commit but before publication; restart/rotate device
   generation; recover exact original receipt without reopening authority. A
   different installation/profile cannot recover or settle it.
5. Close before a delayed reserve, delayed reserve result after close, late child
   release, old parent acknowledgement and a new settlement correction preserve
   current balance/order. Late usage is charged even after an earlier settlement.
6. Inject SQLite receipt/outbox and PostgreSQL result/outbox failures: each local
   transaction rolls back; no cross-store success is invented.
7. Renewal with a retained old-period hold fails when account headroom is
   insufficient. Shared account aliases/profiles/runtimes cannot bypass it.
8. Real old per-profile contribution, unchanged provider and empty fenced source:
   one explicit amendment preserves intent and needs fresh verification. Test
   wrong owner/device/provider, changed semantics, duplicate mapping, concurrent
   source write, old client reopen and nonempty/unknown source fail closed.
9. Native/central DTO fixtures, bounded manifests and real host-driven close →
   recover → known-unused release → renewal work with the mini app closed.
   Clearly display “retained/awaiting settlement”; never infer free capacity from
   a closed window or dismissed notification.

Implement the native zero-execution close/receipt path and prove empty-source
identity reconciliation next. Defer executable-child partial settlement and automatic renewal
until their evidence/authority adapters exist; do not expose optimistic controls
that merely change central status.

Native implementation review confirmed this design matches the current ledger
boundaries. The zero-retained fast path requires the explicit non-execution
invariant; a permanent `active=false` fence alone never changes commitments,
and untouched legacy profile ledgers remain an activation blocker until their
writer-fenced reconciliation is complete. Current central tests use synthetic
native receipts; they do not prove a native ledger released capacity. Real native
adapter and host-driven integration tests are required before activation.

## Central validation completed

The final PostgreSQL run used an isolated disposable database in
`hagency-latest-test-postgres-1` and passed **122 tests, zero failed or ignored**
(`.run/miniapp-parent-postgres-3.log`, 98.26 seconds). Twelve parent-specific tests
cover real HTTP closed inputs/roles, missing proof, retained and overrun balances,
expired delivery/current-generation recovery on the same installation, withdrawal
races, both late-receipt arrival orders, equality against historical native ledger
revisions, enqueue/result notification rollback, and v4→v5 migration rollback
preserving original native evidence. The test runner dropped its database after
completion; application databases were not used.

`cargo clippy --locked -p hagency-agent-service -p hagency-server --all-targets
--offline -- -D warnings`, `cargo fmt --all --check`, and `git diff --check` passed.
The backend built successfully with the existing debug macOS linker warning about
the size of `__eh_frame`; no running process was replaced. Logs are
`.run/miniapp-parent-clippy-2.log` and `.run/miniapp-parent-server-build-1.log`.
Native implementation and independent source review both verified the canonical
fixture; independent source review found the revision equality edge, which is now
fixed and covered by both arrival-order regressions. This is central protocol
validation, not native capacity-release end-to-end validation.
