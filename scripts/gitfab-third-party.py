#!/usr/bin/env python3
"""Writes docs/gitfab/third-party.zh-CN.md: every third-party crate the GitFab crates link at run time,
with its version and declared license, from `cargo metadata` (no build). A crate without a declared
license is listed as NOASSERTION, never guessed.

    python3 scripts/gitfab-third-party.py
"""
import json
import os
import subprocess
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = ["hagency-gitfab-events", "hagency-gitfab-forge", "hagency-gitfab-runner", "hagency-gitfab-board"]
TOOLS = [
    ("Docker Engine / CLI", "运行时", "Apache-2.0", "Runner 构建镜像、启动容器"),
    ("curl", "运行时", "curl（MIT 类）", "验收脚本与大屏读取接口"),
    ("anchore/syft 镜像（默认 v1.18.1）", "运行时（容器）", "Apache-2.0", "Runner 生成 SPDX SBOM；可用 GITFAB_SYFT_IMAGE 替换"),
    ("sigstore/cosign 镜像（默认 v2.4.1）", "运行时（容器，可选）", "Apache-2.0", "Runner 用 --cosign-key 时签名镜像；verify-release 核对签名；可用 GITFAB_COSIGN_IMAGE 替换"),
    ("busybox 镜像", "仅本机测试", "GPL-2.0", "部署与回退实测用的示例服务，不随交付物分发"),
]

# Direct dependencies GitFab adds to existing components, against their main branches.
CHANGED = [
    ("服务端 agent-service、backend", "hagency-gitfab-events、hagency-gitfab-forge（本仓库）", "随本仓库", "不新增外部 crate；其依赖已计入下方清单"),
    ("桌面端（修复分支 fix/server-clock-skew、fix/hagency-panel-jit）", "httpdate 1.0.3", "MIT OR Apache-2.0", "原已是 hyper 的传递依赖，锁文件不新增包，只新增一条直接依赖"),
]


def main():
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout)
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    roots = [p["id"] for p in meta["packages"] if p["name"] in CRATES]
    seen, stack = set(), list(roots)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] in (None, "normal") for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    third = sorted((packages[i] for i in seen if packages[i]["source"]), key=lambda p: (p["name"], p["version"]))
    licenses = Counter(p["license"] or "NOASSERTION" for p in third)
    lines = [
        "# GitFab 开源软件及第三方组件清单",
        "",
        "由 `scripts/gitfab-third-party.py` 从 `cargo metadata` 生成，不要手改。范围：GitFab 新增的 "
        + "、".join(f"`{c}`" for c in CRATES)
        + " 在运行时链接的全部第三方 crate（不含仅用于测试和构建脚本的依赖）。许可证取自各 crate 的声明，未声明的记为 NOASSERTION。",
        "",
        f"共 {len(third)} 个 crate。许可证分布：",
        "",
        "| 许可证 | 数量 |",
        "|---|---|",
    ]
    lines += [f"| {lic} | {n} |" for lic, n in licenses.most_common()]
    lines += [
        "",
        "## 运行时外部工具与镜像",
        "",
        "| 组件 | 用途范围 | 许可证 | 说明 |",
        "|---|---|---|---|",
    ]
    lines += [f"| {a} | {b} | {c} | {d} |" for a, b, c, d in TOOLS]
    lines += [
        "",
        "## 原有组件的依赖变化",
        "",
        "GitFab 对原有组件只做增补，下表是相对各自 main 分支新增的直接依赖（核对 Cargo.toml 与 Cargo.lock 的差异）。",
        "",
        "| 组件 | 新增直接依赖 | 许可证 | 说明 |",
        "|---|---|---|---|",
    ]
    lines += [f"| {a} | {b} | {c} | {d} |" for a, b, c, d in CHANGED]
    lines += ["", "## Rust crate", "", "| crate | 版本 | 许可证 |", "|---|---|---|"]
    lines += [f"| {p['name']} | {p['version']} | {p['license'] or 'NOASSERTION'} |" for p in third]
    out = os.path.join(ROOT, "docs", "gitfab", "third-party.zh-CN.md")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"{out}: {len(third)} crates")


if __name__ == "__main__":
    main()
