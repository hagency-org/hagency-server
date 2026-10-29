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


2026-10-27: Added shared permission register/query/decide/take endpoints and schema 25, plus optional immutable owner tool-policy snapshots for successor launches. Dedicated PostgreSQL integration tests on mini4 passed for owner versus manager decisions, session scope, deny, replay/conflict, expiry, exact digest and schema-13 upgrade rollback with unchanged original command bytes. No production database was used by these tests. Evidence and deployment report: hagency-client `.run/shared-agent-permissions-20261027/`.

Promoted to mini4 as `20261028T002404Z` with backup, preserved app-service identity and unchanged active control database. Public readiness passes; schema is 25 and 小白 remains matrix_verified/running. Updated Desktop test windows reopened their existing profiles and the owner upgraded to local schema 101 without changing permissions.

2026-10-28: Deployed canonical original-budget-block recovery fix as release20261028T034316Z/schema26 on mini4; readiness and appservice identity verified, consistent backup retained.15 v3 tests pass. Desktop accepted original block and prepared attempt7, but an independent YOLO metadata-read guard rejected preflight; that expired unclaimed attempt is now permanently fenced locally. Existing 小白 response recovery remains incomplete; no new model turn or Matrix reply was observed.


2026-10-28: Owner direct funding and regression prevention deployed. dspfac can enter an
amount and confirm once (optional note); no second self-approval. Manager requests retain
resource-owner approval. The explicit approveImmediately+ownerFunded request executes both
records atomically under the same transaction, real owner authority, exact budget snapshot
and existing SQL approval triggers. Retry/lost reply reuses the original request and delta;
old omitted/false fields preserve saved command bytes. Native requires both request and
decision capabilities. UI distinguishes authorization/device application from task execution,
labels server-confirmed consumption honestly, and does not label every account an applicant.
Desktop main CI and installer release dependency now run the combined budget/chat regression
gate; server CI already includes ignored PostgreSQL tests and now sets the tested 16MiB stack.
117 Rust, 36 EN/CN bundle, 20 PostgreSQL tests and independent 4-page PDF acceptance pass.
Actual dspfac UI reviewed through confirmation only, then cancelled; no live top-up/model
call. Same budgets, calls, tool policies, app-service identity and canonical inode208631362.
Server 20261028T170141Z; all 3 default+onboarding-test-support Desktops use .run/direct-owner-final-20261028/rinx-isolated.
Evidence: /Users/ychen/home/hagency-desktop/.run/direct-owner-20261028/validation.json,
owner-confirmation.png, owner-budget.png and scoped.diff. Owner left on 小白 budget page.


2026-10-28 river public registration enabled at user request. Pasion config now has
[account] password_registration_enabled=true and password_registration_contact_required=false.
There is no SMTP/email/SMS provider yet; leave optional email/phone blank until delivery is
configured. Native Palpo registration remains disabled because authentication is delegated
to Pasion. Signup URL https://river.ominix.io:19443/auth/register. Config backup on mini4:
/Users/cloud/hagency-server/backups/before-registration-20261028T171619Z/pasion.toml.
Same server release20261028T170141Z, original Palpo config and Appservice identity preserved.
Live browser signup, Pasion login and Matrix password login passed for a unique ordinary
probe account; it was then deactivated and its local credentials removed. Evidence:
.run/river-registration-20261028/deployment.log and validation.json. The initial test timed
out only because AccountOverview is /auth/ rather than /auth/account; the same created
account was reused for verification/cleanup, not replaced by another signup.
AgentMail proposed by user: SMTP TLS to smtp.agentmail.to:465 is reachable from mini4,
compatible with existing Pasion email.provider type=smtp. No AgentMail organization or
key created yet; owner's email/OTP is pending. AgentMail registration/account setup must
not silently rotate a preexisting account's API key or enable required email verification
before actual delivery succeeds.


2026-10-28: 小白 H100 follow-up cancellation fixed and deployed (server 20261028T173555Z,
all three Desktop profiles .run/xiaobai-context-20261028/rinx-isolated). The request was
accepted/acknowledged but stopped before context freeze/model admission: the server had
regenerated historical plain Matrix content with the newer Markdown renderer, invalidating
immutable delivery evidence. Compare original closed content instead; new rich receipts
carry exact optional formattedBody evidence, validated natively against supported JSON
encodings. Omitted field preserves old receipt bytes/digests. Validate first, then exclude
HTML from provider input while retaining original Markdown. No history rewrite or new
migration. Native context preparation now logs bounded stage/error code at WARN.
All 16 combined release checks pass: 135 Rust tests plus bundle checks, and 15 server
context tests pass including 13 disposable PostgreSQL tests. Native offline subprocess
follow-ups are included in the release gate using a freshly built actual probe. Six live
historical receipts passed read-only content/provenance checks before and after upgrade.
Server readiness/Matrix startup roundtrip and restored Desktop sessions verified. Existing
new owner-funded +1M at17:25:25UTC was independently applied: ceiling9M revision5,
available1,907,955; 8 calls, known251,453, retained7,092,045, policies and owner canonical
inode208631362 preserved. No new prompt, top-up, hold release or cancelled-work replay.
Edward remains in 小白 chat; cancelled “和H100比较呢” needs a new explicit message.
Pasion signup enabled/contact-not-required settings and Appservice identity preserved.
Evidence: /Users/ychen/home/hagency-desktop/.run/xiaobai-context-20261028/validation.json;
investigation: /Users/ychen/home/hagency-client/.run/xiaobai-check-20261028-latest/handoff.json.
