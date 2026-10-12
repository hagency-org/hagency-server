# Executed validation

[中文](VALIDATION.zh-CN.md) · [Documentation](README.md)

The old Fleet/Engagement/allocation results below describe removed implementations.
They do not establish compatibility in the current Agent release. Current behavior
is documented in [Agent architecture](OPERATIONS.md) and the configuration guide.

2026-10-07 cutover checks executed for the new browser/domain boundary:

- `cargo test -p hagency-server --offline --lib browser_auth::tests`: two tests passed.
  Controlled HTTP checks cover live identity/admin-role revocation, cookie/CSRF/origin,
  logout, closed BFF capabilities and old routes unmounted from BrowserAuth.
- `cargo clippy -p hagency-server --offline --all-targets -- -D warnings`: passed
  after removing the obsolete Fleet contract-server example.
- `cargo check -p hagency-frontend --target wasm32-unknown-unknown --offline`: passed;
  51 existing generic frontend dead-code warnings remain.

These checks do not prove browser visual acceptance or complete-host PKCE/Matrix
transport. Record the separately executed integrated smoke results before claiming
those paths were verified. The PostgreSQL runner's results are recorded separately.


Additional 2026-10-07 integration evidence:

- 39 tests passed in the isolated PostgreSQL runner, including private Room discovery/roster authorization and the production Appservice startup readiness gate. The authenticated create/bind HTTP routes return 503 while readiness is false with unchanged Agent/binding/command/audit rows and zero Matrix/Gateway calls. The real startup worker remains unready after Matrix send, becomes ready only after the authenticated AS event is durably received, and the same routes then return 202. Added checks cover 129 execution witnesses across two pages, owner isolation, permanent history, mandatory HTTP history snapshots and atomic concurrent start/acquire rejection without lease changes. The test uses random isolated databases and removes them after execution. Final strict all-target Clippy also passed; the server stores execution evidence rather than model charges.
- `HAGENCY_TEST_BACKUP_RESTORE=1 python3 scripts/test-agent-integration.py` passed: real native Pasion PKCE, puppet membership, messages and stable replies, Room discovery, retirement and [same-host three-database recovery](BACKUP_RESTORE_VALIDATION.md).
- `HAGENCY_TEST_HOLD_CLIENT=1` with the real client OwnerHost test passed: login, Space/Room creation and linking/adoption, Room discovery, Agent creation and actual membership, roster and logout. No model inference; temporary databases cleaned up.
- OpenAPI 3.1 owner/device and Appservice contracts cover 46/4 actual operations; route/DTO validation and eight negative checks passed, including the execution-history route and required lease-acquisition snapshot.

This is a chronological record of checks performed on earlier implementations.
For current behavior, read the final unified Pasion section and the
[configuration guide](guide.md); earlier opt-in authentication and frontend notes
are superseded by later sections. This documentation edit does not rerun those
checks.

