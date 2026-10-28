# Agent 附件元数据：A2 第一批实现

2026年10月27日。基于重新拉取的 Server main `6340df32`，功能分支
`codex/agent-file-transfer`。这是文件收发的协议与持久化基础，A2 尚未完成。
配套 Client 分支同名，Client main `42e59b9e` 已包含在其历史中。

## 已实现

- Appservice 只从原始 `m.room.message` 的 file/image/audio/video 事件提取
  元数据；保留原 caption、标准提及和 thread。编辑、任意 HTTP/file URL、
  加密 Room 事件不进入当前 Server 明文执行链。
- 原 `owner_events` 与附件在同一个数据库事务中创建。重复事件必须匹配
  包含附件描述的原摘要；改变下载源、文件名或声明大小不能替换原记录。
  附件描述禁止更新/删除，老文本事件的摘要和 Dispatch 格式保持兼容。
- `events/poll` 增加默认 false 的 `acceptFiles`。旧轮询在 SQL LIMIT 前排除
  附件，避免占满批次或让旧 Client 把文件名当作已读取的文件内容。
- 新设备接口 `POST /hagency/api/v1/execution/files/attachments` 只接受已有
  Start DTO：lease、dispatchId、executionId。没有可供调用者选择的 Room、
  event、URL 或密钥参数。返回原执行编号及零/一个附件描述。
- 读取必须仍是原 running 执行，拥有当前设备授权、租约和 binding generation，
  通过 Gateway 的当前 owner/requester/puppet 成员与发言权限检查。已观测到
  requester 离开会封存原请求，后来重新加入不能恢复这次附件读取。
- `agent-attachment-metadata-v1` 仅在 transport 已配置时出现。它不表示
  receive_file、send_file 或加密 Room 执行已经可用。

## 线格式

附件外层使用 camelCase，严格拒绝未知字段。source 是私有宿主数据：plain
保存唯一 MXC；encrypted 保存 Matrix v2 文件参数，校验 32 字节 AES key、
16 字节 IV、32 字节 ciphertext SHA-256 及 JWK 字段。这里的 encrypted file
不表示 Server 已解密 `m.room.encrypted` 事件。

`descriptorDigest` 是下列数组的递归按键排序、紧凑 UTF-8 JSON 的 SHA-256：

```
[filename, messageType, contentType, declaredSize, source]
```

它标识原始声明，不是下载后明文字节摘要。两仓库保留相同的纯明文和 v2
加密合成向量，避免 serde_json preserve_order feature 不同导致协议漂移。
文件名/多行 body 是不可信展示数据，不能用作落盘路径。Debug 隐去 source；
Client 为模型另设不含 MXC/密钥的 Summary。

## 验证与原失败

使用独立 PostgreSQL 18 临时实例的合成账号和数据库，未访问生产 Room。
按仓库 CI 规定串行执行完整 `hagency-agent-service` 测试，95 项通过、0 忽略。
实际 Salvo 路由验证旧/新轮询、设备 bearer、精确执行、未知参数拒绝、成员撤销
和 capability 的有无；PG 回归验证事务去重、禁止改写、不同设备和执行的拒绝。
严格 Clippy、格式与 OpenAPI 78 个 owner/4 个 AS 操作及负向漂移测试通过。

日志：`/tmp/hagency-files-server-serial-final.log`、
`/tmp/hagency-files-server-attachment-final.log`、
`/tmp/hagency-files-server-clippy.log`、`/tmp/hagency-files-server-openapi.log`。

首次全量测试误用了 `--test-threads=4`，77 通过/18 失败。PostgreSQL 日志明确
记录既有 DomainStore 初始化 DDL 与其他测试查询互锁；原日志保留在
`/tmp/hagency-files-server-all.log` 和独立 PG postgres.log。随后遵守仓库既有
`--test-threads=1` 配置在新库通过。又一次复用保留数据的测试库时，47 条旧通知
使限 20 条的全局候选查询无法覆盖两个新测试记录（93 通过/2 失败）；该日志
保留在 `/tmp/hagency-files-server-reused-db.log`。最终遵守 CI 的每轮新库约束，
完整 95 项再次通过。没有加重试或放宽断言掩盖原失败。

## 后续交付门槛

A2 仍需当前运行链中的附件目录/receive_file、主人精确审批、有界认证下载和
加密文件校验、生成私有路径的原子落盘、send_file 的冻结字节与可靠投递状态、
未知结果不重发和重复调用回归。完成前 Client 不启用附件轮询。加密 Agent
Room 与远程审批继续属于 A4 的完整目标。本批没有部署或真实文件投递验收。


2026年10月27日后续：Server 原请求分块暂存、上传/发送两阶段所有权及安全投递历史
已实现，见 [Server 文件投递实现与验收](agent-file-delivery-contract-2026-10-27.zh.md)。
本文件上方保留第一批历史，当前 Client 接收已接通；Client 发送接线和完整 A2
联合验收仍需完成，未部署。
