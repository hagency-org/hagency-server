# GitFab 用户操作手册与演示用例操作指南

面向四类人：部署服务端的运维、在房间里发起任务的 Human、提供 Agent 的 Agent 主人、执行验收的验收人。按本文顺序操作即可从零跑通一次演示。系统结构见 `architecture.zh-CN.md`，提示词见 `prompts.zh-CN.md`。

## 1. 服务端准备（运维）

1. 生成事件签名密钥（私钥 0600，只放服务器）：
   ```
   gitfab keygen --key-id gitfab-1 --dir /etc/hagency/keys
   ```
2. 在 hagency 配置里加 `[gitfab]` 段（需要已启用 Agent 服务 `pasion.delegate_matrix_auth`）：
   ```toml
   [gitfab]
   signing_key_file = "/etc/hagency/keys/gitfab-1.key"
   signing_key_id = "gitfab-1"
   journal_file = "data/gitfab/events.jsonl"   # 设置即启用编排器
   stage_timeout_minutes = 120                  # 阶段超时转人工
   github_token_file = "/etc/hagency/keys/github-token"   # 可选：启用 PR 与 Release
   gitcode_token_file = "/etc/hagency/keys/gitcode-token" # 可选
   board_token_file = "/etc/hagency/keys/board-token"     # 可选：大屏读取 Agent 状态
   ```
   所有 Token 文件必须是 0600，否则服务拒绝启动并说明原因。
3. 重启后检查：日志出现 `GitFab orchestrator enabled`；`GET /hagency/gitfab/v1/public-keys` 返回公钥。把公钥文件交给验收方，放进 `acceptance/keys/`。

Git 平台 Token 的最小权限：
- GitHub 细粒度 Token，只授权演示仓库：Contents、Pull requests、Commit statuses 读写，Actions、Metadata 只读；需要设置分支保护时再加 Administration。
- GitCode 经典 Token（不能限定单仓库）：Repositories、PR 读写，Users、Action 只读，其余禁止。

## 2. 准备项目房间（Human，Hagency Desktop）

1. 左侧“Agent 空间”里新建聊天室：在向导里填名称，**关闭“端到端加密”**（编排器只支持不加密的项目房间），创建。
2. “邀请成员”邀请参与的人。
3. “聊天信息 → 成员 → 添加 Agent”，把要参与的 Agent 加进房间。资源池 Agent 需先经资源 owner 批准。
4. 房间里只有一个 Agent 时，三个角色都由它担任；有多个时，发起时要分别指定。

## 3. 发起任务（Human）

在项目房间发一条消息（这条消息会成为任务 Thread 的根）：

```
/gitfab start DEMO-02-task-1
project: DEMO-02
repo: https://github.com/ZhangHanDong/gitfab-demo-02
baseline: 9865050f8628ef399cbab786d79026539830d3f2
spec: spec/DEMO-02.md@v1
coordinator: @协调Agent账号
implementer: @实现Agent账号
reviewer: @评审Agent账号
goal: 实现轻量任务清单 API，见 PRD-DEMO-02-v1。
```

规则：
- 第一行是 `/gitfab start <任务ID>`，任务 ID 只能是一个词。
- 之后每行 `名称: 值`（中英文冒号都可以）。必填：`repo`、`baseline`、`spec`、`goal`；`backend` 可从仓库地址推断（github.com / gitcode），推断不出时写 `backend: github` 或 `gitcode`。
- `goal:` 必须放在最后，它后面的所有行都算目标正文。
- 角色可写 Agent 账号、绑定 ID（`bnd_…`）或资源池分配 ID（`mas_…`）。
- 写错时编排器会在 Thread 里说明原因；发 `/gitfab help` 查看用法。

## 4. 流程中的操作（只有发起人能决定）

