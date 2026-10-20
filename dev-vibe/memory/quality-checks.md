# Server 质量检查

2026-10-20：用户要求补齐 typos/Clippy CI，并在本地修复检查错误。

- `.github/workflows/ci.yml` 原有后端、agent-service、xtask 的严格 Clippy；新增固定 `crate-ci/typos@v1.48.0`，将 WASM 前端的 check 升级为 `clippy --all-targets -- -D warnings`，并将前端单元测试加入现有测试步骤。
- 基于工作区已有的未跟踪 `typos.toml` 调整配置：检查隐藏配置和 JS/CSS/HTML；仅排除 `.git`，针对 Rust Unicode 转义及部署记录中的 Gitar 专名作精确豁免，移除无关模板词条和整类文件排除。
- 前端修复冗余闭包、Copy 类型的 clone、嵌套条件、固定数组、分页向上取整和默认结构初始化；删除未使用的内部接口、类型及辅助函数。测试模块移至文件末尾。ProjectControls 独立放入单节点 rsx，使节点 key 生效。
- HttpError 将结构化 MatrixError 暴露为标准错误 source，保留原展示文本和请求引用；延迟统计仅保留实际使用的时长及最近 200 条窗口。新增这两处的回归测试。
- 使用 Rust 1.99.0。本地后端与 agent-service：`cargo +1.99.0 clippy --locked -p hagency-server -p hagency-agent-service --all-targets -- -D warnings` 通过，日志 `D:/BuildCache/hagency-server/clippy-backend.log`。
- xtask 含 Unix 专用 API，Windows 无法编译；WSL Ubuntu 下同版本 `cargo +1.99.0 clippy --locked -p hagency-xtask --all-targets -- -D warnings` 通过，日志 `D:/BuildCache/hagency-server/clippy-xtask.log`。最初尝试 nightly 后改为精确 1.99.0 验证；最终结果来自 1.99.0。
- WASM 前端：`cargo +1.99.0 clippy --locked -p hagency-frontend --all-targets --target wasm32-unknown-unknown -- -D warnings` 通过，日志 `D:/BuildCache/hagency-server/clippy-wasm-final.log`。前端 `cargo +1.99.0 test --locked -p hagency-frontend`：17 通过、0 失败，日志 `D:/BuildCache/hagency-server/test-frontend.log`。
- `typos` 1.48.0、`cargo +1.99.0 fmt --all -- --check`、actionlint 1.7.12 和 `git diff --check` 均通过。已有 rustfmt.toml 的 nightly 专用选项在稳定版会提示忽略，但格式检查成功；保留用户配置。
- Windows `CARGO_TARGET_DIR=D:/BuildCache/hagency-server/target`，WSL 为 `/mnt/d/BuildCache/hagency-server/target-linux`；cargo metadata 已分别核对实际目录。findmnt 确认 `/mnt/d` 来自 D 盘。Cargo 缓存 `D:/BuildCache/hagency-client/cargo`，临时目录在同目录的 tmp/tmp-linux；WSL 新装工具链也在 `D:/BuildCache/hagency-server/rustup`。WSL 读写共享缓存需 root，普通用户会遇到 mtime 权限错误。
- 保留任务开始前已有的 rust-toolchain.toml 删除和未跟踪 rustfmt.toml；不手工修改 schema.rs。未进行 Docker 构建、生产操作、提交或推送。没有重跑完整后端数据库测试，后端源码未改变。

2026-10-20 后续：用户明确要求将 Server 与 Client 两轮改动分别提交并推送远端 main，包含此前保留的工具链/格式配置。提交前 fetch 确认与 origin/main 无分叉；typos、actionlint、diff 检查再次通过。推送后的 CI 以本次提交的 GitHub Actions 为准，状态记录保存在 D:/BuildCache/hagency-server/push-ci-*，不为追加 CI 记录重复触发完整构建。
