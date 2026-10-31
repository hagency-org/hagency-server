# Original AS ciphertext custody checkpoint (2026-10-27)

Copy exact original pending AS ciphertext under the shared routing/compaction
transaction lock into a separate inbox; never accept a client event as provenance.
Canonical sorted SHA256 commitments and immutable binding/Agent generation remain
through expiry tombstones. Plain Poll cannot see this table; known Agent senders
never create loops. Current lease, assignment, generations, membership, pause,
crypto state and expiry guard every poll/read. External relations are opaque.
Reading/SDK decryption is not task ACK/admission/start; Server retains no plaintext.
Fresh intake2 proves actual normal/thread AS messages after compaction, current
negative scopes and native restart. Independent actual-HS/Python/PG proves both
original commitments, calls0/live leases0 and one historical lease retained.
Native owned jobs must not take calls while Stop may hold it and join them.
Local inbox/history/ACK and full encrypted Codex replies/files remain open.

# Owner-scoped crypto receive checkpoint (2026-10-27)

Keep identity7 separate from receiver8 operations. Force device-bound zero-wait
empty-room sync and accept the SDK fixed use_state_after=true hint only. Owner
master signatures must use the Agent permanently pinned user signer; strip other
signers. Crypto evidence does not establish original AS event origin or execution
authority. Original receive task registration and stop snapshot share the slot
lock; stop/shutdown join tasks even if the observer disappears. Actual fixtures
must start the same identity/membership cleanup worker as backend. A fresh SDK
store cannot reuse an old Matrix device (OTK collision). Fresh receive5 proves
actual owner ciphertext, exact negative scopes/tamper and native restart1→2;
OpenSSL Agent→owner signature and independent ledger calls/lease rows0 pass.
Original encrypted admission/ACK/current-member/lease/replies/files remain open.

# Independent Agent crypto identity checkpoint (2026-10-27)

Keep permanent Agent/host Matrix device keys separate from rotating Hagency
credentials. Pin account cross-signing keys across all hosts; no reassignment
can silently reset them. Commit public digests before HTTP and hold actual scope
rows through bounded IO, using a separate crypto connection and4request slots.
AS registration inhibits login; global versions must not masquerade as an
unregistered Agent. Actual device whoami probes support without lying about the
advertised Matrix version. Client retains private encrypted SDK keys. Actual
identity6 proves two Agents, revocation and native credential1→3 continuity;
calls/leases0. This does not open encrypted task/reply/file or owner trust gates.
Detailed contract and actual/failed qualification evidence are in dev-vibe.

# 持久知识

