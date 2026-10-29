# 持久知识

- 配置协议固定在 `docs/design/2026-10-13-cross-device-agent-configuration-review.zh-CN.md`，Server DTO 位于 `crates/agent-service/src/configuration_types.rs`。队列只传白名单非秘密信息；credentialRef 是 opaque 本地引用，不是凭据。
- 配置 HTTP 路由必须在通用 session bearer 鉴权分流前执行 device bearer 鉴权。`Store::authenticate(false)` 只接受 session token，不接受 device token。
- 配置 worker 的 claim 永不重发，包括 poll 响应丢失。pending 超时 expired；claimed 超时 unknown；丢 complete 响应重试同结果幂等。历史与幂等记录保留七天，调用方须使用唯一 intent key，不可跨原请求生命周期重试。
- DomainStore 授权持 owner/shared row locks；注销会直接 UPDATE sessions，因此配置 publication 另外锁住 owner sessions，再用数据库当前时钟核验请求双方设备和会话 pins，防止独立会话注销穿过检查。
- 新配置表由 DomainStore::open 的 additive migration 初始化；已有 execution-device reassignment 同时访问 transport 表，隔离测试 fixture 要初始化 TransportStore。
- `scripts/test-agent-service-postgres.py` 仅使用 disposable PostgreSQL 随机数据库，支持全量 include-ignored；它不是生产数据验证。2026-10-13 完整测试 72/72 通过。

- 真实配置联调曾发现 Store::authenticate autocommit `FOR UPDATE s,d,u` 与配置 owner→all-sessions 锁顺序冲突。authenticate 必须保持只读无 row lock 的快照；权限使用点由 Domain/Transport 重新核验。Store auth mutation 必须先拿 domain advisory lock 5210750088328904，再锁任何身份/session/device 行。不要靠客户端重试隐藏 deadlock。
- 2026-10-13 锁顺序修复后的完整隔离套件为 74/74；新增实际 HTTP authentication 与 heartbeat/status 并发，不仅测试 DomainStore 方法。


## Shared permission decisions (2026-10-27)

Schema 25 adds finite, authenticated shared Agent permission requests. Register/take are bound to the executing owner's exact live runtime/activity/work; query/decide are visible only to that owner and the original requesting Matrix identity. The native owner marks workspace-only file requests as manager-delegable; broader requests and YOLO remain owner-authorized. Decisions pin the original request digest, record actor/owner status/time, and are consumed exactly once before provider response IO. There are no durable server-wide session grants: the native Codex session owns session scope. Upgrade tests must drop management_shared_approvals before simulating older schemas.

2026-10-28: Schema26 verifies retained v2 raw block hashes and derives canonical originalBlock proofs for v3 consumers. The DB start validator must use the same proof; original bytes/digests remain immutable. The encoding helper is only for closed ASCII-key/integer block DTOs, not a general JavaScript canonicalizer. Real v25 migration rollback/reopen and15 budget-execution regressions pass. See client .run/xiaobai-queue-20261028/.


2026-10-28 owner direct funding: Do not model a resource owner funding their own
Agent as a pending request requiring a second human approval. New clients send
ownerFunded=true and approveImmediately=true only after an explicit amount confirmation;
canOwnerFundDirect on the budget read advertises the server behavior. Old missing/false
flags retain old pending semantics and byte-identical command digests. The server validates
the actual resource owner and budget snapshot, then records the request and its approval
inside one DB transaction using the existing decision checks and SQL evidence triggers.
It returns the original request receipt plus the latest approved top-up. No schema change.
Recovery reuses the original command ID; it must never issue a replacement top-up.
Native direct funding requires both RequestAgentTopUps and DecideAgentTopUps, including
before HTTP, after HTTP and saved-command replay. A script field is not owner authority.
The device still checks the full resource/account budget, siblings and retained unknown
usage. Authorization, device application and task execution remain distinct observations.
The MiniApp's budget is a server accounting view; its consumption field is not a live
provider meter, and null native remaining must never be presented as zero.

Desktop regression command: python3 tools/check-hagency-regressions.py --output .run/regressions
--client ../hagency-client (one shell line). CI and installer releases run the Desktop
checks; release packaging depends on their success. Keep budget/role/replay checks with
Markdown, previews, activity and answer actions/export. The checker rejects empty Rust
test matches. Native HTTP fixtures need RUST_MIN_STACK=16777216. Server tests require a
disposable PostgreSQL database; never use the production recovery database. Evidence:
/Users/ychen/home/hagency-desktop/.run/direct-owner-20261028/.


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


2026-10-28: Shared-thread delivery validation must compare immutable original Matrix
content, never regenerate it through the current presentation renderer. Optional
formattedBody is receipt evidence only: omit when absent to preserve old bytes, validate
closed body/thread/format fields plus original content digest and source/work bindings,
and strip HTML only after validation from provider input. Rich evidence needs coordinated
server/native rollout (old strict clients reject the added field). Previously saved rich
receipts without evidence remain unsupported; never rewrite their immutable digest.
River preflight had6 receipts/0rich so that limitation does not affect this deployment.
Run the combined Desktop regression gate with --client to include real offline-probe
context tests (both filters, --include-ignored); run server shared_context_ against a
separate disposable PostgreSQL DB. Context admission failure must emit bounded stage/code
without raw prompts or credentials. Historical cancelled work stays terminal; a new
explicit chat message creates the next attempt. No live model turn was used for this fix.

2026-10-28 shared Room members: product intent is that a shared/pool Agent serves the
people in its Room, not only its manager. Any joined member addresses it by explicit
mention; only the manager of a pool assignment's dedicated conversation is addressed
without one (`Membership::addressed_by(sender, mentions)`). Pool conversations stay
invite-only, non-guest, non-public, unencrypted and joined by the applicant, but may hold
other members (`gateway::pool_conversation`); an extra member no longer blocks or fences
the Agent. Per-message delivery facts check the actual sender's fresh membership; runtime
authority checks still observe the manager. Manager-delegable permission decisions stay
with the manager: `management_shared_approvals.requester_mxid` stores the manager while
`payload.requesterMxid` keeps the actual sender. Chat status still shows each sender only
their own intake. The native executor no longer requires work requester == manager.
Updated PostgreSQL cases in shared intake/execution tests were not run (no Docker).
The database trigger validate_shared_work also required requesterMxid == manager; a
member's work insert raised shared_work_binding_changed and budget-work/poll returned
503. Schema 28 (management_shared_room_members_schema.sql) redefines only that function
without the condition; demand triggers still require requester == retained sender. It
was applied to a restored scratch copy of River (up and down to 27) before deployment.
River release 20261029T015402Z runs schema 28; admin's mention was answered in thread.
Deploy gotcha: a locally built hagency-server links libpq at .run/libpq; rewrite it with
install_name_tool -change ... @executable_path/../lib/libpq.5.dylib and ad-hoc codesign,
or launchd fails with OS_REASON_DYLD (cost 2-3 min of River downtime once).
