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
