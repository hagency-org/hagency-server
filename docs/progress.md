# 进度

## 2026-10-13 — Desktop issue #2 / Server

已保存并独立复审最终方案，再实现 execution-configuration-v1：设备鉴权 API、严格非秘密 DTO、独立 PostgreSQL 队列/心跳/观察缓存、Agent 和请求双方设备 generation/session fences、幂等 create/complete、16 outstanding 上限、单设备 claim/单 Agent mutation、过期 pending=expired / claimed=unknown、七天历史保留。领取永不重发；配置不使用模型 execution lease，不能修改 Agent 主人或执行设备。

验证均从本 server checkout 执行：`scripts/test-agent-service-postgres.py` 在 disposable PostgreSQL 容器中新建随机独立数据库并清理，72 passed / 0 failed / 0 ignored；新配置测试覆盖真实 HTTP A/B、授权隔离、秘密字段拒绝、丢 complete 响应、过期不重发、离线、配额、独立连接并发、设备旋转、撤销/重分配与独立会话注销。OpenAPI 57 owner operations / 4 appservice operations + 11 validator self tests 通过；cargo fmt、agent-service all-target strict Clippy、diff --check 通过。`cargo build --locked -p hagency-server --bin hagency-server` 通过，macOS linker 报大型 eh_frame 性能 warning。

主 agent 的真实 embedded Server/Pasion/Palpo 双进程 NativeOwner 联调通过：B 无 provider，A 保存并校验模型/目录/policy，A/B 重开及真实 owner-direct binding 的执行 Ledger resolver 一致。详见 docs/design/2026-10-13-cross-device-agent-configuration-validation.zh-CN.md。真实跨物理设备、原生可视检查及真实 provider 推理没有本记录的通过证据。未修改生产数据库或运行中的应用。无 provisioned task-writer，文档不是控制平面任务真相源。

追加严格 DTO 校验：ConfigurationComplete 的 nullable result/errorCode 也必须显式存在；新增缺字段回归在原 focused DTO 测试通过，随后 strict Clippy/OpenAPI guards/diff 检查通过。HTTP 入口此前已经要求完整三个字段，未改变端点行为。

## 真实联调发现并修复的鉴权锁顺序问题

真实双进程 smoke 暴露 storage_unavailable，PostgreSQL 明确报告死锁：autocommit Store::authenticate 先锁 session 再等待 owner，而配置事务已持 owner shared lock 再锁所有 owner sessions。此前 domain-only 并发测试没有覆盖 HTTP 初始 authenticate。

根因修复：只读 authenticate 返回不持 row lock 的身份快照，所有 operational Domain/Transport 仍在事务内重新鉴权；Store 的 sign_in、renew、register_device、revoke_device、sign_out 统一先获取现有 domain authority advisory lock 5210750088328904，再锁身份行。保留发布期间的双方 session pins，不通过 client 重试掩盖错误。

新增持 owner row/advisory 的确定性 barrier 回归，验证实际 Store snapshot 与五类 auth mutation 锁顺序；新增 6 路真实 HTTP auth/status/heartbeat 并发，无重试并全部要求 200。最新隔离 PostgreSQL 完整套件 74 passed / 0 failed / 0 ignored，strict all-target Clippy、fmt、OpenAPI/self tests 通过；最新 binary 已重建。主 agent 用该 binary 的最终双进程 smoke 重新执行通过，含具体无效 model/目录错误码及 A/B 重开/真实绑定 resolver；测试额外显式等待隔离 Matrix 预配完成，未加入生产配置请求重试。


## 2026-10-21: Agent ownership badges and public/private service audience

