# 真实 Synapse 文件链与 v12 权限兼容

当前Server通过实际Synapse状态、媒体和Appservice连接Client桌面SDK及实际Codex。
隔离OAuth introspection桥仍不是生产Pasion登录验收，没有生产部署。

## 产品修正

依据[官方v12规范](https://spec.matrix.org/v1.17/rooms/v12/)，Room ID不再带服务器后缀，
创建者及additional_creators有独立于power_levels.users的权限。Domain接受43字符
URL-safe Room hash，用户ID仍需服务器名。Gateway只从当前可信create状态计算v12
创建者权限；旧版本additional_creators不能提权，坏的创建者/power字段仍拒绝，
成员资格独立核验。原身份、撤权、未知不重发和最终缺席规则保持。

## 实际原结果

agent_file_synapse使用真实Verifier HTTP、当前Agent/Binding生命周期、PG/API和
投递worker。Gateway读取真实Room状态，AS用户查询只确认已知puppet，readiness
必须收到原HTTP回调。MatrixClient实际上传131089字节，以原Agent发到原Room/thread。

Client桌面SDK下载131073字节原附件，真实Codex处理并发送，原请求completed。
四项审批completed，输入38892加输出418为39310 Token，缓存24832是输入子集，
预留/pending/unreported均0，原停止whole_tree。原输出事件为
$We9ZuTMHzALjOa0_8EpAfTRBkbMBay2hJGgNTx1eTKI，读回字节逐字节一致。

默认驱动在发送后用旧fixture token读回，被真实HS拒绝401；保留该失败。改用当前
SDK token后的显式VERIFY_EXISTING只核验同一原字节/回执/审计/费用及checked
shutdown，没有新增推理、acquire、上传或重发。结果verifiedExisting=true，不宣称
首轮默认驱动成功。私有原结果在
/private/tmp/hagency-files-synapse-20261027-1/real-codex-files-result.json；
原失败日志/tmp/hagency-synapse-real3.log，核验日志/tmp/hagency-synapse-real3-verify.log。

## 检查与隔离复现

完整110项Server测试（含全部PG场景）通过。两个v12回归先失败再修复；严格
all-targets Clippy、格式、拼写和OpenAPI83个owner/4个AS路由、22项负向守卫通过。

```sh
cargo +1.99.0 build -p hagency-agent-service --example agent_file_synapse
python3 scripts/agent-files-synapse.py --root /private/tmp/hagency-files-new --port 55518 --agent-port 55519
```

脚本用官方Synapse1.163.2固定摘要，仅发布loopback端口，新私有目录及本地test
账户，不启用联邦。Docker host回调IPv6不可达时，--callback-host可传该实例内部
私有IPv4；本机实测192.168.65.254，不能当成其他机器的固定地址。

示例需要HAGENCY_AGENT_TEST_DATABASE_URL（新专用loopback数据库，库名前缀
agent_files_joint_）、HAGENCY_SYNAPSE_BOOTSTRAP、HAGENCY_FILE_FIXTURE_PORT和
HAGENCY_FILE_FIXTURE_INFO。先确认HS版本接口就绪，再启动当前Server示例及Client
资格入口。结束只停止本轮服务、PG及指定容器，保留原数据。不能把旧库和新Room
当成同一原请求。

原IPv6回调不可达、未等HS重启就绪、旧Client ID拒绝和用户附件正文误命中完成
标记的失败均保留；后者原请求pending且无模型调用，未重跑。驱动现绑定原sender、
Room及文本类型并有负向回归。没有放宽产品授权、截止时间或伪造ready。

完整过程及SDK命令见Client的同名验收文档。生产OAuth、桌面人工、Windows文件
工具、加密执行/远程审批、多Agent协作及前阶段剩余门槛仍待完成，总目标进行中。
