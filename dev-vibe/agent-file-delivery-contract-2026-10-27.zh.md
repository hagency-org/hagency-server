# Agent 原请求文件投递：Server 实现与验收

2026年10月27日，功能分支 `codex/agent-file-transfer`，基于重新 fetch 并确认已包含
的 Server main `6340df32`。配套 Client main `42e59b9e` 也已包含在其功能分支。
本批实现 Server 分块暂存、原始上传/发送所有权及投递历史；Client 的
send_file/get_file_delivery 接线和完整 A2 验收仍需完成。没有部署到生产。

## 对外行为

新增 device-only POST `/hagency/api/v1/execution/files/` 下的四个严格接口：

| 后缀 | 请求 | 行为 |
| --- | --- | --- |
| prepare | lease、dispatchId、executionId、requestId、file | 原 running 执行准备一次不可变文件声明；同键同声明返回历史，同键改声明拒绝。 |
| chunk | lease、target、offset、data | 按连续偏移写入1至65536字节；无 padding 标准 Base64；精确旧块可重读比较，改块、跳跃、重叠或超限拒绝。 |
| queue | lease、target | 实际完整长度和 SHA-256 必须匹配原声明，随后排队。重复调用返回原状态，不能重新激活 unknown。 |
| status | target | 当前同主人设备读取原 dispatch/execution/file 的安全历史；不要求旧租约仍有效，不提供新的发送权限或进行 Matrix IO。 |

file 只接受 filename、caption、bytes、sha256。文件上限8MiB，展示文件名255个
UTF-8字节，caption1000个UTF-8字节；文件名不能包含控制字符、路径分隔符或
点目录。manifestDigest 为紧凑 UTF-8 JSON 数组
`[filename,caption,bytes,sha256]` 的 SHA-256。bytes 可以为0，但必须匹配空文件摘要。
没有 Room、URL、sender 或本地路径选择参数。

每个主人保留最多4096条投递见证、128MiB未结内容预留。sent/cancelled 清除 blob，
保留身份、声明、摘要和历史。unknown 保留见证及预留，不能自动清理成未发送。
通用 HTTP 请求131072字节上限未扩大。公开 Delivery 不含 MXC、blob 或 worker token。
`agent-file-delivery-v1` 仅在 transport 与实际 MatrixClient 同时配置时公布；它描述
Server 协议，不证明 Client 工具、加密 Room 或端到端产品资格已经可用。

## 持久化和权限

file_outbox 与原 owner_event/execution/binding generation/设备 epoch 关联，保留
原主人、Agent puppet、Room、请求者及 thread。数据库触发器禁止删除、身份/声明
替换、倒退状态及变更已知 media/event 身份；分块只能增加原字节前缀。

prepare/chunk/queue 均检查原运行执行、当前设备租约和 Gateway 权限。worker 在上传
和发送两个阶段分别取得新 Gateway 事实，检查当前 owner/requester/puppet、项目/Room、
绑定代数、执行设备、设备 generation、session、有效期及原请求 TTL。锁定真实
设备/session/user 行，并在锁等待和完整字节哈希之后重新检查数据库实际时间与事实
新鲜度。已观测到请求者退出封存原请求，后来重新加入也不能重新放行。

上传前先提交 uploading 和原随机 worker token，消息发送前另提交 sending 与新 token。
每次所有权固定60秒；上传、发送及过期处理是独立 worker 循环。只有首次状态转换能
取得不可克隆的内存所有权。上传使用既有固定 Matrix origin/无重定向/无代理客户端，
AS bearer 和原 puppet，30秒期限、8MiB请求、32KiB响应上限。发送使用原 Room/thread、
Agent 身份和固定 `hagency-file-{file_id}` 事务编号；不带其他 Agent 提及，不把文件
投递当成原任务完成。

