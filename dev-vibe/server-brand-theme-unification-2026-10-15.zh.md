# Server 品牌和商务主题统一

日期：2026-10-15。

## 界面归属

| 入口 | 当前实现 | 品牌和配色维护位置 |
| --- | --- | --- |
| `/`、`/login`、`/admin`、`/hagency/*` | Server 本地 Dioxus 前端，整合原 Padmin/Palpo 管理和 Hagency 页面 | `crates/frontend/src` 与 `public/assets/hagency-theme.css` |
| `/auth/*` 的登录、注册、恢复、账号、安全和 OAuth 授权 | Cargo.lock 固定的 Pasion 前端与模板，版本 `03cd9f94c0c0593c3979de68a0b11ee443f23933` | Server 在资源准备阶段对复制后的页面外壳加入 `branding/pasion-head.html`、`branding/pasion-header.html` |

Pasion 的全部页面并非 Server 内重写。`xtask/src/assets.rs` 继续从固定版本构建其前端，不修改上游 checkout。Pasion 管理页面在统一 Server 控制台里维护；身份账号中心保留 Pasion 的界面和业务流程。

## 修改

- 两条界面链路加载同一份 `/assets/hagency-theme.css`，与 Cloud 商务主题对齐：浅灰 `#f4f6f9` 页面、白色卡片、正文 `#172437`、蓝色 `#2563eb` 主按钮，以及 `#14243b` 海军蓝导航；深色页面 `#0f172a`、卡片 `#18243a`。
- CSS 兼容统一控制台的 HSL/alpha 工具类和 Pasion 的 `--color-*` 变量。覆盖规则在 Pasion 原 CSS 后加载时依旧有效，移除原珊瑚/紫色光晕与按钮渐变。语义颜色保留成功绿、警告琥珀、错误红；实心危险操作按钮与状态文字分别取色。
- 欢迎页、普通用户/管理员侧栏及折叠态使用新 mark。Pasion 外壳增加带同一 mark 的 Hagency 品牌栏，登录、注册、恢复、账号和授权/错误页面共用；为品牌栏扣除原页面最小高度的相应空间。
- 保留 OAuth 客户端 logo、Pasion branding.logo_uri 和浏览器 SVG/ICO 图标。OAuth 外部应用和第三方身份提供方图标保持对应实体，权限/安全等功能图标保持原意。
- `hagency-theme.js` 仅在 WASM 启动前读取各自现有 `theme`、`pasion_theme` 偏好并设置原 DOM 主题约定；不写偏好，不新增存储键，不改变现有主题开关。
- 镜像 CI 检查 SVG、ICO、共享 CSS/JS、两种页面外壳的样式引用和 Pasion 品牌栏。

## 验证及范围

- 从本地上游 Git 对象读取 Cargo.lock 对应版本的实际 CSS 和 app.html，独立渲染带 Server 品牌片段的模板；原运行时配置和入口脚本保留。
- 12 个 CSS/浏览器样例通过：统一控制台、Pasion 登录、Pasion 账号中心 × 明暗 × 1280/390px。原 Pasion CSS 后加载；核对页面/卡片/按钮/侧栏颜色、选中态、logo、主题切换和无横向溢出，主按钮白字对比度超过 4.5:1。
- 8 个初始主题样例通过：两种入口 × 系统明暗 × 空偏好/存储不可用。显式主题与相反系统主题的优先级也在上述 12 个样例中检查。
- CSS 解析、变量覆盖、JavaScript 语法、cargo fmt 与 git diff --check 通过。对比 Git HEAD，原控制台全部非颜色 CSS 声明一致，字体、布局、间距、圆角和断点未改；Pasion 只为新增品牌栏预留空间。
- 浏览器使用静态代表性页面和真实样式/模板，不是完整 WASM、认证端到端或生产验收。预览与验证脚本在 ignored `target/brand-qa`；临时 Python 模板依赖仅安装在该目录。
- 未提交、推送、发布镜像或部署；未在本机/生产机编译 Server 或前端。完整构建与运行验证仍交给 GitHub CI。

没有修改账户、登录/注册、OAuth/CSRF、权限、路由、数据库或 schema。
