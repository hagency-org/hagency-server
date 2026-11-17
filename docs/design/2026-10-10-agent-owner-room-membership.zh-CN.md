# 个人 Agent 群成员授权改进任务报告

个人 Agent 的群服务由所有者授权，所有者必须已经加入该群。第三方 Matrix 邀请只是一项邀请，不能创建服务绑定、改变所有权或启动服务。所有者退群、被踢或失去所属 Space 的有效成员资格后，该群绑定撤销并自动退群。恢复成员资格不能自动恢复旧任务，必须由所有者重新绑定。

## 当前实现与缺口

当前生产入口是 Server 的 `crates/agent-service` 和 Client 的 OwnerHost。历史 Client ADR-188 的自动接受邀请流程不在当前可执行产品调用链中。本任务不重新启用该流程。

已有控制包括认证所有者访问、精确 MXID 对比、绑定前所有者 JOIN 检查、Space 与 Room 策略检查、服务账号的 Matrix 邀请权限、绑定 generation，以及工具执行和回复发送前的授权检查。普通群成员和群管理员不能替代 Agent 所有者调用绑定接口。服务账号代所有者执行 Matrix 邀请和 JOIN 是已授权动作的实现，不要求人类手动发送每一个协议请求。

成员资格丢失目前只把 active 绑定变成 suspended。后台扫描不检查 suspended；joining 绑定遇到所有者离群只停止加入重试，却不清理已经发生的加入。这可能使 Agent 独自在群里残留，也允许普通 resume 恢复曾失效的绑定。

## 行为与实施细节

| 场景 | 结果 |
| --- | --- |
| 他人邀请 Agent，尚无所有者绑定 | 不加入，不提供服务 |
| 所有者只收到群邀请，尚未 JOIN | 拒绝绑定 |
| 所有者已 JOIN 并通过策略检查 | 持久化 joining，后台执行加入，确认后 active |
| joining 期间所有者离群 | 先变成 leaving 并递增 generation，停止加入，清理可能已加入的身份 |
| active 或 suspended 期间所有者离群 | 先变成 leaving 并递增 generation，撤销工作权限，再退群 |
| Agent 被移除 | active 或 suspended 转 leaving，确认不在群后 left；不自动重新加入 |
| Matrix 查询失败或超时 | 不推断成员缺席，不授予新权限，保留状态重试 |
| 退群响应失败或不确定 | leaving 持久化保留，重新观察和重试，不能恢复服务 |
| 所有者回群 | leaving 不允许 resume/rebind；确认 left 后使用新幂等键重新绑定 |
| 旧任务或旧回复 | generation 不匹配即拒绝；排队任务失效，已开始任务保留 unknown，不假报成功 |
| 同一 Agent 的其他有效群 | 继续服务，撤销仅限发生变化的绑定 |

使用现有 leaving/left 状态和 generation，无需迁移数据库。所有者私聊也遵守成员与隐私约束；离开后允许所有者明确重新采用原来的私聊，保持永久 Room 绑定，不改成 Project，也不能迁移到另一私聊。保留管理员暂停标记，避免重新绑定绕过管理员限制。joining 状态下 Agent 尚未 JOIN 是正常过渡，不能当作被踢；但完成 JOIN 后的最终激活检查必须要求 Agent 仍为 JOIN，缺失就撤销，不能自动重试加入。所有者、项目或 Room 的授权缺失则无论 joining/active/suspended 都撤销。工具授权触发的撤销必须独立提交，避免后续拒绝错误回滚该撤销。

后台扫描纳入 suspended。joining 完成前的最终验证再次独立提交成员资格丢失，避免外部 JOIN 已发生但最后检查失败后遗留。实际退群只有在确认当前 generation 和终止状态后执行；最终 left 必须来自精确 Puppet 的新鲜 Matrix 状态。进群期间的人类退群与远端 JOIN 无法原子化，允许短暂物理残留，但不能激活服务，后台负责收敛。

## 完整性复核与测试计划

覆盖所有者未加入、第三方访问、joining/active/suspended 成员丢失、Agent 被踢、暂停后离群、generation 过期、恢复成员后拒绝 resume、明确退群后重新绑定、其他群隔离、已执行任务与排队任务失效。增加真实 PostgreSQL 状态机回归和后台控制器通过本地 HTTP Matrix 测试端点的自动退群验证，包括退群失败重试。