| 时机 | 在任务 Thread 里发 | 结果 |
|---|---|---|
| Spec 出稿后 | `/gitfab approve` | 进入 Plan |
| | `/gitfab revise` | 重新出 Spec，版本加一（修改意见直接写在 Thread 里） |
| | `/gitfab reject` | 流程结束（失败） |
| 转人工介入后（超时、评审驳回超过 2 次、评审 blocked） | `/gitfab continue` | 同一 Agent 重新执行该阶段（新的一次派发，不重放旧的） |
| | `/gitfab reassign @Agent账号` | 卡住的阶段改由本房间里指定的 Agent 接手（也可写绑定 ID 或资源池分配 ID）；该角色此后都由它担任 |
| | `/gitfab reassign` | 不指定时仍派给该角色原来的 Agent，记录里标明是改派 |
| | `/gitfab abort` | 流程结束（失败） |

Thread 里能看到：每个阶段的派发消息（含任务分支 `agent/<project>/<task>`）、Agent 的回复、编排器的平台说明（“已创建或更新 PR”“平台测试状态”“已发布 Release”）。

## 5. Agent 主人需要做的

- 保证执行设备在线，Agent 能读写演示仓库并推送 `agent/…` 分支（推送凭据在执行设备上，不交给 GitFab）。
- 不需要自己建 PR 或 Release：配置了平台 Token 时由编排器完成。

## 6. 部署与回退（Runner，Linux + Docker）

1. 生成 Runner 自己的密钥（与服务端密钥分开）：`gitfab keygen --key-id runner-1 --dir /etc/gitfab-runner`，公钥交给验收方。
2. 检出 Release 对应的提交后部署：
   ```
   gitfab-runner deploy --key /etc/gitfab-runner/runner-1.key --key-id runner-1 --runner-id runner-a \
     --receipts /var/lib/gitfab/receipts.jsonl --port 18080 --container-port 8080 \
     --project DEMO-02 --task DEMO-02-task-1 --backend github --repo https://github.com/ZhangHanDong/gitfab-demo-02 \
     --commit <PR 头提交> --tag <Release 标签> --context <检出目录> --sbom-dir /var/lib/gitfab/sbom
   ```
   退出码 0 表示健康与版本检查通过；1 表示新版本没起来，回执会写明是否已恢复上一版。
   加 `--registry <host[:port]/namespace>` 时，镜像先推送到该 Registry 再启动，回执的 `image_ref` 记为 `<registry>/<项目>@sha256:<manifest digest>`（Registry 返回的不可变摘要）；`image_digest` 仍是本机镜像 ID，供回退和容器核对使用。推送失败则不替换线上容器，部署失败。Registry 的登录凭据由 Runner 机器上的 `docker login` 管理，不经过 GitFab。
   再加 `--cosign-key <cosign.key>`（须同时有 `--registry`）时，推送后、启动前用 cosign 对该 digest 签名，签名存放在 Registry 中；签名失败则不替换线上容器。密钥对用 cosign 官方镜像生成（`docker run --rm -e COSIGN_PASSWORD -v <目录>:/k -w /k gcr.io/projectsigstore/cosign:v2.4.1 generate-key-pair`），私钥 0600 只留在 Runner，口令放在 Runner 进程的环境变量 `COSIGN_PASSWORD`，不写在命令行。公钥改名为 `cosign.pem` 交给验收方放进 `acceptance/keys/`（不能叫 `.pub`，那是 GitFab 自己的 Ed25519 公钥格式）；verify-release 用它离线核对签名，不依赖透明日志。
3. 回退：`gitfab-runner rollback`（参数同上，去掉 Release 相关项，保留 `--project`）。
4. 离线核对 SBOM：`gitfab verify-sbom --keys <公钥目录> /var/lib/gitfab/sbom/<文件>.spdx.json`。

## 7. 大屏

```
gitfab-board --keys <公钥目录> --journal <事件日志> --receipts /var/lib/gitfab/receipts.jsonl \
  --probe DEMO-02=18080 --server https://<hagency 服务器> --server-token-file <大屏 Token 文件>
```

