# Hagency Server 品牌资源

更新：2026-10-15。

- 新 logo 原样来自 Cloud `assets/hagency-mark.svg`，保存在 `crates/frontend/public/assets/hagency-mark.svg`，公开地址 `/assets/hagency-mark.svg`。固定资源沿用现有 no-cache/ETag/GET/HEAD 路由，不改 API 或认证边界。
- 欢迎页顶部和主卡片、管理员/普通用户共用侧栏及折叠侧栏已替换通用品牌图标。权限菜单里的盾牌等功能图标保持原意。
- 新前端 index.html 声明 SVG favicon；`public/favicon.ico` 使用同一 logo 的 Desktop 派生 ICO，覆盖浏览器默认回退。Dioxus public 目录由现有 xtask 复制进最终镜像，SVG 沿用 gzip 预压缩。
- 内嵌 Pasion 资源准备时，向复制后的 app.html 加入同源 favicon、共享主题和 Hagency 品牌栏，覆盖登录/注册/账号/授权/错误页面；上游 checkout 不修改。Pasion branding.logo_uri 和 Hagency Server OAuth 客户端 logo_uri 指向该 SVG，其他品牌法律配置保留。
- 原 Padmin 功能由 Server 本地 `crates/frontend` 维护；Pasion 身份前端仍由 Cargo.lock 固定上游版本构建。共享商务主题在 `crates/frontend/public/assets/hagency-theme.css`，与 Cloud 的灰白/蓝色/海军蓝及深蓝灰配色一致。原字体、布局和主题偏好键保持，Pasion 品牌栏单独预留高度。
- 镜像 CI 校验最终镜像内 SVG、ICO、共享 CSS/JS，以及两个页面模板的品牌和主题引用。
- 已验证与 Cloud 源 SVG 逐字节一致、XML/ICO 解码、真实 index.html 的 Chromium favicon 加载、资源路由/MIME/复制路径、模板注入保持原内容、cargo fmt 与 git diff --check。
- 主题追加验证：12 个代表性 CSS 浏览器样例、8 个初始主题回退样例通过；使用固定 Pasion 版本的实际样式与模板，非完整 WASM 端到端。CSS/JS 检查及原控制台非颜色声明一致性通过。细节见 `../server-brand-theme-unification-2026-10-15.zh.md`。
- 未本地编译 Server/前端，未提交、发布镜像或部署；完整构建和运行验证仍需 GitHub CI，不把这些静态检查当作镜像发布成功。
