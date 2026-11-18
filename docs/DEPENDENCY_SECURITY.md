# Dependency security

The security branch removes the two previously recorded cryptographic vulnerabilities and all five unmaintained-dependency warnings from the complete locked graph. `security/dependency-policy.json` checks the complete Cargo audit report and every Git package against its reviewed source.

On 2026-11-18 the unfiltered audit reported zero vulnerabilities and two newly published maintenance warnings, for chrono 0.4.45 (RUSTSEC-2026-0543) and chrono-tz 0.10.4 (RUSTSEC-2026-0555). The existing reviews of those exact package versions and advisory fingerprints expire on 2026-11-29. Their original reasons and review records are in the policy file.

| Previous finding | Remediation |
| --- | --- |
| [RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), rsa 0.9.10 | Pasion JOSE and keystore callers migrate to rsa 0.10.0 and the corresponding signature, digest and key-format APIs. |
| [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html), crypto-bigint 0.5.5 | EC 0.14 / ECDSA 0.17 use crypto-bigint 0.7.11, above patched minimum 0.7.6. |
| pin-utils | Published wasmtimer 0.4.5 uses maintained pin-project-lite. |
| async-std | A lettre 0.11 maintenance backport fixes ownership of an optional runtime feature while retaining Tokio SMTP and sendmail. |
| paste | Published EXR 1.74.3 removes Pulp; a rav1e 0.8 maintenance backport uses maintained pastey. EXR and AVIF encoding remain enabled. |
| smartstring | The Prometheus exporter uses maintained CompactString for temporary label buffers, preserving escaped exposition output. |
| unsafe-libyaml | Configuration parsing, export and the Figment provider use serde-saphyr; generated typed YAML is exported and reloaded in a regression. |

The Pasion changes are reviewed in [Pasion #169](https://github.com/meldry-com/pasion/pull/169). The exporter change is [upstream #15](https://github.com/sandhose/opentelemetry-prometheus-text-exporter/pull/15). The lettre backport preserves the original change from [#1145](https://github.com/lettre/lettre/pull/1145), with a separate [maintenance review](https://github.com/chrislearn/lettre/pull/1). The compatible rav1e backport is [upstream #3481](https://github.com/xiph/rav1e/pull/3481). Temporary Git sources use exact tested revisions. Replace them with compatible maintained releases when available, then review and update source pins. The embedded server uses Salvo 1.2.1 with Palpo `36d5ab61` and Pasion `22d3109c`. Both pins include the upstream fixes. The backend tests cover encoded Matrix identifiers.

Pasion preserves all 14 existing JWT algorithms, old strong-key fixtures and previously issued tokens. OpenSSL independently verifies old and new JWTs and rejects tampered signatures. No configured private key is rewritten, deleted or rotated. RSA keys below 1024 bits now produce an explicit import error, following the new library minimum; production generation remains 2048 bits. RFC 7518 requires at least 2048 bits for RS/PS JWT use, so operators of weaker existing keys must review them explicitly. See the upstream [migration notes](https://github.com/meldry-com/pasion/blob/badd27d5cec1e9f4dbc0bc2eaa74dce480f83844/docs/cryptography-migration.md) for key-format, RNG, unbalanced-prime, multiprime and P-521 compatibility details.

## Reproduce the gate

Use the repository toolchain and cargo-audit 0.22.2:

```sh
cargo install --locked cargo-audit --version 0.22.2
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/audit-dependencies.py
```

The script saves the full report to `.run/dependency-audit.json`. Every vulnerability fails the gate. Each informational warning must match a current maintenance disposition, including package version and advisory fingerprint. It rejects malformed or inconsistent reports, nonzero auditor exits, filtered targets or severities, advisory ignores, missing warning categories, future review dates and changes to reviewed Git sources or new unreviewed Git packages. It never passes `--ignore` to the auditor. An intentional source update requires a fresh audit and compatibility review before updating `reviewed_sources`.

`--no-fetch` uses a previously fetched advisory database; the default refreshes it. CI stores the full report as an artifact. A clean dependency audit does not establish absence of application bugs or prove side-channel resistance; cryptography, protocol, PostgreSQL and image regressions are separate checks.

The real EXR-to-PNG regression also exposed an existing floating-point thumbnail failure. Palpo normalizes only float RGB/RGBA before sampling, preserving transparency and the existing integer 16-bit paths. Real EXR, nonfinite-channel, PNG color-depth and AVIF regressions cover the boundary. The Agent encrypted-ingress fixture now uses the actual host ingress clock; production future-timestamp rejection remains unchanged.

Every Git package is recorded individually, including packages sharing a repository. Keeping one package at a reviewed revision cannot authorize a different package at another revision. All 23 Git packages currently resolve to five reviewed repository sources.
