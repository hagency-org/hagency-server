# GitFab 验收脚本

对应附件 3 第十节的参考入口。共同约定：

- 退出码：0 表示全部必需检查通过；1 表示有检查失败；2 表示用法或环境错误；3 表示该入口尚未提供。
- 每次运行有一个 `acceptance_run_id`（`ar-` 开头）。若要把多个命令的结果放进同一个证据包，先 `export ACCEPTANCE_RUN_ID=ar-...`。
- 证据包在 `evidence/<acceptance_run_id>/`，每个命令写 `<命令>-report.json`，结束时重写 `manifest.json`（相对路径、大小、SHA-256）。证据包里出现凭据、私钥或本机绝对路径时，不生成 manifest，命令失败。
- 凭据只从配置里指定的文件读取（权限 0600），经标准输入交给 curl，不出现在命令行、输出和报告里。

## 准备

```sh
cargo build --release -p hagency-gitfab-events     # 生成 target/release/gitfab
cp acceptance/gitfab.env.example acceptance/gitfab.env   # 填写地址和凭据文件路径
# T0：在 acceptance/projects/DEMO-0N.env 填写仓库和基线 SHA；把公钥放进 acceptance/keys/<key-id>.pub
```

## 入口

| 命令 | 状态 | 作用 |
|---|---|---|
| `preflight.sh [DEMO-0N]` | 可用 | 无副作用探测：Runner 环境、工具链、Matrix、Hagency、公钥、Registry、各项目仓库与基线 SHA、PRD 摘要；写 `preflight.json` |
| `verify-events.sh [FILE...]` | 可用 | 校验全部事件：schema、Ed25519 签名、关键事件的身份事件；默认读 `GITFAB_JOURNAL` |
| `verify-ledger.sh DEMO-05 [FILE...]` | 可用 | 导出该项目的 memory/token 事件到证据包，验签并复算账本 |
| `run-project.sh DEMO-0N --backend ...` | 可用：以验收人身份在项目房间发出请求（作为任务 Thread 的根），发起流程并跟踪到结束；Spec 由人在 Thread 里确认，或设 `GITFAB_AUTO_APPROVE_SPEC=1` 由脚本确认；随后在同一证据包里执行 verify-events 和 verify-release | 0 通过；1 流程失败、转人工或核对不通过；2 配置不全 |
| `verify-release.sh DEMO-0N` | 可用：验签；找到项目最近的 Release 事件；只读核对平台上的 Release 和 PR 测试；核对 Runner 部署回执（验签、健康检查通过、提交与 PR 头一致、SBOM 摘要一致）；核对容器正在运行回执中的镜像、健康和版本接口当前可用 | 0 全部通过；1 有核对不通过；2 配置不全 |
| `rollback.sh DEMO-0N` | 可用：调用 `gitfab-runner rollback` 回到上一个正常的 Release，核对新回执验签通过、结果为通过、容器运行目标镜像、健康和版本接口返回目标 Release | 0 通过；1 回退或核对失败；2 配置不全 |

## 自检

```sh
./acceptance/tests/selftest.sh
```

用临时密钥和事件检查这些命令本身：正确的事件通过，被改动的事件失败，未实现的入口返回 3，每个证据包都有 manifest 且不含本机路径。