uploading/sending 失败、丢失响应、取消或超时转 unknown，worker 不重新认领或重发。
延迟上传成功只能登记历史 MXC，unknown 不变，不能由此重新发消息。原发送的实际
HTTP event_id，或经过 HS bearer 验证的 Appservice 原发送者/Room/完整 content 事件，
可以将原 sending/unknown 确认为 sent。撤权后仍可记录真实历史回执，但不能生成新 IO。
错误发送者、Room、内容或不同回执不能改写原投递；已发送事件不进入新模型队列。

当前 Server 路径只允许明文 Room。即使已上传，后续观测到加密或撤权也不发送。
A4 必须另接完整 Client crypto，不允许降级为明文。已撤权且未开始的暂存/排队/已上传
项取消并清理 blob，已可能发生的上传/发送保持未知，直到原操作真实证据出现。

## 验证结果

独立 PostgreSQL18 临时实例，新数据库、严格串行，完整 Agent-service **103通过、
0失败、0忽略**。包含7个新PG文件回归及1个纯声明回归：原声明/字节身份、精确块重试、
不同设备/主人拒绝、摘要不符、空文件、原所有权重开拒绝、固定截止时间、迟到上传不
重新发送、原可信事件确认、设备撤销锁竞争、上传后撤权/加密禁止发送和原请求保留。

实际 Salvo 路由、真实本地 HTTP MatrixClient 和 AS router 联测检查原5字节上传、AS
认证、puppet、文件名/caption/size、原 thread、固定 txn、一次上传/一次消息，以及
HTTP500丢失发送确认后不重发、原 Appservice 事件恢复历史 sent。测试使用合成身份和
本机端点，没有生产 Matrix 消息。它不是实际 homeserver、真实模型、Client send_file
审批、人工桌面或跨平台联合验收。

OpenAPI **82 owner + 4 AS**、19个负向守卫通过。严格 Clippy/格式最终结果及远端CI
记录在本文件末尾。原始日志：

- `/tmp/hagency-send-server-full.log`
- `/tmp/hagency-send-server-pg-races.log`
- `/tmp/hagency-send-server-openapi.log`
- `/tmp/hagency-send-server-clippy.log`
- `/tmp/hagency-send-server-fmt-check.log`

首轮代码检查因 sha2 0.11 的摘要不实现 LowerHex 失败，已使用实际摘要的 hex 编码，
日志 `/tmp/hagency-send-server-check.log` 保留。首次混用已有测试数据库造成其他 fixture
排队项进入本次 worker，暴露了测试 Gateway 过度假定所有 Room 属于当前 fixture；
现对其他 Room 返回不可用，并按既定每轮新库执行，保留原错误。另一个失败把调用前
时间加60秒视为提交后的截止时间，实际事务内时钟更晚；现读取本次实际持久截止时间，
在该原截止点验证过期，没有放宽生产60秒规则。原日志是
`/tmp/hagency-send-server-pg-http.log`，修正后为 `pg-http-fixed.log`。

## 尚未完成

Client 仍需冻结真实工作目录字节、精确 owner 审批、本地一次性发送登记、分块上传接线、
send_file/get_file_delivery、原审计和有界状态观察；未知不能从重启/快照恢复中重新获得
发送所有权。之后还需真实 Server/Client/Matrix/模型联合、人工原生和平台验收。
A2 未完成，A4 加密执行/远程审批、A3 协作及前阶段剩余资格仍在总目标范围内。

## 本批最终检查

新增 capability 正反检查后，最终新数据库完整103项再次通过（0失败/0忽略），
日志 `/tmp/hagency-send-server-full-final.log`。Server、Agent-service、xtask 三包
all-targets no-deps 严格 Clippy 通过，日志
`/tmp/hagency-send-server-clippy-final.log`。全工作区格式、OpenAPI/19负向守卫及
`git diff --check` 通过。远端 CI 将由更新后的 PR7 运行，结果尚未在这里声明。

首个远端 CI40389695253 在拼写检查拒绝了测试里的三字节文件前缀字面量，
未进入编译/测试阶段。现改为同一个完整原文件的 `[..3]` 切片，字节及断言
保持原样；未添加词典豁免。原输出保留在
`/tmp/hagency-send-server-ci-4f2f466.log`，后续提交重新验证。