- Added persisted, strict public/private audience with private as the new-allocation default. Migration preserves public for existing non-left project bindings and keeps owner-only identities private; repeated migration preserves saved choices. Owner-only audience changes refuse demotion until every shared Room departure completes. Binding admission/replay, binding authority and execution/reply SQL all require public for project scopes. Owner-direct authorization retains exact creator/room checks.
- Added authenticated public-only discovery with safe metadata and isMine; owner management remains owner-only. Room rosters filter others' private Agents. Web management/discovery now displays Mine and audience badges, publication controls and public services. Updated OpenAPI59 owner routes/4 AS routes and parser support for serde-defaulted/skipped fields;11 contract self-tests pass.
- Verification in edited Server:77/77 PostgreSQL tests with all ignored database tests explicitly enabled,0ignored; wasm frontend check and strict all-target no-deps Clippy for agent-service/frontend pass. Isolated real SQL upgrade check verifies shared=>public, owner-direct-only=>private, new=>private and idempotent rerun. Logs:/tmp/hagency-visibility-server-audience-final.log,/tmp/hagency-visibility-migration-final.log,/tmp/hagency-visibility-server-clippy-final.log,/tmp/hagency-visibility-frontend-check.log. Earlier missing visibility in owner-direct RETURNING caused3failures and was fixed. Reusing a populated test DB caused2outbox fixture failures; a fresh disposable DB passes all77 without changed assertions. Original logs retained. Local stable1.98.1 was refused by repository MSRV1.99; installed1.99.0 was used.
- Source changes only; no deployment, production data migration, restart, commit or push. Temporary PostgreSQL cluster is isolated under /tmp/hagency-visibility-pg.2DsTpF. task-writer is absent in the Client source coordination checkout.


## 2026-10-21: Follow-up Agent audience review

- Added real HTTP regressions for default-private allocation, explicit publication/privacy changes, foreign owner/revoked credential denial, malformed audience payloads, discovery ownership flags and no private/device/internal owner fields. Added a real migration test for shared/direct/left legacy bindings, private default, saved-choice retention on repeated migration and invalid-value constraint rejection. Added an adversarial stale private project binding test: routing/claim/execution/reply candidates and reply claim refuse it; pending dispatch is cancelled. Room roster now also excludes inactive owners.
- Web controls no longer mislabel missing audience as private, disable audience editing on unknown/retired state, and explain the required completed departure. Native Client review also repaired the separate chat-info picker, reset creation defaults and added safe catalog DTO projection and compact badge wrapping.
- Final Server suite:80passed/0failed/0ignored using HAGENCY_AGENT_TEST_DATABASE_URL against a fresh dedicated local PostgreSQL database (/tmp/hagency-visibility-review-server-final.log). Web18passed;wasm check and all-target strict no-deps Clippy pass (/tmp/hagency-visibility-review-web-all-tests.log,/tmp/hagency-visibility-review-web-final.log,/tmp/hagency-visibility-review-server-clippy.log). OpenAPI59/4routes and11self-tests pass. Initial added runtime test expected an inactive scope value, but the API correctly refused with binding_not_running; final test asserts that stronger denial. Initial compile trial had one incorrect sign_out test argument, corrected before qualification. Logs are retained. No production deployment, data mutation, restart, commit or push. Isolated PostgreSQL is stopped after verification.


## 2026-10-21: Publish Agent audience changes to main

- Operator explicitly authorized committing and pushing all Server/Client changes to origin main. Server publication preflight confirms local main matches origin/main, clean whitespace, successful formatting check and the previously recorded PostgreSQL80/web18/strict-Clippy/OpenAPI qualification. Include the new visibility SQL migration with the service, web UI, tests and contract documentation. This source publication does not perform a deployment or migrate production data.


## 2026-10-23: Shared native Project kanban