用最新版 Chrome 打开 `http://127.0.0.1:8090/`。页面每 5 秒刷新，只读：概览、Agent 状态、任务进度、部署结果、Token 记录、签名与审计事件流。

## 8. 验收（验收人）

配置 `acceptance/gitfab.env`（照 `gitfab.env.example`）和 `acceptance/projects/DEMO-0N.env`，然后按顺序：

```
./acceptance/preflight.sh
./acceptance/run-project.sh DEMO-01 --backend gitcode
./acceptance/run-project.sh DEMO-02 --backend github
./acceptance/verify-events.sh
./acceptance/verify-ledger.sh DEMO-05
./acceptance/verify-release.sh DEMO-01
./acceptance/rollback.sh DEMO-01
```

每条命令把报告写进 `evidence/<acceptance_run_id>/`。设 `ACCEPTANCE_RUN_ID=ar-…` 可让多条命令共用一个证据包。退出码：0 通过，1 有核对不通过，2 配置不全。命令细节见 `acceptance/README.zh-CN.md`。

查看与追溯（都只读，只采用验签通过的记录）：

```
gitfab report --keys acceptance/keys --project DEMO-02 <事件日志> <部署回执>    # 一个项目做了什么（Markdown）
gitfab trace  --keys acceptance/keys --id <任务/运行/提交/Release/镜像 digest> <文件…>
gitfab trace  --keys acceptance/keys --text spec.md <事件日志>                  # 这段文本是否为某个签名阶段的原文
```

验收结束后归档证据包（保留至质保期满后 30 日）：

```
./acceptance/archive.sh <acceptance_run_id> --retain-until <质保期满后 30 日，YYYY-MM-DD>
./acceptance/archive.sh --check evidence/archive/<acceptance_run_id>.tar.gz   # 日后核对归档未被改动
```

报告里每个 Spec 版本和每次评审回复都标出输出摘要，可以集中查看 Spec 的历史版本。`trace --text` 用文件的 SHA-256 查找输入或输出正是这段文本的签名阶段：原样保存的 Spec 或评审回复能找到对应事件；改动过的文本对不上任何签名记录，返回 1。

## 9. 演示用例

| 项目 | 后端 | 测试仓库 | 演示重点 |
|---|---|---|---|
| DEMO-01 健康状态与版本服务 | GitCode | alexzhang1/gitfab-demo-01 | 主流程冒烟，现场演示首选；部署与回退 |
| DEMO-02 轻量任务清单 API | GitHub | ZhangHanDong/gitfab-demo-02 | 完整 PR 流程；Spec 人工批改 |
| DEMO-03 Markdown 规格检查服务 | GitCode | 待建 | Reviewer 驳回一次后返工通过 |
| DEMO-04 Release Notes 生成服务 | GitHub | 待建 | 冻结夹具、确定性输出 |
| DEMO-05 Open Token 账本校验器 | 待定 | 待建 | 账本去重、冲突拒绝、`verify-ledger.sh` |

PRD 草案在 `acceptance/fixtures/DEMO-0N/`，冻结前需双方书面确认。

## 10. 常见问题

- **发起后提示“本房间有 N 个 Agent，请用 `角色: @Agent账号` 指定”**：房间里不止一个 Agent，按提示写出三个角色。
- **提示角色“不是本房间里正在服务的 Agent”**：该 Agent 不在房间，或绑定未启用、资源池分配未批准。
- **Spec 一直等待**：只有发起人发的 `/gitfab approve|revise|reject` 有效。
- **服务启动失败，提示 must be mode 0600**：把对应 Token 文件改为 `chmod 600`。
- **PR 或 Release 没出现**：检查是否配置了该后端的 Token；平台调用失败时 Thread 里会有“Git 平台操作未完成”的说明，流程不会因此卡住。
- **房间是加密的**：编排器不处理加密房间，请新建不加密的项目房间。
