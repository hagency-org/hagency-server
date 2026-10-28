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

## 后续修正：任务完成后仍待投递的文件必须保留撤权证据

发送宿主接线审查发现：owner_events已completed时，原reject_event_delivery不会
改写已完成结果；单靠它封存文件，无法保留请求者退出、puppet失去发送权限或Room
加密的否定观察。恢复正向事实后，旧queued/uploaded文件可能再取得发送权。
新回归在未修复版本明确失败，日志 `/tmp/hagency-send-completed-before.log`。

新增不可更新/删除的file_delivery_denials，按原owner_event登记否定证据。共享可信
权限观察为同binding generation下、对应请求者或整binding的旧活跃请求/待投递文件
留证；即使尚未准备文件也不能稍后给该旧请求创建发送意图。新原始请求保持独立。
prepare/chunk/queue/claim和sendable view检查该证据；原completed结果和旧文本回复
语义不变。否定后已可能发生的原发送仍可接受真实晚到回执，不能反推远端不存在。

迁移仅添加日志表/禁止改写触发器及view，复用已有文本delivery_blocked的历史证据。
在真正03c9e0b格式的旧独立测试库执行（迁移前file_outbox存在、denial表不存在），
9条文件、199条原请求和14条回复的完整数据摘要前后不变；重复初始化也不改数据。
日志 `/tmp/hagency-send-migration-{original-schema,original-counts,preservation}.log`。

最终新库串行Agent-service106/106通过、0忽略，含完成后queued/uploaded的三种
否定/恢复、准备前共享否定、新请求独立、迟到真实回执及不可删除见证。三包严格
Clippy、全仓格式、typos1.48、OpenAPI82+4和19个负向守卫通过。日志为
`/tmp/hagency-send-server-revocation-{final,clippy-final,fmt-check,spelling,openapi}.log`。
中间全量105通过/1失败保留在revocation-full.log：永久拒绝覆盖了原明确的加密拒绝
错误码。现先返回当前加密事实的原拒绝，再检查永久日志；原断言未改，记录仍永久。
本修正的远端CI将在推送后另跑，03c9e0b的旧绿灯不代表新提交已完成远端验证。

## 历史查询协议基础：丢失prepare响应也能查原意图

新增`agent-file-history-v1`和设备专用只读`POST execution/files/lookup`，请求仅含
原dispatchId/executionId/requestId。无需原lease、正向Matrix事实或重新启动模型；
响应严格绑定原请求，并只含安全delivery和preparationClosed，不含文件字节/MXC/
worker令牌。没有记录且preparationClosed=false仅为暂时缺失，不能当成取消。

查询与prepare、终态转换共用原Transport事务锁，并在锁内核验当前会话、设备及
原始执行的永久owner/执行编号；只有unknown/completed/cancelled这三个不可回到
running的状态会给出preparationClosed=true。即使关闭，已有delivery仍按原状态
观察，不能将未知发送改写为取消。若关闭且无记录，后续原prepare必被拒绝，因而
形成最终未准备证据；这不会恢复任何写权，也不会在Server伪造文件回执。

真正PG覆盖“无准备/丢失准备响应”两种顺序、运行中暂时缺失、同owner另一设备、
不同owner/执行拒绝、结束后只读查询、重开保留和未来prepare拒绝。真实Salvo路径
覆盖普通用户token拒绝、禁止夹带lease及原请求精确查回。完整新库107/107通过，
0忽略；严格三包Clippy、全仓格式、拼写、OpenAPI83+4及22项负向守卫通过。
日志`/tmp/hagency-file-lookup-{first,full,server-clippy-final,openapi,server-spelling}.log`。
启动fixture时一次遗漏自定义PG端口，已在同一专用实例恢复55437；测试本身通过。
首个收尾脚本错误使用zsh只读status变量，另一次Clippy错用不存在包名xtask，均未
修改生产断言；更正为任务专用变量及hagency-xtask后完成检查，原日志保留。

Client已提供相同严格类型/只读设备操作，主人历史界面及最终未准备证据的本地
容量结清尚未接通，A2仍在进行。f5ac86c的CI40404302743已通过；本节新提交另行CI。
