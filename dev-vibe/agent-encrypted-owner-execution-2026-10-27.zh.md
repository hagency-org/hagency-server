# 主人私聊完整加密 Codex 执行与文件链

本批在原有 ciphertext custody/admission/ACK 上接入实际 Native/Codex 执行、
原始加密回复与双向文件；此前 ACK 文档保留为历史检查点。本批支持经过验证的
主人私有 Agent Room，共享 Project 请求者信任和跨主机密钥恢复继续关闭。

## Server 协议与边界

新增 agent-encrypted-owner-execution-v1 能力及固定 POST
/hagency/api/v1/execution/encrypted/action，严格 action 为 start/authorize/
finish/publish/status。所有活操作重新检查原始 dispatch、当前主人设备、原绑定
代次、私聊成员关系、执行主机和租约。普通明文执行接口拒绝 encrypted_*。
相同 start 只返回原执行，不再次授予模型许可；未知或停止的工作不能重启。

publish 只接受有界 SDK Megolm 密文和指向该主人的 Olm key shares；不接受正文、
文件名、密钥或任意用户/URL。实际 SDK 的可选 org.matrix.msgid 经长度/控制字符
检查后保留。独立 immutable publication 在网络前写 possible，首次事务才交出
发送工作。重复请求只能查询原记录，换载荷冲突；未知不重发或另起消息。
每次 key share 和 Room 发送前重新授权，实际 Matrix HTTP 回执才更新 sent。
文件 sent 不关闭任务，原加密回复 sent 才进入 encrypted_completed。

Receive broker 只补充当前永久主人的 signed_curve25519 key claim，仍不开放
任意 peer 或通用 Matrix 写代理。新建主人私聊默认启用 Megolm。加密附件原字节
只在桌面 SDK/本地运行时解密；上传前由 SDK 加密，Server 的明文 file_outbox
不参与这条链。原密文、发布见证、失败与费用均保留。

## 实际联合验收

真实 Synapse、PostgreSQL、AS/workers、桌面 SDK、当前 NativeOwner 服务和真实
Codex 成功证据：/private/tmp/hagency-encrypted-codex-20261027-2/flow1/attempt2。
真实输入加密附件131073字节；Codex 实际执行命令生成131097字节输出，主人实际
SDK 解密并逐字节核对。文件及回复各有一条真实 Agent 加密事件回执。成功任务
仅调用一次，input44629/output467，共45096 Token；完成 receive_file、命令及
send_file 审计，并记录 whole_tree 停止。两次服务重开后调用和费用不增加，原始
文件与回复仍可解密；正常新私聊默认加密亦实际验证。

独立 HS/PG/SQLite 检查原始 source/transport 摘要、SDK 密文与实际事件、执行 ID、
文件摘要、工具审计和结算费用：成功发布2、Server 明文文件行0、活租约0、历史
租约行2。两次先前付费失败未重放：绝对路径选择限制和 SDK msgid 校验均已修复，
失败与费用保留；本批三次实际推理总195281 Token。测试 OAuth introspection 桥接
不等于生产 OAuth 或人工桌面资格。仅 macOS 实际联合结果，不替代其他平台。

全量新建专用 PG 显式 --include-ignored：125通过、0忽略；严格 all-target Clippy
及格式通过。OpenAPI105 Owner +4 AS，34个负例通过。Client 配套报告：
https://github.com/hagency-org/hagency-client/blob/codex/remote-owner-approvals/dev-vibe/agent-encrypted-codex-validation-2026-10-27.zh.md

## 2026-10-28：合并前直接复核与发布校验修复

复核起点为 Server PR7 f24b95d；配套 Client PR9–12 已合入 main b906dd68。
确认用途是接通原附件、独立 Agent 身份、原始加密接收/执行与回复/文件，
不是把 ACK、未知发送或公开历史当作执行权限。检查了当前 owner/device/
Agent/binding/lease fences、只面向永久主人的受限 broker、明文入口隔离、
不可变发布见证及实际 Matrix receipt。共享 Project 加密 requester 仍关闭。

发现 P2：validate_payload 未要求 Megolm 必需的 session_id，缺失该字段的
发布可先创建不可逆 possible 记录，再尝试发送无法解密的事件。新单元回归
在原代码明确失败于 missing session_id；修复后通过。现在 session_id 和
ciphertext 必须为非空且无控制字符的字符串，保留原 ciphertext 与总长度上限；
不强制已经废弃的 device_id/sender_key 字段，不改变正常 SDK 载荷。
规范：https://spec.matrix.org/v1.17/client-server-api/#mroomencrypted

扩展实际 PostgreSQL 原执行/发布回归：缺失、null、数字、空串、控制字符均
在创建 publication 前拒绝，status 仍为空，原任务保持 encrypted_running；
随后原 request 的合法载荷仍恰好首次获得发送 custody，原重复/变更/撤租约/
晚回执断言保留。专用全新 loopback PostgreSQL 库全套显式 include-ignored、
单测试线程：126 passed、0 failed、0 ignored。没有新付费推理或生产数据库操作。

同步 OpenAPI 载荷说明；重新审阅该源文件后只更新其 x-source-sha256。
105 Owner/4 AS 操作和34个 validator 自测通过。原质量 CI40505663342 的
125项实际成功保留，不冒充本修复新头的 CI。新头远端结果将记录在 PR 评论，
避免为最终结果再产生 docs-only head。

本地证据目录 /tmp/hagency-server-pr7-review-data：megolm-before.log（原失败）、
megolm-after.log、agent-service-all.log、openapi-after.log、clippy.log。

受影响 agent-service 全目标严格 Clippy（-D warnings）、全 workspace 格式和
空白检查通过。真实模型、生产 OAuth 和人工/跨平台联合验收仍沿用原有边界。
