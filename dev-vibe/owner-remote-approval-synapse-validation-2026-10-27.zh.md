# 私密远程审批的真实联合资格支持

仅扩展现有agent_file_synapse开发示例和隔离Synapse脚本，生产router、身份协议和
数据schema无修改。脚本显式--retain-owner-password可在新私有fixture内保留该test
主人的密码供第二台SDK登录；默认不保留。示例OAuth introspection桥不再只比较
单个bootstrap token，而是逐token调用真实loopback Synapse whoami，要求实际Alice
及非空设备ID。实际两台主人SDK token均通过，Bob、无效token和错误secret均拒绝。
这仍然是明确的OAuth桥，不是生产Pasion OAuth验收。

Client新增native_owner_remote_approvals_qualification例程复用正式桌面SDK传输，
真实双SDK/双NativeOwner经加密原请求和决定，放行原待批Codex命令；31字节结果、
verified_matrix_device completed审计、whole_tree停止及一调用10912Token结算通过。
验后查询当前live lease0，原历史epoch行1保留。任务仍在普通Room；加密Agent身份/
任务/文件、人工UI和生产OAuth仍待完成。详细Client报告及代码见
[Client PR12](https://github.com/hagency-org/hagency-client/pull/12)。

成功私有证据为/private/tmp/hagency-remote-approvals-synapse-20261027-4/joint4；
前三轮独立失败原样保留。joint1/2无模型调用；joint3无工具调用但实际正常回复并
结算10967Token，不能说成远程审批通过，也没有重发它。相关资格日志前缀为
/tmp/hagency-remote-synapse。当前示例严格Clippy、Python语法和实际桥正反检查通过；
本批未重复跑全Server110项套件。所有本轮fixture服务已停止，数据留存。
