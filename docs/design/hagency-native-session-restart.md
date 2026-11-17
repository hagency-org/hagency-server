# Preserve the original native device across ordinary Desktop restarts

The restart defect is in native login lifecycle, not the immutable allocation
receipt. Desktop previously sent normal application exit through the same remote
revocation path as logout. Its next start kept the installation identifier but
created a new server session and registered the device again. Registration
increments the device generation, permanently fencing commands issued under the
old generation.

`Store::renew` already accepts an expired, unrevoked native session when presented
with a fresh upstream proof for the exact issuer, subject, Matrix ID and OAuth
client. It renews the original session without changing the device. Normal
restart therefore requires no new grant table, capacity reservation or human
approval. Existing equality checks and irreversible execution fences remain.

## Native credential custody

The SDK-backed native login writes a bounded, OS-private
`server-login-sdk-resume.json` in its existing owner state directory. Its binding
includes the canonical state-directory scope, origin, original installation,
client, subject, owner and server user, and the original native session/device
credentials and generation. It never serializes SDK access or refresh tokens,
`Instant` values, local runtime capabilities, provider identity or model data.
Deserializing the record grants no authority.

The local states are:

| State | Meaning | Automatic next step |
| --- | --- | --- |
| Absent | No credential journal is present | First registration only for a genuinely fresh owner; an existing owner descriptor requires explicit new sign-in |
| Registering | The original device POST may have been issued; its usable response is not durable | Refuse another POST; retain unknown outcome |
| Active | Exact native credentials were durably saved before publication | Renew and independently verify the original device |
| Revoked | Explicit logout, removal or an authenticated rejection of the original device | No ordinary restore or implicit replacement |

Registering is written before `/devices`; Active is written before exposing local
authority. Cancellation or lost device response cannot silently repeat
registration. The host retains the original state-directory ownership lock.
Credentials copied to another state directory fail scope validation; the feature
does not import credentials from another account, machine or legacy profile.

An existing owner descriptor with no credential journal is ambiguous: it may be
an upgrade from the memory-only client or lost local credentials. It returns
`native_resume_missing` without registering. An explicit new SDK sign-in can
authorize replacement; it does not preserve a previously rotated generation.

## Fresh restoration proof

Ordinary SDK restoration first checks the live SDK source. It then renews the
original native session through the existing `/sessions/current/renew`, validates
`/identity`, and authenticates the original device bearer at the new read-only
`GET /hagency/api/v1/devices/current`.

That endpoint returns the closed shape:

```json
{
  "userId": "usr_original",
  "mxid": "@owner:example.test",
  "subject": "original-subject",
  "clientId": "original-sdk-client",
  "issuer": "https://example.test/auth/",
  "deviceId": "dev_original",
  "generation": 1,
  "validUntilMs": 0
}
```

The illustrative zero deadline above is not valid authority. Production deadlines
are fresh and finite. The client compares every identity and device field,
rechecks the SDK source, local retirement gate and unchanged journal immediately
before installing a new in-memory session. The server authenticates again before
returning. A timeout, unsupported endpoint, expired proof or unexpected response
never falls through to device registration.

An SDK-source failure during ordinary renewal invalidates only this process's
capability. Source/account mismatch can race local retirement, and the renewal
error alone does not prove revocation of the saved original account. Its private
record remains unchanged. Explicit logout still tombstones it, and authenticated
rejection of the original device after fresh same-owner restoration proof also
refuses future automatic restoration. Legacy browser-owned OAuth revocation
semantics are unchanged.

## Exit, switch and logout

Normal exit, saved-account switching and the source-to-canonical adoption
transition use native suspension: synchronously retire local capability handles,
stop and await retained runtime/provider tasks, preserve unknown cleanup, close
the store, and retain the original native credentials. Suspension does not itself
extend any server lease or authorize a stopped process to restart.

Explicit logout and account removal first durably mark the journal Revoked,
before any asynchronous drain or remote request. They then queue/retry revocation
of the exact original native session, including a credential that was never
installed in the current process. A persistence failure remains an observable
logout failure; remote revocation is still attempted. Late restore replies cannot
overwrite the tombstone.

Saved-account removal also works before constructing a NativeOwner. A native-only
synchronous operation locks the exact existing owner root, checks its marker and
account binding, and writes the tombstone plus pending original-session revoke.
It opens no SDK, console, database or network connection, and refuses a busy or
foreign root. Removing an account therefore cannot leave a usable restoration
record merely because no management page was opened in this process.

An actual new SDK sign-in is distinct from SDK restoration. The trusted Desktop
OAuth-success path supplies default-denied
`MatrixTokenSource::permits_explicit_reenrollment()`. Only a durable Revoked
record may use this permission. The client completes the old native-session
revocation, verifies the new SDK identity, rechecks the permission and replaces
the record immediately before a new device registration. An Active or uncertain
Registering record cannot be replaced through this signal. No mini-app input
supplies this permission.

## Acceptance and remaining boundary

Acceptance covers real HTTP plus private-file/native lifecycle tests: normal
restart retains the exact original credentials and generation; fresh proof is
mandatory; unavailable or unsupported proof preserves the record; wrong identity
or changed device never authorizes; lost initial registration never retries;
logout wins an in-flight restore; only explicit new sign-in can replace a revoked
record. PostgreSQL tests cover expired original-session renewal, current-device
proof, rotation, revocation, wrong bearer and unchanged original session/device
identity.

This correction preserves existing resource/grant/reservation identifiers and
accounting. It does not revive a generation already rotated by an earlier build,
remove a permanent fence, change an approved scope or replay an issued command.
Current shared execution still needs a separately versioned successor-process
protocol for an agent whose original process stopped. Unknown model work remains
retained; session restoration alone cannot make that process runnable.
