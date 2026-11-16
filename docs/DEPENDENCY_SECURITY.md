# Dependency security review

The 2026-10-30 hardening updates Rustls, WebPKI, root certificates, ctutils,
pulldown-cmark, webpage, Tokio and the embedded Pasion/OpenDAL graph. Palpo and
Pasion use reviewed upstream commits explicitly pinned in the manifest and Cargo.lock.
The host OpenTelemetry version follows the embedded Pasion version.

The original lockfile had 14 vulnerability records. The updated graph has two
unresolved vulnerability records and five informational warnings. A successful
dependency-policy check **does not mean that cargo-audit is clean**.

## Unresolved upstream cryptography

| Advisory | Locked dependency | Disposition |
| --- | --- | --- |
| [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) | crypto-bigint 0.5.5 through Pasion JOSE and elliptic-curve 0.13 | The fixed 0.7.6 API is incompatible with this curve generation. A coordinated cryptography migration is required. Secret-dependent arithmetic remains an open timing risk; this review does not establish non-exploitability. |
| [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | rsa 0.9.10 through Pasion JOSE/keystore | RustSec now lists patched versions >=0.10.0 (advisory revised 2026-11-08). The locked 0.9.10 is still vulnerable; its upgrade requires a coordinated RSA/hash/key-format trait migration and JOSE/keystore round-trip validation. Reviewed JOSE code signs/verifies JWTs and does not call RSA padding decryption. This is not a non-applicability claim; broader private-key timing risk remains unresolved. |

These are retained existing risks, not newly fixed vulnerabilities. They must
remain visible during deployment risk assessment. No persistent signing keys
are deleted or silently rotated to make an audit pass. Updating crypto-bigint
as an additional direct dependency would not replace the vulnerable transitive
version and is not a fix.

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

Informational warnings currently cover async-std, paste, pin-utils, smartstring,
and unsafe-libyaml. These remain replacement/review work; they are
not included in the two vulnerability-record count.

On 2026-11-04, upstream main advanced to `37d6e30`. Pasion's JOSE,
keystore, registration handler and browser source trees remain identical to
`83e52e5`. The Server graph updates the process/Tokio OpenTelemetry
instrumentation to 0.3.0 and drops its unused OpenTelemetry 0.32 generation.
The reviewed source pin follows the new revision; existing risk dispositions
and their expiration remain unchanged.

On 2026-11-16, the repair review moved the Pasion source pin to
`716abc5e684be44e7b2883c38f328fdae500f769`. Its JOSE and keystore source trees
are identical to the prior `37d6e30` review. The current graph still has two
vulnerability records and five informational warnings; legacy rand 0.7.3 has
left the graph and its stale disposition was removed. Fresh RustSec database
commit `02e2517f457a359fc6d3440ea4560c8ea188fee8` confirms the RSA patched range.
The review date and advisory fingerprints were updated explicitly; the
expiration remains **2026-11-29**. No vulnerable package was relabeled fixed.
