# Hagency 前端

[English](README.md) · [项目快速配置](../../README.zh-CN.md)

使用 Dioxus 0.7.5/WASM，将 Padmin 与 Hagency 页面整合，由 Rust 后端托管。

在仓库根目录构建或启动开发监听：

```sh
just prepare-frontend
just dev
```

生成的资源位于 `resources/frontend/public`，不会提交到 Git。
配置见[开发指南](../../docs/guide.zh-CN.md#开发环境)，
上游来源和许可证信息见根目录 [NOTICE](../../NOTICE)。

## 新 Agent 管理界面

普通用户以自己的 Matrix 账号通过 Pasion 登录，进入 `/hagency/projects` 或
`/hagency/agents`。Palpo/Pasion 通用管理页面保留原有管理员权限检查。

通用网页会话使用 `/hagency/api/login/token`、`/hagency/api/session`、`/hagency/api/logout`。
新 Agent 管理使用 `/hagency/api/browser/v1` 的封闭 cookie/CSRF BFF；服务器保存的
个人 OAuth 授权经新服务再次验证后才发起操作，不向网页暴露原生授权凭据，也不开放
device/execution 操作。原生 `/hagency/api/v1` 继续拒绝 browser Cookie/Origin。

Project 页面登记已有 Matrix Space/Room，编辑默认创建许可和 allow/deny 清单，采用
revision 防止并发覆盖；服务器检查真实 Space 管理权。Agent 页面创建永久属于本人的
傀儡、跨 Project 绑定 Room、查看状态与暂停/恢复。Room 的成员关系仍独立管理。
本地配额、Codex 设置及高风险工具策略由 hagency-client 管理。

已删除 Fleet/Hafleet、Engagement/资源额度审批、自建账号审批和原生密码登录页面，
不提供旧路由别名。