Validated locally on 2026-10-03 with Rust 1.98.1, PostgreSQL 18.6 and Node 26.9.
The final Cargo dependency is the public Palpo Git revision
`c8568d9844a6be0a3172d98d7c9810e3a1f7521c` ([PR #505](https://github.com/palpo-im/palpo/pull/505)).

## Passed

- Palpo library/binary compilation and library suite: 244 passed, 17 opt-in
  database regressions ignored. Includes a mounted-router ownership regression.
- `cargo fmt --check`, locked all-target compilation and
  `cargo clippy --all-targets --locked --no-deps -- -D warnings`.
- `cargo test --locked`: atomic rollback and two unified-config/signing-key tests.
- Opt-in PostgreSQL test: persistent delivery state, reopen/restart, single writer
  advisory lock and rejection of a second writer.
- Ten HTTP contract groups against a controlled Matrix fixture: exact frontend
  bytes; session/Host/Origin/CSRF/live authorization; owner credential isolation;
  Fleet install/idempotency/lifecycle; managed identity lifecycle; private project
  approval rooms and request source bindings; outbound lease/ACK/proof/rotation;
  resource admission and verified final-allocation retirement; ordinary account
  provisioning through private authenticated approval.
- Real integrated binary against an empty isolated PostgreSQL database:
  Matrix administrator bootstrap/login, homepage, Matrix versions and discovery,
  real App Service registration/representative creation, real event transaction
  delivery to the mounted relay, ACK and exact probe proof, graceful restart,
  retained users/Fleet/credentials/signing key and persisted delivery history.
- Original frontend HTML/JS/CSS compared byte-for-byte with Palpo's checkout.
- Development config `--check-config`, Python/Node/shell syntax and Compose
  configuration validation.

## Docker verification

The first fully optimized build exceeded the Docker VM's 8 GB memory budget.
The build now uses one compiler job and opt-level 1 for the large Palpo crate;
its dependencies retain full optimization.

- `docker build -t hagency-server:integration-check .` passed on Linux arm64.
  Final release build completed within the 8 GB VM memory budget.
- `node tests/docker-smoke.mjs` passed against that image: isolated PostgreSQL
  and Rust server, PID 1 runs as UID 10001, runtime configuration mode 0600,
  unchanged homepage, mounted Matrix APIs, actual administrator password login,
  graceful restart, retained account and identical persisted signing key.
- Test Compose project containers and named volumes were removed by the harness.

Image manifest: `sha256:acc80a21ba2a2b5772110463ec4faf1e4112609cd09f5c31a6e0d1c600d573d5`.
Image size reported by Docker: 359,121,160 bytes.

## Scope and limits

The contract fixture is intentionally controlled; it proves HTTP behavior and
state transitions, not interaction with a live Codex/Claude model runtime. The
real integration proves actual Palpo event delivery and restart recovery. It does
not exercise Matrix federation, every Matrix API, or the complete optional
account flow against a real approval bot. No existing development stack or user
database was stopped, reconfigured or migrated. Test containers/volumes are
isolated and removed after verification.

The adapter retains the original single-writer model. Browser sessions are
memory-only. Existing SQLite admin state is not automatically migrated. Palpo's
embedding API remains process-global, with one MatrixServer per runtime/process.

## Upstream CI baseline

Palpo's current main revision already fails its strict workspace Clippy job with
17 redundant-field diagnostics from existing Diesel-derived data structs:
[main workflow](https://github.com/palpo-im/palpo/actions/runs/37019841267).
PR #505 has the same diagnostics, outside this change. This is separate from the
passed downstream all-target Clippy check above. The library extraction also
exposed two stale documentation imports; these are corrected in a documentation
follow-up on the Palpo PR (`cargo test -p palpo --doc --locked`: 2 passed).
The server remains pinned to the runtime revision
used in all native and Docker integration tests, rather than an untested update.

## Embedded Pasion (2026-10-04)

Pasion is pinned to `b8333e1ee60c6362847d0f1890461b7af7547037`
([PR #102](https://github.com/meldry-com/pasion/pull/102)). It uses the same
Tokio/Salvo process and listener at `/_pasion/`, with its own database on the
shared PostgreSQL service. No existing development deployment was changed.

Executed checks:

- Pasion mounted-state isolation, SPA prefix, API schema base paths and legacy
  account redirects; loopback homeserver proxy bypass; template prefix tests.
- Standalone Pasion CLI/backend compilation and CI-pinned formatter.
- Dioxus 0.7.5 release WASM assets built with `--base-path /_pasion/`.
- Browser login and Security Center navigation; prefixed JS/WASM/CSS, no browser
  errors.
- Host configuration regression rejects matching database names even through
  differently spelled hosts, before writing keys or creating data directories.
- Original web-admin ten-group HTTP contract and actual Palpo/Fleet integration
  still pass with Pasion dependencies linked.

- Final pinned revision: `cargo build --locked` and `cargo test --all-targets
  --locked` passed (one atomic-state test and three configuration tests; one
  opt-in database test remains ignored in this invocation).
- Final native integration passed: same listener, prefixed discovery, SPA, JS,
  WASM and API schema; legacy account redirect; registration/browser login;
  background Palpo provisioning; delegated Matrix password login/introspection;
  identical signing/encryption keys across restart and graceful shutdown.

- Final host `cargo fmt --check` and strict all-target Clippy passed.

- Final Linux arm64 Docker build passed within the 8 GB VM budget. Backend
  release build took 8m50s with warmed dependency caches; Dioxus release assets
  also built inside their separate build stage.
- `HAGENCY_TEST_IMAGE=hagency-server:pasion-integration-check node
  tests/docker-smoke.mjs` passed: unchanged homepage, actual administrator login,
  same-port Matrix/Pasion discovery and account UI, UID 10001, mode-0600 runtime
  config and OAuth keys, graceful restart, retained administrator, unchanged
  Matrix signing key and OAuth JWKS. The harness removed its containers/volumes.

Pasion-integrated image manifest:
`sha256:a542c5c46cc2774fa3dedad5e1c382ac79ec6fefaebafc2cdbe787e8a4849496`.
Docker-reported size: 439,346,725 bytes. Native test PostgreSQL containers and
their temporary data were also removed.

The authentication integration verifies actual account registration, background
Palpo provisioning, delegated Matrix password login and token introspection.
It does not prove every OIDC authorization-code/PKCE or external-provider flow.
Delegated Matrix authentication is opt-in; existing Palpo passwords are not
automatically migrated into Pasion.

The upstream PR also includes a test-only discovery-cache isolation follow-up.
Coverage runs independent AppStates in one process and previously reused the
first issuer. The complete backend suite after that fix passed with an isolated
PostgreSQL database: 191 passed, 1 ignored. The host remains pinned to the
production revision validated above; this test-only follow-up retains production
caching behavior.

## Three independent databases (2026-10-04)

The earlier sections record the two-database baseline. The current configuration
requires an independent top-level `database_url` for Hagency and uses
`matrix.db.url` for Palpo and `pasion.database_url` for Pasion. Defaults are
`hagency`, `palpo`, `pasion` on one PostgreSQL service. Fresh Compose volumes
create all three; existing volumes/configurations require the documented split
procedure and are not altered by this change.

Executed checks:

- Locked native binary/example compilation, all-target tests, formatter and
  strict all-target Clippy passed. Pairwise database collisions (including
  different host spellings), missing Hagency URL, and invalid PostgreSQL URLs
  are rejected before initialization.
- Generated development and Compose configuration validated with the actual
  binary: three connection URLs and mode 0600, in isolated temporary directories.
- Ten-group unchanged HTTP contract passed.
- Actual Palpo/Fleet integration passed on separate Hagency and Palpo databases:
  the admin DB contains only `hagency_admin_state`; the Matrix DB contains its
  homeserver tables and no admin table. Actual event delivery/ACK/proof and
  retained users/Fleet/transport state after restart passed.
- Actual Pasion integration passed on three independent databases: registration,
  background Palpo provisioning, delegated Matrix login/introspection, prefixed
  frontend/API/discovery, stable keys and graceful restart/shutdown.
- Linux arm64 Docker image built and isolated Compose smoke test passed: three
  table sets in their respective databases, non-root UID, mode-0600 config/keys,
  unchanged homepage, native admin login, Matrix/Pasion endpoints, retained
  account and stable signing keys after restart.

Image manifest:
`sha256:78915ebed3329376292f5c5a7b16095bf364a66243aacfa3bca8a9932575b7bc`.
Docker-reported size: 439,336,859 bytes. All database/integration deployments use
isolated containers and temporary configurations; existing development stacks
and private configuration files were not changed.

## Component-owned configuration files (2026-10-04)

Current profiles are `config/dev/` and `config/docker/`, with separate
`hagency.toml`, `palpo.toml` and `pasion.toml`. Templates live in
`config/examples/`. Hagency references the two component files, Palpo keeps its
native ServerConfig sections, and Pasion keeps native sections plus `[hagency]`
embedding options. Database names and runtime ownership are unchanged.

Executed checks:

- Locked native binary/example compilation and all-target tests passed, including
  four configuration regressions: persistent keys, schema/host checks, three DB
  collision/missing URL checks, and paths resolved against independently located
  component files (media, registration token and password secret files).
- Strict all-target Clippy and formatter passed.
- Development and production generator checked in isolated directories: three
  valid files, mode 0600, directory mode 0700, explicit origin/Matrix identity,
  native file references, and refusal to overwrite existing output.
- `--list-config-files` returns all component references without database access;
  the development watcher includes referenced files outside the main directory.
- Ten-group HTTP contract passed with native Palpo configuration.
- Actual Palpo/Fleet delivery/ACK/proof and restart integration passed using
  separate host/Palpo config files and databases.
- Actual Pasion registration, Palpo provisioning, delegated Matrix login/token
  introspection, prefixed UI/API/discovery and stable keys across restart passed
  using three config files; native Pasion database pool settings are retained.
- New local development files were generated and checked without starting any
  application or changing existing databases. The previous private root-level
  configuration files were preserved.

- Final Linux arm64 image built successfully. The isolated Compose smoke test
  passed with the mounted component configuration directory: all three native
  files loaded, runtime directory mode 0700 and all three files mode 0600 under
  UID 10001, unchanged frontend, actual administrator login, Matrix/Pasion
  endpoints, separate database table sets, graceful restart and identical
  Matrix/OAuth signing keys. The test harness removed its containers/volumes.

Image manifest:
`sha256:c6efb58bc46828c5464a4a87b1442a3dd9014618b05f36c6e611508c17ad5ea7`.
Docker-reported size: 439,287,638 bytes. Native test PostgreSQL was also removed;
existing application stacks/databases were not restarted or migrated.

## Just commands and Rust development tools (2026-10-04)

The three Python tools have been replaced by an independent Rust `xtask` and a
root `justfile`. Configuration paths/schema, the server binary and frontend
behavior are unchanged. Docker asset preparation also uses the Rust tool and
does not install Python.

Executed checks:

- `just check-tools`: formatter, strict all-target Clippy and four tests passed.
  The tests cover private profile permissions, password URI escaping, separate
  database names, preservation of existing credentials/configuration, local
  resource copying, and checksum verification before installing Dioxus.
- The watcher was exercised with isolated compiler/server fixtures: compilation
  and configuration failures preserve the running process; recovery, external
  Palpo/Pasion config edits and local Palpo source edits trigger replacement;
  SIGTERM gracefully stops the child. Custom Cargo target directories and paths
  containing spaces are supported.
- Actual just recipes generated a temporary three-file development profile and
  validated it with the real server's `--check-config`; local Pasion resources
  were copied through `just prepare-pasion --skip-frontend`. Arguments containing
  spaces passed intact. Existing private configuration files were preserved.
- Just/server formatting, diff whitespace and Compose configuration checks passed.
- Linux arm64 image built successfully using the Rust resource preparation tool,
  including a real Dioxus WASM build. The isolated Compose smoke test passed:
  three component configurations and separate databases, non-root process,
  same-port web-admin/Matrix/Pasion, administrator login, stable keys and restart.
  The test removed its containers/volumes; existing services were not restarted.

Image manifest:
`sha256:35c0a571c3f33607d2ef6187aa35a67a420e50fdd9f57ea2246c0c324bae6b6d`.
Docker-reported size: 439,287,638 bytes. Python and the Rust development helper
are absent from the final runtime image.

## Backend/frontend workspace and integrated Padmin (2026-10-04)

This section supersedes earlier references to unchanged embedded HTML/JS assets.
The host is now the `crates/backend` package in a virtual Cargo workspace;
`crates/frontend` is copied Padmin source with native Dioxus Hagency pages.
Padmin provenance is recorded in NOTICE and its original AGPL license is retained.

Executed checks:

- Backend formatting, strict all-target Clippy, six unit/config tests passed.
  The dedicated PostgreSQL single-writer test remains opt-in; real restart and
  database isolation are covered by the integration scripts below.
- Frontend Dioxus/WASM release build passed. Ten native frontend tests passed,
  including member/admin OAuth scope separation, Matrix IDs containing ports
  or IPv6 servers, and published resource role aggregation.
- `just check-tools` passed: formatting, strict Clippy, CLI verification,
  private configuration generation, local Pasion assets, watcher recovery and
  external component configuration watching. Concurrent frontend builds are
  serialized so Dioxus bundling cannot overwrite another build's working files.
- Eleven controlled HTTP contract groups passed: SPA paths and WASM MIME/magic,
  runtime configuration, reserved route isolation, traversal rejection, the
  token-to-cookie bridge, live administrator checks, Origin/CSRF, owner isolation,
  callback and outbound pairing, project/request workflows, account approvals,
  retirement, durable delivery and credential revocation.
- Real Palpo integration passed against separate empty Hagency/Palpo databases:
  administrator login, App Service installation, actual relay/proof delivery,
  persisted queue and signing keys across graceful restart.
- Real Pasion integration passed against three separate empty databases:
  mounted discovery/SPA/API, registration/login, delegated Matrix login and
  token introspection, reserved frontend client/runtime settings, member token
  bridge, stable keys and graceful restart. Temporary test databases were removed.
- Browser verification used separate sessions for the running native test server
  and an isolated OAuth server. Native administrator login, Dashboard, Users,
  Rooms, Hagency connections, projects, requests and approvals rendered without
  uncaught JavaScript errors. A host restart successfully rebound the existing
  in-memory Matrix token to a new Hagency cookie. The corrected user list keeps
  `@admin:localhost:8088` intact.
- OAuth member login reached Projects with only service navigation; `/api/fleets`
  returned 403. An account granted administrator status in both isolated Pasion
  and Palpo databases completed the administrator OAuth flow and opened Padmin's
  Pasion account list. These test promotions did not affect development accounts.

Current limits: Padmin's optional `palpo_admin` sidecar operation screens are
retained as source, but their menu is disabled because the sidecar is not embedded.
Pasion and Matrix administrator roles remain separate. Full page reloads require
sign-in because bearer/refresh tokens are held in memory; SPA navigation does not.

The final Docker image also passed `tests/docker-smoke.mjs` in its own Compose
project: three independent databases, native Matrix administrator login,
Dioxus index/SPA routes and linked JS/WASM served by the Rust process, runtime
configuration and mounted Matrix/Pasion APIs, UID 10001, mode-0600 config/key
files, graceful restart and persistent Matrix/Pasion signing keys. Its temporary
containers and volumes were removed. The local development server remains at
`http://127.0.0.1:8088/`, with Pasion account UI at `/_pasion/`.

## Unified Pasion authority and web-admin workflow completion (2026-10-04)

This section supersedes the previous separate-role/default-native-auth notes.
Generated profiles now enable Pasion delegation. Pasion owns human passwords,
registration and administrator roles; its provisioning queue synchronizes Matrix
identities and roles. The console exposes one Pasion sign-in entry. The workflow
comparison and intentional legacy approval boundary are recorded in
[the web-admin parity record](WEB_ADMIN_PARITY.md).

Executed checks:

- Backend formatting and strict all-target Clippy passed; seven unit/config
  tests passed. The optional dedicated PostgreSQL lock test remains opt-in.
- Twelve native frontend tests and the release Dioxus/WASM build passed,
  including identity-bound administrator authorization and the regression where
  an active but unusable request must not appear in Ready to use.
- All eleven controlled HTTP contract groups passed, retaining callback/outbound
  pairing, grants, proofs, durable delivery, project/request and compatibility
  account approval behavior.
- Real Pasion integration used three fresh isolated databases. It exercised
  first administrator bootstrap, actual Authorization Code + PKCE and consent,
  member-only scope denial even for an administrator, human creation/password
  bypass rejection, Matrix profile updates, Pasion grant/revoke synchronization,
  immediate denial through both Matrix APIs and an existing Hagency cookie, and
  stable signing keys after restart. Tests also confirmed that authorization
  handover revokes the preliminary grant while preserving the replacement's
  Matrix device and access token. Temporary databases were removed afterwards.
- Separate fresh browser sessions completed administrator and member sign-in.
  The administrator needed no second password prompt during scope elevation,
  opened Dashboard, Hagency connections and Pasion Accounts, and saw callback
  transport disabled when no origins are allowed. The member reached Projects
  with service-only navigation. Both sessions had no uncaught JavaScript errors.
- The final Linux arm64 image passed isolated Compose smoke verification:
  three component databases, Pasion administrator bootstrap and real OAuth
  login, non-root process, protected configs/keys, hosted JS/WASM and SPA paths,
  discovery, Matrix APIs, restart and persistent Matrix/Pasion keys. Health
  readiness precedes asynchronous user provisioning, so this test waits for the
  administrator's Matrix record before exercising authorization. Its temporary
  containers and volumes were removed.
- `just check-tools` passed formatting, strict Clippy and all five tests.

The local development databases were privately backed up before switching auth.
The existing `@admin:localhost:8088` identity was explicitly linked to the first
Pasion administrator, retaining its Matrix ID and the existing password file;
its Pasion login passed. The existing Pasion member account was retained. Private
configuration, backups and credentials remain ignored by Git. Normal startup
uses `just dev`, without bootstrap flags.

The legacy pre-registration Matrix-room approval policy is retained only for
explicit native-auth compatibility. Unified deployments use Pasion registration
and account management; that optional legacy policy has not been reimplemented
inside Pasion. Full-page reloads still require sign-in because frontend bearer
and refresh tokens remain in memory.

## Bilingual documentation, license metadata and Rust 1.99 (2026-10-04)

README now contains quick setup only, in English and Chinese. The detailed guide,
workflow comparison and historical validation records live in `docs`, with both
languages and working navigation. English/Chinese shell and TOML examples match;
local Markdown paths, heading anchors and code fences were checked.

All three project packages declare Apache-2.0 and Rust 1.99; `LICENSE` contains
the sole project license text. NOTICE retains the original Palpo/Padmin/Pasion
provenance and licenses. This metadata change does not relicense upstream AGPL
code. Earlier statements about locally included upstream license files describe
the historical state, before this edit.

Executed checks with Rust 1.99.0:

- Locked workspace/all-target `cargo check`, frontend WASM `cargo check` and
  workspace formatting passed. Existing frontend dead-code warnings remain.
- `just check-tools` passed formatting, strict Clippy and all five tests.
- Cargo metadata confirmed Apache-2.0 and Rust 1.99 for backend, frontend and xtask.
- The Rust 1.99 bookworm/trixie Docker manifest tags were verified available;
  the full Docker image was not rebuilt for this metadata/documentation change.
- Existing local server health remained successful; databases and private
  configuration were not changed by this documentation/toolchain update.

## Node.js dependency clarification (2026-10-04)

Both `just prepare-frontend` and `just prepare-pasion` completed their real
release/WASM bundling with Node.js/npm/npx commands disabled; no Node tool was
invoked. README prerequisites and both detailed guides were corrected, and the
unused `nodejs` installation was removed from the Docker web-tools stage.
Node.js is needed only for `tests/*.mjs`, not for frontend builds or server
runtime. The full Docker image was not rebuilt for this dependency removal.

## Hagency contract and Operations migration (2026-10-05)

Migrated the implemented contract and Operations workflow from Palpo draft #508
at `985c7242c2074b7cb0561c14c7c79dc6ed1a2bf7` into local workspace crates.
Operations shares the existing Hagency PostgreSQL state and writer with the
legacy fleet API. No fourth operational database or SQLite service was added.
The ownership, compatibility and remaining integration work are documented in
[Operations](OPERATIONS.md).

Executed checks:

- Contract/Operations: 40 passing tests. The optional PostgreSQL test also passed
  against a separate empty database: competing decisions, rollback, process
  exclusion, restart, exact retries and preservation of legacy extensions and
  delivery leases. The temporary test database was removed.
- Backend: eight passing unit/configuration tests; the existing optional
  PostgreSQL regression was not rerun. All twelve HTTP contract groups passed,
  including the browser adapter, native endpoint aliases and existing fleet APIs.
- Formatting and strict all-target Clippy passed for contract, Operations and
  backend. The five xtask tests passed. Development watches now include both
  new crates.
- Frontend WASM check and production bundling passed. The 49 existing frontend
  unused/dead-code warnings remain; the macOS backend linker still emits its
  existing large `__eh_frame` warning.
- Updated the local development process and verified `/healthz`. Existing
  Pasion administrator sign-in reached Dashboard with all Padmin navigation
  retained; `/hagency/inbox` loaded its empty state successfully.
- Notification tests used a loopback Matrix stub, covering private-room checks,
  lost-reply transaction deduplication, metadata-only notices and seen/snooze.
  The optional worker is disabled in the development configuration; no real
  notification messages were sent.

The Docker image was not rebuilt for this migration. Live hagency-rs delegation,
Agent provisioning/chat and Codex/Claude quota execution were not exercised.
Palpo cleanup was committed on a separate branch and submitted as draft
[PR #512](https://github.com/palpo-im/palpo/pull/512), removing the old application
and its CI with bilingual migration guidance. Its diff leaves all Rust sources,
Cargo manifests/lockfile and Matrix tests/deployments unchanged, has no dangling
application-path references, and passes whitespace checks. Palpo Rust tests
were not rerun for that source/documentation-only cleanup. The PR remains draft
pending the replacement release and client/state acceptance. Draft features for
online association/delegation are not advertised as implemented.

## Fleet terminology (2026-10-05)

- Both hagency-client and hagency-server now use Fleet in UI, source, native
  enrollment APIs, configuration examples and bilingual documentation.
- Workspace library tests: eight passed; the dedicated PostgreSQL persistence
  check was skipped. Operations HTTP workflows: 16 passed; its dedicated
  PostgreSQL check was skipped.
- Thirteen management HTTP contract groups passed, including canonical Fleet
  responses, deprecated Hafleet paths/inputs, owner isolation, conflict rejection
  and byte-identical pairing credentials.
- Native enrollment contract passed with `[fleet_access]` and its old
  `[hafleet_access]` alias, quota, idempotent credentials and owner isolation.
- Client PKCE/owner binding/revocation regression, both Rust Clippy checks and
  both frontend release builds passed. Existing frontend unused-code warnings
  remain.
- Actual Rust client/server integration with controlled Pasion/Matrix peers
  passed: enrollment, automatic import, outbound poll, App Service delivery,
  exact probe receipt and reception verification.
- A controlled browser fixture showed **My Fleets** at `/hagency/fleets`,
  **Fleet connections**, and the existing Matrix/Padmin navigation.
- Persisted IDs, registrations, tokens, protocol keys and audit history were
  retained; no database migration is needed.

2026-10-07 new PostgreSQL transport/domain recovery regression:

- `python3 scripts/test-agent-service-postgres.py`: 22 tests passed, none failed or
  ignored. The runner created and removed its dedicated random database.
- Includes explicit known-result recovery after lease takeover, immutable original
  execution identity, retained unknown execution, stable reply transaction/payload,
  reauthorization of definitely unsent cancellation, ambiguous network preservation,
  active-worker rejection, permanent delivery denial and binding revocation.
- Includes trusted committed provisioning after owner logout and terminal scope
  sweep for late joins. This database test does not itself simulate Matrix network
  failure or prove a live deployment physically removed a late membership.
- `cargo clippy -p hagency-agent-service --offline --all-targets -- -D warnings`
  passed for the new modules.

2026-10-07 real browser login and visual acceptance (isolated live fixture):

- Built executable web assets with `just prepare-frontend`; inspected the actual
  Rust backend in native Chrome Guest, not a mock or developer token form.
- Pasion native sign-in with disposable fixture administrator, personal Matrix
  consent, second administrative consent and OAuth callback reached Dashboard.
  Generic Users, Rooms and Pasion Local Accounts pages loaded real server lists.
- Projects and My agents displayed the new permanent-owner/local-Codex boundary.
  No Fleet, Engagement, allocation approval or native enrollment navigation was
  present. At the inspected 1200×730 viewport forms remained readable and scrollable.
- Found and fixed a real reload defect: the frontend previously rejected a valid
  HttpOnly browser session after its memory-only OAuth bearer was lost. Owner
  pages now revalidate `/hagency/api/session` and use the restricted BFF. Generic Matrix
  management re-enters real Pasion PKCE authorization when a bearer is needed;
  access and refresh tokens remain out of browser persistent storage.
- Repeated with a newly registered ordinary user through Pasion's native account
  registration and personal Matrix consent. Default landing was Projects; only
  Projects/My agents navigation was shown. Reload preserved that verified session.
  Cookie-only logout succeeded; direct navigation to Agents after logout returned
  to Login. No tokens were pasted, injected or exported from the browser.
- Frontend wasm check and final release packaging passed (51 existing generic
  unused-code warnings). HOLD fixture termination runs its normal server and
  dedicated database cleanup; no production account or business database was used.
- This browser pass verifies login, read/navigation, reload and logout. Project/
  Agent creation and policy mutations remain covered by HTTP/PostgreSQL/live
  protocol tests, not asserted as visually exercised here. Current forms register
  existing Matrix Space/Room IDs; the frontend does not yet provide joined-room
  discovery or a Matrix chat UI.

Project administration completion:

- Restricted browser BFF now permits only the fixed Project/Room creation-policy,
  service-state and pause/clear-pause operations. Matrix Room IDs are decoded as a
  single safe path segment; encoded slash, nested encoding and arbitrary operation
  paths remain rejected. Pause commands require an empty body and the existing
  authenticated-cookie/CSRF check; native gateway rechecks actual Matrix authority.
- Project UI provides Room inheritance/allow-list/disabled creation policy and
  administrator Project or Room service pause, including current status/revision.
  Clearing administrative pause explicitly tells the owner to resume bindings;
  creation-policy changes do not stop existing agents.
- These added controls were compiled and covered by fixed BFF capability tests;
  the real browser pass above predates these specific added controls.

2026-10-07 real authorization regression completion:

- `HAGENCY_TEST_NATIVE_PKCE=1 python3 scripts/test-agent-integration.py` passed
  against the actual embedded Pasion/Palpo backend using dedicated disposable
  databases and a real native DCR/PKCE/consent grant.
- Before any Agent existed, Project service pause rejected creation, allocated no
  Agent and stored no command. Clearing it restored eligibility. Encoded Matrix
  Room IDs worked for native Room creation policy and service-state requests;
  disabled policy rejected creation and restoring inheritance restored eligibility.
- Room pause rejected creation; combined Project+Room pause reported both flags.
  Clearing only Room pause left the Project flag and merged servicePaused active,
  and still rejected a new Agent. Clearing Project pause permitted creation.
- Owner GET command status returned the same Agent/binding as stable same-key
  replay. A changed payload returned 409; unauthenticated command discovery was
  refused. Dedicated PostgreSQL ownership tests separately verify another owner's
  same-key status request cannot disclose the original result.
- The original live mention/ACK/start/reply flow and terminal retirement with a
  real late Matrix rejoin still passed after these additional checks. No model or
  tool runner was invoked. All fixture databases were removed by the runner.
- `python3 scripts/test-agent-service-postgres.py`: 23/23 tests passed. New scope
  regression proves an empty paused scope cannot be bypassed by Agent creation or
  a new binding of an existing Agent; disabled creation does not stop a running
  binding, and clearing admin pause does not implicitly resume one.

2026-10-07 routing/reply worker fairness regression:

- Fixed two starvation paths: a single Room state failure previously aborted
  routing/sending for the pass, and fixed oldest-first limits could repeatedly
  select the same unsendable routing transactions or unknown reply intents.
- Trusted worker scans now advance separate circular transaction/reply ID cursors,
  including failed candidates. A routing transaction is compacted only after all
  relevant events/scopes have durably succeeded or been rejected with fresh facts;
  transient or unavailable Matrix observations leave it pending. Healthy events in
  the same transaction can be accepted and later replay deduplicates them.
- Failed Room observations are suppressed for that pass and have an eight-second
  timeout. Routing and accepted-reply processing begin concurrently. These are
  bounded sequential batches, not a guaranteed low-latency parallel Room SLA.
- Reply retries preserve original payload, Matrix transaction and execution IDs.
  Failed network sends remain unknown; fairness never reopens a completed model
  execution. Cursor state is process-local; the durable intent is unchanged and
  restarts resume circular scanning from the initial batch.
- New meaningful PostgreSQL test uses its own additional disposable database to
  avoid contaminating parallel global-worker tests. It covers twelve failed
  routing transactions plus a later mixed failed/healthy Room transaction, and
  twenty-one failed Room reply intents plus a healthy Room's attempted send.
  The healthy event was queued once, transient transactions remained pending,
  unknown reply identity was stable, and all twenty-two model executions remained
  completed. No model was invoked. The nested test database is dropped with FORCE
  even when the test task fails.
- Final PostgreSQL runner: 25/25 passed. Strict agent-service all-target Clippy
  passed. Membership-controller cursor and lifecycle behavior were not changed.

## Final integrated owner roundtrip and restricted Room (2026-10-07)

The rebuilt backend passed `HAGENCY_TEST_NATIVE_PKCE=1 python3 scripts/test-agent-integration.py` after queue fairness and AS compaction changes. The isolated run covered startup roundtrip, retired API 404s, real Pasion DCR/PKCE/consent, creation/pause policy, stable owner command replay, scoped device tool authorization, Matrix mention/ACK/start/reply and retirement with a late real join. It also bound the same permanent Agent to a restricted Space-child Room via existing Matrix invitation/join APIs, verified puppet membership and unchanged restricted join rules. This does not claim automatic Space-to-Room membership propagation. All three fixture databases and private success files were removed; no model was invoked and Palpo source remained unchanged.

## Stopped three-database recovery (2026-10-07)

`HAGENCY_TEST_NATIVE_PKCE=1 HAGENCY_TEST_BACKUP_RESTORE=1 python3 scripts/test-agent-integration.py` passed against the real embedded backend. All original/restored random databases and successful private files were cleaned up. See [exact restoration evidence and boundaries](BACKUP_RESTORE_VALIDATION.md): immutable identity/digests/sent reply, fresh native authority, explicit lease takeover, unfinished execution fenced unknown, new request/reply and late-join retirement all passed without inference.

## Independent private filesystem recovery extension (2026-10-07)

The same recovery flag passed with a different restored private directory and the original directory renamed offline before restart. Complete data/media/config, byte-identical AS/Matrix/Pasion keys and configured password pepper, real Matrix media upload/download, fresh native login/device, immutable reply/unknown execution, new roundtrip and kick/power/retirement regressions passed. See [exact copied paths and remaining whole-machine boundaries](BACKUP_RESTORE_VALIDATION.md).