- Created codex/project-kanban from f48717f7 at operator request, coordinated with Client branch of the same name. Added hagency_project_v1 independent additive migration/pool, strict native board API, live Project/Space/session authorization, task versions, atomic rank/order, committed change sequence, permanent original-intent receipts, comments/activity, archive/restore and bounded filtered pages. No Cloud feature code or Agent execution changes.
- Capability project-board-v1 is conditional on initialized domain and project_board_enabled. Optional host flag defaults true to avoid requiring new deployment inputs; false disables capability/routes while retaining storage. This differs explicitly from the report's suggested default-closed rollout; operators can opt out before pilot. OpenAPI has71 owner/4 AS operations and13 validator self-tests.
- Fresh dedicated local PostgreSQL18 database kanban_final: complete agent-service suite with --include-ignored --test-threads=1 is85passed/0failed/0ignored (/tmp/hagency-kanban-server-final-tests.log). Backend check and strict no-deps Server/agent-service/xtask all-target Clippy pass; fmt and OpenAPI self-tests pass. No production migration.
- Real TCP cross-repo fixture with actual SQL/API and two actual NativeOwner instances passes1/0/0, including share/replay/stale409/reopen/member exit/outsider refusal (/tmp/hagency-kanban-native-e2e-final.log). The dev-only example replaces Pasion introspection/Matrix facts explicitly; it is not real Matrix-account or multi-desktop qualification.
- Earlier parallel full test run failed on existing DomainStore initialization DDL deadlocks; serial run on reused DB then failed2global outbox fixture cases due old rows. Original failures retained; final repository-prescribed fresh+serial run passes85. No skipped failure was declared passing.
- Usage, API, rollback, fixture boundaries and release caveats: docs/project-board.md. Client App584/0/2 existing ignored; engine248/0/7 existing ignored plus Core3/0/0. No Windows/Linux/manual native UI, production restore or performance qualification. No deployment or push.
- Final cleanup stopped only this task's loopback SQL/API fixture and native design-preview processes. Test data and logs remain in ignored target/temp paths for review; user applications and production services were untouched.


## 2026-10-23: Custom columns, WIP, subtasks and checklists (v2)

- Implemented the four operator-requested common features in Server and native Client on codex/project-kanban; matching additive contract in dev-vibe/project-kanban-common-features-contract-2026-10-23.zh.md. No Cloud product changes. Capability v2 explicitly gates the dynamic-column contract; Server/Client must upgrade together.
- Server additive schema_v2 seeds existing columns, preserves task IDs/versions/original receipts, replaces fixed state with per-Project FK, and adds parent/checklist storage. Board row serialization protects configuration sequence, WIP create/cross-column/restore admission and bounded one-level parent relationships. Configuration changes have nullable taskId; progress reflects configured completed columns.
- Native UI adds themed column settings/reorder/WIP controls, fixed stable-ID creation targets, editable/ordered checklist rows and progress, parent navigation/unlinking, child creation/navigation and progress. Same task-version/conflict/original-intent guards preserve nested fields. No placeholder local data path.
- CUA native operation: added/reordered/renamed quality-verification column with WIP1; accepted #2 drag and refused #3 overflow; added/toggled/edited/reordered #2 checklist1/2; created child#18 and moved it done, parent progress1/1; nonempty column deletion refused. Restarted client AND local Server, reopened #2 and verified persisted columns/checklist/child. Read-only receipt /tmp/hagency-kanban-v2-native-receipt.json; local interactive app/API/PG intentionally left running for user inspection.
- Final Server fresh PostgreSQL full suite88/0/0; App591/0/2existing ignored; Core3/0/0 and engine248/0/7existing ignored; separate two-NativeOwner real HTTP/SQL1/0/0 including a maximum multilingual checklist, membership revocation and independent child edits after parent archive. Strict affected no-deps Clippy, format, OpenAPI72+4/13guards and i18n2330/0missing pass. Logs /tmp/hagency-kanban-v2-{server-tests-release,app-tests-release,core-engine-tests,http-tests-final,client-clippy-release,server-clippy-release}.log.
- Found Salvo payload parser's default64KiB while reviewing maximum checklist size; board body parser now explicitly128KiB inside existing router cap. Native board replies bounded4MiB. Initial native HTTP command omitted fixture-info env and failed setup; corrected configured run passes unchanged assertions. No platform/performance/production qualification or push claimed.