- Palpo main `296f6bdb` includes the 2026-10-30 publication migrations. Stop all
  old writers sharing a Matrix database before migration; start one upgraded
  instance first, then the remaining upgraded instances. Preserve opaque sync
  tokens and follow upstream UPGRADING.md. A source lock update does not upgrade
  an already running service or establish full recovery qualification (#605).

- Synapse资格示例的OAuth桥可验证多个实际主人SDK token，但必须逐token查真实
  whoami并拒绝Bob/无效token；不能将第二台设备的任意字符串视为主人。可选
  --retain-owner-password仅保存新隔离test主人的密码，默认关闭，绝不用于生产。
  双SDK/双NativeOwner/真实Codex远程审批联合验收通过，但普通Room任务不代表
  加密Agent执行已完成；生产OAuth/人工UI仍是独立门槛。

- 配置协议固定在 `docs/design/2026-10-13-cross-device-agent-configuration-review.zh-CN.md`，Server DTO 位于 `crates/agent-service/src/configuration_types.rs`。队列只传白名单非秘密信息；credentialRef 是 opaque 本地引用，不是凭据。
- 配置 HTTP 路由必须在通用 session bearer 鉴权分流前执行 device bearer 鉴权。`Store::authenticate(false)` 只接受 session token，不接受 device token。
- 配置 worker 的 claim 永不重发，包括 poll 响应丢失。pending 超时 expired；claimed 超时 unknown；丢 complete 响应重试同结果幂等。历史与幂等记录保留七天，调用方须使用唯一 intent key，不可跨原请求生命周期重试。
- DomainStore 授权持 owner/shared row locks；注销会直接 UPDATE sessions，因此配置 publication 另外锁住 owner sessions，再用数据库当前时钟核验请求双方设备和会话 pins，防止独立会话注销穿过检查。
- 新配置表由 DomainStore::open 的 additive migration 初始化；已有 execution-device reassignment 同时访问 transport 表，隔离测试 fixture 要初始化 TransportStore。
- `scripts/test-agent-service-postgres.py` 仅使用 disposable PostgreSQL 随机数据库，支持全量 include-ignored；它不是生产数据验证。2026-10-13 完整测试 72/72 通过。

- 真实配置联调曾发现 Store::authenticate autocommit `FOR UPDATE s,d,u` 与配置 owner→all-sessions 锁顺序冲突。authenticate 必须保持只读无 row lock 的快照；权限使用点由 Domain/Transport 重新核验。Store auth mutation 必须先拿 domain advisory lock 5210750088328904，再锁任何身份/session/device 行。不要靠客户端重试隐藏 deadlock。
- 2026-10-13 锁顺序修复后的完整隔离套件为 74/74；新增实际 HTTP authentication 与 heartbeat/status 并发，不仅测试 DomainStore 方法。


- Shared native Project kanban uses separate hagency_project_v1 schema and four-connection pool; do not mix it with operator tasks or Agent domain migration versions. Board initialization has its own advisory lock and avoids DDL on a valid reopened schema. Authorize_identity retains existing identity/session/device row locks without imposing the old global domain advisory lock on every board.
- Every board mutation obtains fresh trusted Space membership before SQL, locks identity/project and then Board, and rechecks age after waits and before commit. Board row commits order/sequence/task/history/receipt atomically. Same intent+canonical payload replays its ORIGINAL result; clients re-read for current state. Receipts are permanent and scoped by Project+actor+intent.
- Run the full existing agent-service PostgreSQL suite against a NEW disposable database with include-ignored and test-threads=1. DomainStore startup still runs legacy DDL; parallel tests can deadlock, and reused global outbox rows contaminate older fixture assertions. Never hide this with production retries or weakened tests.
- project_board_fixture is a dev-only actual Server/SQL test host with explicitly fake Pasion/Matrix identity sources on loopback55474. It refuses non-loopback database URLs or database names not prefixed kanban_. Its bypass endpoints must never be included in the production router.


- Project board v2 replaces hard-coded states with 2–12 ordered column definitions. Preserve stable column IDs through reorders and creating-task drafts; labels are presentation. Keep a completed column. Delete only columns with no active OR archived tasks. Server must enforce WIP under the same board row lock for create, cross-column move and restore; filtered totals are not WIP occupancy.
- Parent/checklist values are part of versioned Fields. Preserve them through every editor fill/save, conflict draft and original retry; summary cards omit checklist bodies. Parent links are same-Project, one level, at most100children, and independent of parent archive/completion. Existing parent links remain editable after parent archive.
- Empty newly added Fields serialize with skip_serializing_if to preserve old receipt digests. Board schema_v2 is additive within hagency_project_v1 and preserves v1 receipts/IDs; discovery only advertises project-board-v2 to prevent old five-column clients silently misinterpreting custom workflows.
- Salvo Request::payload defaults to64KiB even with a larger outer size_limiter. Explicitly set the board parser secure max to128KiB and exercise an actual maximum multilingual checklist over HTTP; only increasing a post-read length check does not increase accepted input.

- Complete desktop board integration now uses project_board_fixture's explicit Matrix/OAuth test protocol. whoami must include FIXTURE_ALICE/FIXTURE_BOB matching the SDK session device; metadata must include both query and fragment response modes. Empty sliding-sync responses must only return requested list names, since encryption and room-list connections differ. These test routes stay in the example, never app.router().

- 看板协作能力为 project-board-v3，增量 schema v3；旧 v2 Client 需同步升级。通知在变更/receipt 同一事务产生，只投递给当前项目成员，操作者不收自己的通知。附件字节留存于独立表，仅精确上传路由允许 3 MiB body，其余 API 保持 128 KiB；单附件解码后最多 2 MiB。项目内通知不等于系统推送。

## 看板图片引用回执

Client 插图应使用上传事务返回的 attachment ID，不能根据文件名或详情列表顺序猜测。该 ID 随原幂等回执持久化，重试必须相同；其他 mutation 的 attachment 可为 null。客户端还需同步放宽本地转发请求的 Base64 预算，单改 Server 上传上限不足以支持截图。

- **Megolm 发布 envelope，2026-10-28 review：** 除 algorithm 和 ciphertext 外，
  session_id 也是必需字段。发布前须拒绝缺失/非字符串/空值/控制字符，不能先
  消耗永久 possible 发布见证。用真实 PG 断言 status 仍无 publication、原任务
  仍 running，随后原 request 合法载荷仍有首次 custody。device_id/sender_key
  已废弃，不能重新强制要求；正常 SDK 的 org.matrix.msgid 仍允许。

- Room type filters must be applied before the server's count/page slice; filtering only one fetched page loses later matches and leaves incorrect pagination. Frontend RoomFilter is part of the cache key, and use_resource must read page/search/sort/filter signals inside its reactive closure. Empty encrypted-room key backup reads distinguish a valid empty sessions map from unknown/deleted/foreign backup versions; never bypass the Client SDK history-sharing preflight on an arbitrary error.

- Operator preference (2026-10-29): always use current upstream Palpo main. Keep the workspace dependency as branch="main", refresh Cargo.lock deliberately, and submit Palpo bugs upstream. A dependent PR can lock the latest-main-based upstream fix for reproducible CI, but must clearly state the upstream PR dependency; do not keep a permanent rev pin or an old-base custom variant.

- Quality review 2026-10-29 at `30264bc`: Salvo decoded route matching differs from raw URI prefix checks in DelegatedAdminGuard; actual in-process guard tests confirm encoded/repeated-slash bypass of the extra Hagency boundary, while real Palpo still requires Matrix administrator identity. BrowserAuth login rate keys include source ports; an actual test confirms a new port resets the bucket. The 4096 per-owner lifetime file witnesses and 40 MiB retained Board attachment bytes are explicit contract limits, not implementation violations; policy changes must preserve permanent replay witnesses. The full isolated PG 18.6 suite (134 passed), other tests (49 passed), and strict Clippy pass; audit has 14 vulnerability records and 6 warnings requiring per-path triage. Report: `dev-vibe/server-quality-review-2026-10-29.zh.md`; evidence: `.run/quality-review-20261029`. No production fixes applied.

- Hardening 2026-10-30: Admin guards use Salvo PathState; every BFF request including GET requires the original CSRF. Session restoration is separate, and each tab pins its actor in memory. Logout revokes the original sid/token without a late Cookie-clear header or an unbound Pasion SSO logout. Permanent file witnesses no longer consume active count; append-only chunks preserve legacy prefixes and unknowns. Board v7 clears deleted payloads while retaining size/digest/receipts. Stop old writers before both storage upgrades. Worker drain keeps internal HTTP alive for 35 seconds; deployment/dev supervisors allow 75 seconds. Current-frame JSON projections are shared, but fresh frame reads and completeness checks remain mandatory. Updated dependencies leave two explicit upstream cryptography risks and five warnings; the CI policy expires 2026-11-29 and must not be called a clean audit. See `dev-vibe/server-quality-hardening-2026-10-30.zh.md`.

- PostgreSQL container readiness must probe TCP (`pg_isready -h 127.0.0.1`), not its default Unix socket: the official entrypoint starts a temporary socket-only server during initdb, then stops it before final startup. A socket probe can prematurely release Compose dependencies or a backup fixture. The 2026-10-30 deployment gate reproduced this race; require the final TCP listener in both loop and final assertions.

- Pasion's current bootstrap/key-generation and password hashing use rand_chacha0.3, whose SeedableRng is rand_core0.6. Import the trait through rand_chacha::rand_core; a separate direct rand_core0.10 trait is incompatible even though both versions coexist. Removing the redundant direct dependency does not remove upstream0.6 or migrate permanent cryptographic identities.

- Bootstrap-admin provisioning is asynchronous: pasion::bootstrap_admin commits a durable ProvisionUserJob, then the worker creates/mirrors the human Matrix identity and role after HTTP starts. Successful AS readiness alone is insufficient for fixture login. Wait for the actual Matrix users row and expected active/non-guest administrator role before minting fixture OAuth grants; never fix this timing race with direct role writes or permissive BFF authentication.
