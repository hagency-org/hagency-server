# GitFab 提示词配置指南

按合同第五章，本指南只覆盖软件工坊的四类提示词：PRD、Spec、任务拆解（Plan）和 Reviewer。执行（Execute）、测试（Test）、发布（Release）阶段的指令写在同一处代码里，这里只列出原文，便于对照，不作为本指南的配置对象。

## 提示词在哪里、怎么生效

- 每个阶段发给 Agent 的消息由编排器生成：`crates/agent-service/src/orchestrator.rs` 的 `instruction()`（各阶段要求）和 `stage_message()`（消息结构）。
- 消息以 `@_hagency_service` 的身份发到任务 Thread，类型为 `m.notice`，不提及任何人；被派发的 Agent 只看到这一条消息，所以阶段之间需要的上下文都写在消息里。
- 修改提示词需要改上述代码并重新部署服务端。每条阶段消息的 SHA-256 记入该阶段签名事件的 `input_digest`，因此任何一次运行用的是哪一版提示词，都能从事件日志追溯；改动后摘要随之变化。
- 改动提示词要同时更新本指南，并在验收记录里写明版本（提交 SHA）。

## 阶段消息的结构

```
[GitFab <阶段>] 任务 <任务 ID> · 第 <轮次> 轮 · 第 <次数> 次
目标：<goal>
仓库：<repoRef>（<gitBackend>，基线 <baselineSha>）
任务分支：agent/<project_id>/<task_id>
Spec：<specRef>（第 <Spec 版本> 版）

本阶段要求：<该阶段的提示词>

上一阶段产出：
<上一阶段 Agent 的回复，最多 32 KiB>
```

- 字段来自发起流程时的 `context`（房间命令 `/gitfab start` 或 API）。
- “上一阶段产出”把前一阶段的回复原样带给下一阶段；评审驳回后的返工轮次，带的是 Reviewer 的意见。

## PRD（需求输入）

PRD 不是发给 Agent 的提示词，而是 `specRef` 指向的冻结文件，Spec 阶段以它为依据。五个演示项目的 PRD 草案在 `acceptance/fixtures/DEMO-0N/PRD-DEMO-0N-v1.md`，统一结构：

| 章节 | 写什么 |
|---|---|
| 目标 | 一句话说明交付什么 |
| 运行 | 启动方式、端口、配置项、`/healthz` 与 `/version` |
| 数据 | 数据结构与字段约束 |
| 接口 | 每个端点的方法、路径、请求、响应、错误码 |
| 测试 | 冻结的测试命令与夹具 |
| 不做 | 明确排除的范围 |
| 验收 | 可机器判断的验收条件 |

要求：
- 写可验证的条件，避免“友好”“合理”这类无法判断的词；
- 有意留下的歧义（例如 DEMO-02 的标题长度上限）要在内部记录中注明，以便在 Spec 确认环节演示人工批改；
- 冻结后记录 SHA-256（`acceptance/projects/DEMO-0N.env` 的 `PRD_SHA256`），preflight 会比对。

## Spec 提示词

原文：

> 根据目标起草本任务的 Spec：范围、验收标准、不做的事、风险。验收标准单独成节，标题含“验收标准”，下面逐条列出（每条以 -、* 或序号开头）。只输出 Spec 正文，等待人工确认。没有验收标准的 Spec 不算完成。

