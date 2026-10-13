# 持久知识

- 配置协议固定在 `docs/design/2026-10-13-cross-device-agent-configuration-review.zh-CN.md`，Server DTO 位于 `crates/agent-service/src/configuration_types.rs`。队列只传白名单非秘密信息；credentialRef 是 opaque 本地引用，不是凭据。
- 配置 HTTP 路由必须在通用 session bearer 鉴权分流前执行 device bearer 鉴权。`Store::authenticate(false)` 只接受 session token，不接受 device token。
- 配置 worker 的 claim 永不重发，包括 poll 响应丢失。pending 超时 expired；claimed 超时 unknown；丢 complete 响应重试同结果幂等。历史与幂等记录保留七天，调用方须使用唯一 intent key，不可跨原请求生命周期重试。
- DomainStore 授权持 owner/shared row locks；注销会直接 UPDATE sessions，因此配置 publication 另外锁住 owner sessions，再用数据库当前时钟核验请求双方设备和会话 pins，防止独立会话注销穿过检查。
- 新配置表由 DomainStore::open 的 additive migration 初始化；已有 execution-device reassignment 同时访问 transport 表，隔离测试 fixture 要初始化 TransportStore。
- `scripts/test-agent-service-postgres.py` 仅使用 disposable PostgreSQL 随机数据库，支持全量 include-ignored；它不是生产数据验证。2026-10-13 完整测试 72/72 通过。

- 真实配置联调曾发现 Store::authenticate autocommit `FOR UPDATE s,d,u` 与配置 owner→all-sessions 锁顺序冲突。authenticate 必须保持只读无 row lock 的快照；权限使用点由 Domain/Transport 重新核验。Store auth mutation 必须先拿 domain advisory lock 5210750088328904，再锁任何身份/session/device 行。不要靠客户端重试隐藏 deadlock。
- 2026-10-13 锁顺序修复后的完整隔离套件为 74/74；新增实际 HTTP authentication 与 heartbeat/status 并发，不仅测试 DomainStore 方法。
