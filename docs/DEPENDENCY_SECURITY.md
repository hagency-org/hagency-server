# Dependency security review

The 2026-10-30 hardening updates Rustls, WebPKI, root certificates, ctutils,
pulldown-cmark, webpage, Tokio and the embedded Pasion/OpenDAL graph. Palpo and
Pasion use reviewed upstream commits explicitly pinned in the manifest and Cargo.lock.
The host OpenTelemetry version follows the embedded Pasion version.

The 2026-11-18 integration pins Palpo at `36d5ab61` and Pasion at
`22d3109c` to use Salvo 1.2.1 throughout the embedded server. Pasion includes
the upstream JOSE and keystore migration. Cargo.lock now contains rsa 0.10.0
and crypto-bigint 0.7.11, and the previous RSA and crypto-bigint advisories
no longer appear in the full lockfile audit. That revision also removed
async-std, unsafe-libyaml and smartstring.

The 2026-11-20 update pins Palpo 0.6.0 at `3e036e82` and Pasion 1.8.0 at
`591aa3b5`. Pasion's reviewed JOSE change uses Jiff for claim time checks and
retains integer epoch seconds on the wire. Pasion now uses the crates.io Lettre
release. Its optional async-std dependency appears in Cargo.lock, while the
server uses Tokio for email.

The updated audit reports zero vulnerability records and five maintenance warnings
covering paste, pin-utils, chrono, chrono-tz and async-std. The review still
expires on 2026-11-29. The original review started with 14
vulnerability records.

The subsequent [Palpo #700](https://github.com/palpo-im/palpo/pull/700) update
pins the embedded server and core to `3d44c435`. Matrix token introspection
now surfaces issuer outages as gateway or service errors. An inactive or expired
token still returns HTTP 401.

The subsequent [Palpo #701](https://github.com/palpo-im/palpo/pull/701) and
[Palpo #703](https://github.com/palpo-im/palpo/pull/703) update pins the embedded
server and core to `51e48a89`. Sliding sync now sends an accepted invitation as a
new joined room, and the targets of the membership events in a room's timeline as
lazily loaded members. No dependency of the embedded server changed.

The subsequent [Palpo #704](https://github.com/palpo-im/palpo/pull/704) and
[Palpo #705](https://github.com/palpo-im/palpo/pull/705) update pins the embedded
server and core to `216c13c3`. Sliding sync now sends a room by the user's
membership of it: a room subscription no longer serves a room to a user who has
no part in it, and a user who has left or knocks gets no room state. The summary
of a thread is now put together for the user who reads the thread's first event,
and holds the latest reply only for a user who may see that reply. No dependency
of the embedded server changed.

The subsequent [Palpo #706](https://github.com/palpo-im/palpo/pull/706) update pins
the embedded server and core to `50511ee0`. It adds the server option
`invite_state_member_limit`: with a value above zero, an invitation of one of this
server's users also names the room's joined and invited members, up to that
number. With the default of zero nothing changes. Knocks and invitations of users
on other servers are not changed. No dependency of the embedded server changed.

## Automated review policy

`python3 scripts/audit-dependencies.py` runs cargo-audit against a fresh RustSec
database, preserves its complete JSON result, and compares every vulnerability
and informational warning with `security/dependency-policy.json`. It rejects:

- New advisory IDs, finding kinds, packages or affected versions.
- Changed advisory content, affected operations or patched/unaffected ranges,
  even when the advisory ID and locked version are unchanged. Version 2 policy
  records pin a SHA-256 of the reviewed advisory, version ranges and affected
  metadata; updating that fingerprint requires re-review, not automatic renewal.
- Changes to the reviewed Pasion JOSE source commit.
- Expired, future-dated, duplicated or longer-than-30-day review records.
- Stale dispositions after a vulnerable dependency is fixed or removed.
- Malformed reports or a disposition that calls a vulnerable version resolved.

It does not use `cargo audit --ignore`. Existing unresolved findings and warnings
are printed on every run. The current review expires on **2026-11-29**. Updating
that date requires rechecking the call paths, affected operations, upstream fixes
and source revisions; it is not an automatic renewal.
Review dates explicitly use Asia/Shanghai, including on UTC GitHub runners.

On 2026-11-03, the password registration continuation fix moved the locked
Pasion revision from `881ae7a` to `7967064`. The Pasion JOSE and keystore trees
and the resolved dependency graph are unchanged; the only upstream source
changes are the registration handler and browser routing. The reviewed source
pin follows that revision. Findings, dispositions, and the original expiration
are unchanged.

The subsequent update to Pasion main `83e52e5` keeps the same JOSE and
keystore source trees. The Server lockfile updates `cron` to 0.17.0 and
already contains `hyper-util` 0.1.21. The reviewed source pin follows the new
revision without changing dispositions or the expiration.

The `Dependency risk policy` workflow runs on PRs, main pushes, weekly and manual
dispatch. Cargo Dependabot updates run weekly. Local offline reproduction may use
`--no-fetch` only after the advisory database has already been fetched.

On 2026-11-04, upstream main advanced to `37d6e30`. Pasion's JOSE,
keystore, registration handler and browser source trees remain identical to
`83e52e5`. The Server graph updates the process/Tokio OpenTelemetry
instrumentation to 0.3.0 and drops its unused OpenTelemetry 0.32 generation.
The reviewed source pin follows the new revision; existing risk dispositions
and their expiration remain unchanged.

On 2026-11-16, the repair review moved the Pasion source pin to
`716abc5e684be44e7b2883c38f328fdae500f769`. Its JOSE and keystore source trees
are identical to the prior `37d6e30` review. That graph still had two
vulnerability records and five informational warnings; legacy rand 0.7.3 has
left the graph and its stale disposition was removed. Fresh RustSec database
commit `02e2517f457a359fc6d3440ea4560c8ea188fee8` confirms the RSA patched range.
The review date and advisory fingerprints were updated explicitly; the
expiration remains **2026-11-29**. No vulnerable package was relabeled fixed.
