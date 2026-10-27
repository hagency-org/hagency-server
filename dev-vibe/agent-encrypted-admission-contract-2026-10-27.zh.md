# 原始加密输入准备与确认

`agent-encrypted-admission-v1` 增加 ciphertexts/prepare、ack、admission。
当前设备以原始 inboxId/sourceDigest 和当前租约准备任务；只有 prepare 接受实际
SDK 解密后认证的 thread/mentions 投影，不接受明文正文或信任布尔值。

每步重新读取原始密文、当前 owner/session/device、指定执行主机、Agent 与 binding
代次、Room 成员及 lease，等待事务后再校时。事务中的 immutable mapping 先绑定
原始 ciphertext，再创建正文为 canonical ciphertext 的 owner_event。独立
`encrypted_*` 状态、数据库触发器与普通执行入口共同隔离，旧明文启动接口拒绝。

ACK 之前主机须原子保存实际明文、原始密文、source/seal 和本地/Server历史摘要。
ACK 为幂等原始回执，不能开始模型或工具。租约释放/换代仅重置未执行的状态，保留
原始 dispatch/mapping；过期变成不可重启的 encrypted_cancelled。相同 Agent/Room/
session/密文字节的 seal 摘要规范化 Base64 padding；换外层 event_id/时间不能
创建第二个任务。本地账本也实施同样约束。

实际联合 fixture `/private/tmp/hagency-encrypted-ack-20261027-1/ack1`：真实 Synapse、
主人 SDK、两个正常 Agent 与 NativeOwner 接收普通/线程输入、重复 ACK、重启1→2
及新租约保留原 ID。SDK 专用接收取消未经认证外层 m.relates_to 的兼容回填，真实
负例拒绝伪造线程、密文新事件重放及错误作用域。独立实际 HS/PG/SQLite/Python
核对2份 admission 与3份 ciphertext 的摘要；AS原始正文均已压缩，普通明文0、
模型调用0、活租约0、历史租约行1。所有本批服务已停，数据保留，无付费推理。

新鲜专用 PG `--include-ignored --test-threads=1` 123项全通过/0忽略；严格
all-target Clippy、格式与 OpenAPI103 Owner+4 AS/30负例通过。最初不带
include-ignored 的35通过/88忽略不是全量验收。Client 详细报告另记原始诊断失败。

完整加密模型/工具 Start、原始加密回复和文件收发、共享请求者信任与 A3 未完成；
本批不能代替人工桌面、生产 OAuth 或跨平台联合资格。禁止以 ACK 宣称执行成功。