配置要点：
- Spec 必须包含验收标准，这是后续 Test 与 Review 的依据。读取是严格的（`orchestrator_flow.rs` 的 `acceptance_criteria`）：要有一行写明“验收标准”（或 Acceptance criteria），其后、下一个标题之前至少一条列表项。没有的 Spec 回复按失败处理，自动重派一次，仍没有则运行失败。
- Spec 的版本与摘要：Spec 出稿事件（spec / blocked）的 `output_digest` 是 Spec 正文的 SHA-256；此后每个阶段事件都带 `spec_revision`，写明它依据的是第几版 Spec（`revise` 一次加一）。仓库里的 Spec 文件（`specRef` 去掉 `@版本`，例如 `spec/DEMO-02.md`）在 Execute、Test、Release 时读出 Git blob 记入事件 `spec_blob`；确认后若有人改了这个文件，verify-release 的 `spec.unchanged` 会失败并列出前后 blob。
- “等待人工确认”对应编排器的闸门：Spec 回复后流程停在 `awaiting_spec_approval`，只有发起人在 Thread 里用 `/gitfab approve|revise|reject`（或 API）决定后才继续，Agent 无法绕过。
- 选择 `revise` 会重新派发 Spec 阶段，Spec 版本号加一；上一版 Spec 作为“上一阶段产出”带入。需要的修改意见请直接在任务 Thread 里写给 Agent。

## 任务拆解（Plan）提示词

原文：

> 根据已确认的 Spec 给出实施计划：步骤、涉及文件、测试方法。并把任务拆成 Module，在回复中用一个 ```gitfab-modules 代码块给出 JSON 数组，每项含 id（字母数字、-、_）、title、spec（该 Module 的范围与验收要求）、tests（测试命令或用例，至少一项）、capabilities（所需能力，至少一项），最多 16 项。缺少或格式不对的计划不算完成。

Module 代码块示例：

````text
```gitfab-modules
[{"id":"M1","title":"任务存储","spec":"内存存储；ID 递增且删除后不复用","tests":["cargo test --locked store"],"capabilities":["rust"]},
 {"id":"M2","title":"HTTP 接口","spec":"六个接口的状态码与错误体按 Spec","tests":["acceptance/fixtures/DEMO-02/cases.json"],"capabilities":["rust","http"]}]
```
````

配置要点：
- Module 清单的读取是严格的（`orchestrator_modules.rs`）：只认一个代码块，字段不能多也不能少，id 不能重复，spec、tests、capabilities 不能为空。没有合格清单的 Plan 回复按失败处理，自动重派一次，仍不合格则整个运行失败。
- 合格的清单作为任务卡写入 Plan 成功事件的 `modules` 字段（每项含 id、title、spec、tests、capabilities），并随 Plan 全文带入 Execute 阶段。
- Plan 由协调角色（coordinator）完成，Execute 由实现角色（implementer）完成，派发规则固定，见 `orchestrator_flow.rs` 的 `Stage::role()`。
- 计划里的“测试方法”应指向 PRD 冻结的测试命令，便于 Test 阶段直接执行。

## Reviewer 提示词

原文：

> 评审本轮实现和测试结果是否满足 Spec。逐条对照 Spec 的验收标准，写明是否满足以及证据（测试输出、提交 SHA 或改动位置）；再逐条列出问题；最后单独一行写结论：VERDICT: approve、VERDICT: reject 或 VERDICT: blocked。

配置要点：
- 结论行的读取是严格的：必须单独一行，写成 `VERDICT: approve`、`VERDICT: reject` 或 `VERDICT: blocked`。没有结论行的回复不算通过（`orchestrator_flow.rs` 的 `verdict_lines_are_read_strictly`）。
- `reject` 触发自动返工，最多 2 次；第 3 次仍被驳回转人工介入，保留每轮轮次与意见。`blocked` 直接转人工介入。
- Reviewer 的回复（前 4 KiB）作为 `findings` 写入签名事件，评审结论可审计。
- 修改提示词时，不要改变结论行的格式；如需新增结论种类，需同时改编排器的解析与流程规则。

## 其他阶段（仅列原文，不在本指南配置范围）

- Execute：按计划在仓库中实现，提交到任务分支。回复中列出提交 SHA、改动文件和自测结果。若上一轮评审驳回，逐条处理其中的问题。
- Test：运行 PRD 冻结的测试命令，逐条回复：命令原文、退出码、通过与失败数、失败原因、日志摘要（末尾若干行）和日志位置、产出的工件。
- Release：按发布流程打标签并发布，回复标签、提交 SHA、制品摘要和回退方式。配置了 Git 平台 Token 时，PR 与 Release 由编排器在平台上创建，链接写入签名事件。
