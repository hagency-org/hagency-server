# main 本地完整环境：资源一致性修复与验收

日期：2026-10-28。Client / Server 均在 main，已 fetch origin main 并仅允许快进更新。
基线分别为 Client `b906dd6800e391f935d7bd585be1261abab43dd4`、
Server `773dfd7562294511c1ad6e8bdf9756c5c82c682d`。本记录形成于提交前；未部署远端。

## 混用原因

1. 首次启动直接沿用了 checkout 中的功能分支，没有先确认最新 main 和既有主题设计。
2. 因本地缺少完整资源，我从 10 月 12 日旧 Server 镜像复制了浏览器产物，
   同时运行较新源码构建的后端。共享商务主题在 10 月 15 日才加入；
   Pasion 的固定上游版本相同，不代表 Hagency 的模板和共享静态资源也相同。
3. 旧启动逻辑只判断 index.html 存在；HTTP health/ready、真实账号/API 检查不能证明
   页面加载了匹配的资源。缺少的 `/assets/hagency-theme.css` 和 logo 返回 404，
   浏览器露出了 Pasion 原来的粉紫渐变。
4. 发现缺失资源后，我先尝试调整认证资源路径，而没有先恢复 main 的完整构建。
   调查后这些尝试被搁置。本次保持主干的共享 `/assets/` 约定和原品牌文件不变。

这些是启动与验证方法的错误。main 没有撤回统一主题，Pasion 页面也没有被新架构替换。
既有归属见 `server-brand-theme-unification-2026-10-15.zh.md`：管理界面由 Server 维护，
身份流程复用固定版本的 Pasion 前端，再注入 Hagency 品牌与共享主题。

## 实现

- Client `tools/local-dev.sh` 调用相邻 Server `scripts/local-dev.py`。
  `start --pull` 先更新两个 main，再执行完整构建与验证；`update` 可只更新源码。
  Git 分叉、非 main 或会覆盖本地修改时停止，不自动 reset/stash/force。
- `scripts/local_build.py` 为后端及其两个浏览器应用生成独立构建目录；原生 Client
  使用独立、同样受校验的构建目录。每批记录仓库路径、分支、HEAD、工具链、实际
  源码指纹，以及每个文件的 SHA-256。数据库和账号状态不在产物目录里。
- 一键环境不读取旧 resources 目录或镜像里的产物。缓存必须满足来源、完整性、
  文件校验；不匹配时重新构建整批资源。新产物完整验证后才替换运行服务。
  构建过程中源码变化会拒绝发布。两仓库均在切换进程前再次核对。
- 运行验证核对 `/login` 的实际 HTML、两个前端的脚本/样式/logo 和 WASM 字节。
  包含 Dioxus 生成的 `/./assets/` 引用规范化。`doctor` 可随时复核，API 健康
  不能代替资源验证。原生运行副本与签名包装目录分开，资源副本也核对原始清单。
- 一键隔离环境默认开放密码注册、联系方式可留空；保留已有私有配置。
  部署模板仍默认关闭注册。
- 关闭注册且无第三方注册方式时，页面显示关闭原因、已有账号/新用户的下一步，
  以及返回登录按钮。可用性加载时显示加载状态，不先展示虚构的可注册表单。
  修复通过 `xtask/patches/pasion-registration-availability.patch` 应用到专用构建副本，
  不修改上游 checkout、Cargo git 缓存或认证协议。补丁不匹配会明确失败；固定缓存
  checkout 的跟踪文件有修改时也拒绝构建。并发 Pasion 构建串行化。

## 实际验证

- 从 main 完整构建 Server、自有 Dioxus 前端、固定 Pasion WASM 前端及原生 Client。
- 真实浏览器检查 Server 欢迎页与 Pasion 注册页：按钮 RGB(37,99,235)，无渐变；
  logo 无破图。关闭注册页面分别检查浅色 RGB(244,246,249) 和深色 RGB(15,23,42)，
  表单隐藏、说明完整、返回登录成功。关闭状态使用同一构建包的独立真实临时后端，
  专用三库已清理；主测试环境始终恢复为开放注册。
- 省略邮箱/手机号，实际创建普通测试账号，完成显示名和注册。真实 Pasion
  DCR/PKCE/consent、Matrix whoami、Space、Project adopt、看板写入/读回通过。
  注册后最初的 whoami 遇到异步配置尚未完成的 401；配置完成后的验证通过，
  未把该首次响应记为成功。
- 完整原生客户端输入 loopback 地址，识别 Server、打开系统浏览器、完成真实授权
  和本机回调，显示项目及 API 创建的看板任务。没有使用身份或 Matrix 测试替身。
- 重复 start 保留原 Server/Client PID；stop 后 start、以及 restart --pull 均成功。
  原生账号自动恢复，项目和任务再次显示；SQL 读回原任务 version=1。
  管理员密码、数据库凭据和三份身份/签名密钥的五项指纹保持不变。
- 最终 doctor 通过：5 个去重 HTML 资源引用及所有打包 WASM 的真实响应与构建清单一致，
  Server / Client 来源与文件校验、原生运行副本校验通过。
- Python 启动/一致性回归 16/16；Rust xtask 单元 8/8、集成 4/4；严格 all-targets
  Clippy、格式、Bash 语法和两个仓库 diff 检查通过。未声称执行整个 Client/Server
  产品单元套件；本次产品后端业务代码未修改。

## 复现与证据

```sh
# Client 仓库
./tools/local-dev.sh start --pull
./tools/local-dev.sh doctor
./tools/local-dev.sh restart
./tools/local-dev.sh stop
```

完整说明在 Client `docs/local-development.zh-CN.md`。
Server 的 `.run/local-dev/` 保留 `final-launcher-tests.log`、`main-tools-tests.log`、
`main-tools-clippy.log`、`final-restart.log`、`final-doctor.log`、
`main-live-verification.json`、`idempotent-start-receipt.json`、
`persistence-before-stop.json`、`persistence-after-start.json` 和 `verification/` 截图。
该目录含私有配置/密码，已忽略，不应提交或公开导出整个目录。

当前入口为 `http://127.0.0.1:8088`，浏览器 `/login`，注册 `/auth/register`。
服务和原生客户端保留运行。模型推理、其他平台与远端部署不属于本次验收。
