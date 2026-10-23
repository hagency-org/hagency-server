# 持久知识

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
