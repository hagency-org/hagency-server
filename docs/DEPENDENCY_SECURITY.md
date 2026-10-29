# Dependency security review

The 2026-10-30 hardening updates Rustls, WebPKI, root certificates, ctutils,
pulldown-cmark, webpage, Tokio and the embedded Pasion/OpenDAL graph. Palpo and
Pasion remain on their upstream `main` branches with exact commits in Cargo.lock.
The host OpenTelemetry version follows the embedded Pasion version.

The original lockfile had 14 vulnerability records. The updated graph has two
unresolved vulnerability records and five informational warnings. A successful
dependency-policy check **does not mean that cargo-audit is clean**.

## Unresolved upstream cryptography

| Advisory | Locked dependency | Disposition |
| --- | --- | --- |
| [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) | crypto-bigint 0.5.5 through Pasion JOSE and elliptic-curve 0.13 | The fixed 0.7.6 API is incompatible with this curve generation. A coordinated cryptography migration is required. Secret-dependent arithmetic remains an open timing risk; this review does not establish non-exploitability. |
| [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html) | rsa 0.9.10 through Pasion JOSE/keystore | RustSec has no patched release. Reviewed JOSE code signs/verifies JWTs and does not call RSA padding decryption. The advisory remains unresolved, including its broader warning about observable private-key timing. |

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
- Changes to the reviewed Pasion JOSE source commit.
- Expired, future-dated, duplicated or longer-than-30-day review records.
- Stale dispositions after a vulnerable dependency is fixed or removed.
- Malformed reports or a disposition that calls a vulnerable version resolved.

It does not use `cargo audit --ignore`. Existing unresolved findings and warnings
are printed on every run. The current review expires on **2026-11-29**. Updating
that date requires rechecking the call paths, affected operations, upstream fixes
and source revisions; it is not an automatic renewal.
Review dates explicitly use Asia/Shanghai, including on UTC GitHub runners.

The `Dependency risk policy` workflow runs on PRs, main pushes, weekly and manual
dispatch. Cargo Dependabot updates run weekly. Local offline reproduction may use
`--no-fetch` only after the advisory database has already been fetched.

Informational warnings currently cover async-std, paste, smartstring,
unsafe-libyaml and legacy rand. These remain replacement/review work; they are
not included in the two vulnerability-record count.