执行 Agent service 全部测试（包含 requires PostgreSQL 的 ignored 用例）、该 crate Clippy，以及 git diff --check。Server 无 specs/project.spec.md 或 provisioned task-writer；当前环境 PATH 无 agent-spec，不宣称执行其 lifecycle。所有测试使用独立临时数据库，不访问应用数据库，不调用真实模型。

## 时序边界

“立即停止”指观测到成员资格丢失后先撤销权限。后台扫描存在检测延迟，授权检查也依赖新鲜 Matrix 快照；远端 Matrix 状态和本地数据库没有跨系统事务。已开始的模型计算与已被外部系统接受的工具效果不能被数据库撤销追回；客户端既有 monitor_running 每 5 秒检查当前执行授权，失败后终止 provider；工具执行前也逐次授权。执行屏障、后续工具授权和回复授权负责阻止继续使用失效群权限，历史结果保留原有 unknown 语义。独立服务机器人模式不属于本任务。

## 实施与代码复核结果

实现已完成，代码修改限于 `crates/agent-service/src/domain.rs`、`workers.rs`、`owner_direct.rs`，以及 transport 的共同撤销调用和对应回归测试。无需数据库迁移或客户端协议升级。

代码复核修复了三个关联缺口：暂停绑定未被后台检查；实际 JOIN 后所有者离群或 Puppet 被移除时，最终激活拒绝不能回滚撤销；退群清理在 Matrix GET 之后必须再次检查 generation。私聊绑定增加确认 left 后的明确重新采用路径，避免新的安全撤销使原私聊永久不可用。个人 Agent 在其他有效群的授权不被撤销。

新增两项 PostgreSQL 回归，分别验证 joining、暂停和最终激活阶段的撤销，以及实际后台控制器在本地 HTTP Matrix 测试端点上的退群与失败重试。后者同时验证正常群不受影响、查询失败不被误判为成员离开、被踢的 Agent 不自动 JOIN。既有回归增加排队任务 cancelled、运行任务 unknown、旧版本执行拒绝、确认离开后重新绑定和私聊重新授权检查。普通服务暂停仍然允许 resume，不能与成员资格撤销混为一谈。

最终验证：`python3 scripts/test-agent-service-postgres.py` 执行 57 项测试，57 通过、0 失败、0 忽略；独立数据库已删除。`cargo clippy -p hagency-agent-service --offline --all-targets -- -D warnings` 和 `git diff --check` 通过。首轮旧恢复预期及最后激活保护对应测试曾失败，已依据新状态契约修正并重新执行整个测试集，没有把失败标成通过。

复核未发现本次修改仍需修复的问题。当前交付是源码和本地回归验证，尚未部署到运行中的服务，也未执行真实 Matrix 服务与真实模型的端到端验证。后台只依据观测到的新鲜成员状态撤销；两个观测之间发生又恢复的短暂退群不等同于已被系统观测并撤销，这仍是周期检查的时序边界。


## 2026-10-31：原始会话与额外项目房间

原始 owner-direct / pool 会话允许邀请成员，不因第三人加入而撤销绑定。
新消息以实时 Matrix 成员状态判断是否需要 @；群聊中包括主人在内的每条请求
必须显式提及目标 Agent。旧 thread_auto_reply 标志与已知线程均不能绕过此规则。
加密确认分开传递 addressed 与 mentioned，服务器再次检查真实成员。

资源池 Agent 保留不可变的申请房间和预算预留。额外项目房间记录在
shared_agent_rooms，绑定仅由 Agent 主人创建，需要主人加入空间和房间，
并遵守项目/房间接入策略。每个绑定有独立 generation；执行工作携带
roomBinding，原有工作省略该字段以保留原规范字节。接收、开始执行和发送
回复均验证实际工作房间及当前绑定。解除绑定后，旧队列及加密候选失效，
不影响原会话；项目或房间停用、管理员暂停同样停止额外绑定。

GET /hagency/api/v1/room-agents 提供当前主人可添加的本地及资源池身份。
个人执行协议仍只接受 personal Agent；额外资源池绑定使用独立命令记录和
成员协调器，未扩大个人 Agent 的执行授权。客户端与服务器须配套更新。
