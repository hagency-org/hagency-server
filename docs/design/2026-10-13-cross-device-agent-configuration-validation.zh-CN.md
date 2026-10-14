# Desktop issue #2 实现复审与验证

方案先保存并独立复审，再由 Server、Client native、Desktop 三路 agent 开发；主 agent 汇总复审和联调。各仓库使用 `codex/issue-2-cross-device-config` 分支。方案与协议见同目录设计稿和 review 文档。

## 实现复审收敛

- Server 强制 device bearer，绑定主人、Agent generation、请求方和执行方设备 generation/live session；旧领取不重发，完成丢响应仅重传原始确认。
- A 在领取及实际写入前重新检查请求与授权，在发布结果前再次检查；跨数据库与本地文件不承诺原子撤销，未确认结果明确 unknown。
- Native worker 在 SDK 授权 cookie 发布后启动，Matrix 会话更新有独立锁；心跳独立于慢 provider 和本地调用锁。Applying/原完成回执私有持久化，重启不重复应用。
- 目标 Agent 的活跃 service/runtime 阻止远程 model 保存；provider connect 因为影响主人级关联，检查同主人的所有服务/执行。两者返回 `configuration_runtime_busy`，避免使用旧 profile 或隐式停止其他 Agent，其他 Agent 的模型配置不被错误阻塞。
- 只读 `Store::authenticate` 不取得行更新锁；会话/设备变更在取得任何行锁前使用既有 Domain authority advisory lock。配置持有 owner/session share locks 发布结果，不会再与设备鉴权发生 session/owner 反向等待。
- provider `versionSupported` 表示安全版本探测和当前 provider 合约调用成功，不是仅凭版本字符串做兼容声明。Octos 不声称验证账户；秘密字段不进入结果。
- input/result 各自有界；Work 响应同时含 input/result，native 接收和 journal 上限为 2 × 64 KiB + 16 KiB，HTTP 提交仍受 64 KiB 上限。未知字段、runtime/result 不匹配均拒绝。
- Desktop 只开放 Agent 配置白名单；离线缓存只读、初始错误可见、后台状态读取不抹掉保存反馈，设备 generation 变化清除旧目录和 catalog。未确认 intent 保留键，确认后等内容的新保存使用新键。

## 执行证据

Server 从本 checkout 执行 `python3 scripts/test-agent-service-postgres.py`：74 passed / 0 failed / 0 ignored，使用独立随机数据库并清理。覆盖实际 HTTP、独立连接并发、请求双方撤销和会话注销、过期/丢响应/不重发、缓存 generation、白名单、配额和幂等，以及确定性锁顺序和 240 次并发鉴权/状态/心跳调用。新增严格 nullable DTO 检查通过。Agent-service all-target strict Clippy、fmt、OpenAPI 57 owner + 4 appservice 路由及 11 自检通过，最新 server binary 构建通过。

主 agent 从 Server checkout 执行：

```sh
HAGENCY_TEST_SERVER_BINARY=/Volumes/Data/Works/palpo-im/palpo/target/debug/hagency-server \
HAGENCY_TEST_HOLD_CLIENT=1 HAGENCY_TEST_CROSS_DEVICE=1 \
python3 scripts/test-agent-integration.py
```

真实 embedded Server/Pasion/Palpo、OAuth PKCE 两份 grant、两个 NativeOwner 进程及独立状态目录：1 passed / 0 failed / 0 ignored，辅助控制端 test 在子进程明确调用。B 的 Codex/Claude/Octos executable 全部缺失；仍发现 A 的 fixture provider/version/catalog，在 A 验证 model 和目录、保存 profile/policy、取回幂等结果，B 和 A 重开读回一致。实际创建 active owner-direct binding，用运行时相同 `Ledger::model_profile` resolver 验证配置生效。Server 启动检查及 durable Matrix AS roundtrip 同时通过。

首次联调发现 SDK 初始化 worker 会话竞态并修复；第二次配置链路已通过，但测试误以为新 Agent 自动有 owner-direct binding，修正为真实 ensure + 等待 active；第三次完整链路通过。随后强化具体错误码断言，暴露隔离 Matrix 异步预配就绪竞态和鉴权/配置事务反向行锁死锁。测试现在显式等待真实 Matrix whoami（30 秒有界，超时失败），生产代码统一鉴权写操作锁顺序。最新 binary 的最终完整链路通过，无效 model 和目标目录分别确认 configuration_model_unavailable / invalid_local_policy。故障记录未冒充通过。

Native 最终 library：216 passed / 0 failed / 5 existing ignored；实际 TCP focused 4 passed，覆盖丢完成响应/原确认重试、Applying 不重放、旧 capability 和 calls 锁占用时独立心跳；不同 Agent 的 busy 范围回归另 1 passed。all-target strict Clippy、fmt、diff 检查通过。Desktop 最终 library：458 passed / 0 failed / 2 existing ignored；locked rinx binary 构建通过；i18n：1821 entries / 1430 translated sites / 0 missing。详情见各仓库 progress/native-validation 文档，ignored 项未标作通过。

这些是同一机器上模拟 A/B 的进程测试。provider 是明确的测试程序，没有真实模型推理；未声称完成跨物理设备、真实 provider 推理或原生可视验收。所有数据库、OAuth grant、状态目录及 provider 文件均来自隔离 fixture，没有修改生产账户或应用。
