# astorm-server / astorm-cli Linux 镜像（无头 API + Web UI + CLI）
#
# 构建（在仓库根目录；Dockerfile 在 backend/，构建上下文 = 仓库根）：
#   docker build -f backend/Dockerfile -t astorm-server:dev .
# 运行：
#   docker run -p 8787:8787 -v astorm-data:/data -e ASTORM_TOKEN=xxx astorm-server:dev
#   LLM 供应商不预置：浏览器登录后在「设置 → 模型」配置（写 /data/config/providers.json）。
#   自动化/CI 可用 ASTORM_LLM_BASE_URL / ASTORM_LLM_API_KEY / ASTORM_LLM_MODEL 预置。
# 说明：
# - 镜像内含 Chromium + astorm-render 渲染助手：webfetch（硬依赖页面渲染）与
#   websearch 的 baidu/google 引擎开箱可用；stealth 反爬脚本随助手内置。
# - MCP 服务器开箱可跑：node/npm（npx 系）、python3/pip + uv（uvx 系）均已内置，
#   客户端在 core（Rust，stdio + HTTP JSON-RPC），配置走 <数据根>/config/mcp_servers.json。
# - office 工具 + 内置 skill 默认内置：构建前运行 node scripts/sync-office-build.mjs
#   同步 ../astorm-office 构建源即可（源缺失时自动降级为不含 office）。

# edition 2024 需 Rust ≥ 1.85；用最新 stable 避免追锁文件工具链版本
FROM rust:1-slim AS builder
# 可选：apt 源镜像（默认不变 = deb.debian.org；中国大陆构建建议
#   --build-arg APT_MIRROR=mirrors.aliyun.com  —— 直连 deb.debian.org 常只有几十 KB/s）
ARG APT_MIRROR=deb.debian.org
RUN if [ "$APT_MIRROR" != "deb.debian.org" ]; then \
      sed -i "s|deb.debian.org|$APT_MIRROR|g" /etc/apt/sources.list.d/*.sources /etc/apt/sources.list 2>/dev/null || true; \
    fi \
    && apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev git \
    && rm -rf /var/lib/apt/lists/*
# crates 源镜像：构建容器实测连不上 index.crates.io 与 github.com（cargo 会卡在
# "Updating crates.io index"）；境外/CI 传 --build-arg CARGO_MIRROR= 即回官方源。
ARG CARGO_MIRROR=sparse+https://rsproxy.cn/index/
RUN if [ -n "$CARGO_MIRROR" ]; then \
      printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' "$CARGO_MIRROR" \
        > /usr/local/cargo/config.toml; \
    fi
WORKDIR /build
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/crates ./crates
COPY backend/tests ./tests
# BuildKit 缓存挂载：target / registry 跨次构建持久化（增量编译，分钟级迭代）；
# 产物拷到 /out（target 缓存卷不进镜像层）
RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release -p astorm-server -p astorm-cli \
    && mkdir -p /out \
    && cp target/release/astorm-server target/release/astorm target/release/astorm-render /out/

# ---- office 五件套 + 内置 skill（默认内置）----
# 构建前运行 node scripts/sync-office-build.mjs 把 ../astorm-office 构建源同步到
# backend/.office（剪枝后 ~8M）；源缺失时此步自动跳过（目录仅剩 manifest），镜像
# 不含 office，工具按契约报未找到——不影响其余能力
COPY backend/.office /office
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/office/target \
    if [ -f /office/Cargo.toml ]; then \
      cd /office \
      && cargo build --release \
      && mkdir -p /out-office/bin /out-office/skill \
      && cp target/release/astorm-office target/release/json2docx target/release/json2xlsx target/release/json2pptx target/release/json2pdf /out-office/bin/ \
      && cp -r skill/. /out-office/skill/ \
      && for book in ai-word ai-excel ai-ppt ai-pdf; do \
           if [ -d "$book/skill" ]; then cp -r "$book/skill" "/out-office/skill/$book"; fi; \
         done ; \
    else mkdir -p /out-office/bin /out-office/skill ; fi

# ---- agent-browser CLI（Linux 版）----
# 容器里「工具侧」的浏览器驱动：core 的 browser 工具照桌面那条路 spawn 它（`--cdp` 附着到
# 服务端自有 Chromium 的浏览器级 WS），审批/分级授权/结果整形仍在 core。源码与桌面
# resources/agent-browser/<os>-<arch> 同一份（vercel-labs/agent-browser，Apache-2.0），
# 版本由 backend/.agent-browser/manifest.json 记录 —— 改源码时同步桌面二进制。
#
# 源码**随构建上下文进来**（`node backend/scripts/sync-agent-browser-src.mjs` 同步到
# backend/.agent-browser/，剪枝后 ~7M），不在这里 git clone：实测 Docker 构建环境连不上
# github.com。源缺失/编译失败只告警：镜像不含 CLI，browser 工具落回服务端直连 CDP 的宿主
# 实现（astorm-server/src/browser/mod.rs），能力不受影响；降级会在镜像里留 NOT-BUILT 说明。
FROM rust:1-slim AS abbuilder
# crates 源镜像：构建容器实测连不上 index.crates.io 与 github.com（cargo 会卡在
# "Updating crates.io index"）；境外/CI 传 --build-arg CARGO_MIRROR= 即回官方源。
ARG CARGO_MIRROR=sparse+https://rsproxy.cn/index/
RUN if [ -n "$CARGO_MIRROR" ]; then \
      printf '[source.crates-io]\nreplace-with = "mirror"\n[source.mirror]\nregistry = "%s"\n' "$CARGO_MIRROR" \
        > /usr/local/cargo/config.toml; \
    fi
COPY backend/.agent-browser /ab
RUN --mount=type=cache,target=/cargo-target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    mkdir -p /out-ab \
    && rm -f /out-ab/NOT-BUILT \
    && if [ -f /ab/cli/Cargo.toml ]; then \
         ( CARGO_TARGET_DIR=/cargo-target cargo build --release --manifest-path /ab/cli/Cargo.toml \
           && cp /cargo-target/release/agent-browser /out-ab/agent-browser \
           && /out-ab/agent-browser --version ) \
         || printf 'agent-browser 编译失败：检查构建日志与磁盘。\n' > /out-ab/NOT-BUILT ; \
       else \
         printf 'agent-browser 源码未同步：先跑 node backend/scripts/sync-agent-browser-src.mjs。\n' > /out-ab/NOT-BUILT ; \
       fi

# ---- Web UI 构建（复用桌面 renderer 工程，产物为静态 SPA）----
# electron-vite build 产出 out/renderer；ELECTRON_SKIP_BINARY_DOWNLOAD 跳过 Electron
# 二进制下载（构建期不需要运行时）；sync-office 缺 sibling 仓库时优雅降级为占位组件
FROM node:22-slim AS webbuilder
RUN corepack enable && corepack prepare pnpm@10 --activate
ENV ELECTRON_SKIP_BINARY_DOWNLOAD=1
# frontend 是独立 pnpm 项目（锁文件在 frontend/）；根 scripts/sync-office.mjs
# 用于 office viewer 占位降级（kit 缺失时保证 renderer 可编译）
WORKDIR /src
COPY scripts ./scripts
# 依赖层先行：只拷清单文件装依赖（源码改动不 bust 该层）
COPY frontend/package.json frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml ./frontend/
# vite 打包大 SPA 需要显式放宽 V8 堆（容器默认 ~2GB 会 OOM）
ENV NODE_OPTIONS=--max-old-space-size=4096
RUN cd frontend && (pnpm install --frozen-lockfile --ignore-scripts || pnpm install --ignore-scripts)
COPY frontend ./frontend
# 已有真实 viewer（构建上下文带了 resources/office/viewer）时不要用占位组件覆盖它；
# 仅在缺失时生成 stub，保证 renderer 可编译。
RUN [ -f frontend/resources/office/viewer/office-viewer.js ] || node scripts/sync-office.mjs || true
RUN cd frontend && pnpm run build

# 运行时与 builder 同基线：rust:1-slim 现为 trixie（glibc 2.39+），bookworm（2.36）会报
# GLIBC_2.39 not found；OpenSSL 包名 trixie 为 libssl3t64（旧名兜底）
# chromium 供 astorm-render 渲染助手使用（websearch 防反爬 / webfetch 硬依赖）
# node + npm 供 npx 系 MCP 服务器（`{"command":"npx","args":[...]}`）；python3 + uv 供
# uvx / python 系 MCP 服务器——MCP 客户端在 core（Rust）内，子进程由它拉起。
FROM debian:trixie-slim
# 可选：apt 源镜像（默认不变 = deb.debian.org；中国大陆构建建议
#   --build-arg APT_MIRROR=mirrors.aliyun.com  —— 直连 deb.debian.org 常只有几十 KB/s）
ARG APT_MIRROR=deb.debian.org
RUN if [ "$APT_MIRROR" != "deb.debian.org" ]; then \
      sed -i "s|deb.debian.org|$APT_MIRROR|g" /etc/apt/sources.list.d/*.sources /etc/apt/sources.list 2>/dev/null || true; \
    fi \
    && apt-get update \
    && (apt-get install -y --no-install-recommends git ca-certificates libssl3t64 curl chromium fonts-liberation fonts-noto-cjk \
        || apt-get install -y --no-install-recommends git ca-certificates libssl3 curl chromium fonts-liberation fonts-noto-cjk) \
    && apt-get install -y --no-install-recommends nodejs npm python3 python3-pip \
    && rm -rf /var/lib/apt/lists/*
# uv/uvx（python MCP 服务器主流分发方式）：装失败不阻断镜像（node/python 已可用，
# uvx 只是便利；构建日志会留痕）。PyPI 索引可换（境外/CI 可 --build-arg PIP_INDEX_URL=https://pypi.org/simple）
ARG PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple
RUN pip install --break-system-packages --no-cache-dir --index-url "$PIP_INDEX_URL" uv \
    || echo "warn: uv 安装失败（uvx 系 MCP 服务器不可用，npx/python 不受影响）"
COPY --from=builder /out/astorm-server /usr/local/bin/astorm-server
# astorm-cli 的 [[bin]] 名为 astorm（包名 ≠ 二进制名）
COPY --from=builder /out/astorm /usr/local/bin/astorm
# 页面渲染助手（webRender 协议的容器侧实现）
COPY --from=builder /out/astorm-render /usr/local/bin/astorm-render
# 工具侧浏览器驱动（agent-browser CLI，Linux）：core 按 PATH 查找（resolve_binary）；
# 缺省时目录为空 → 工具落回服务端直连 CDP 实现
COPY --from=abbuilder /out-ab/ /opt/astorm/agent-browser/bin/
ENV PATH="/opt/astorm/agent-browser/bin:${PATH}"
# Web UI 静态产物（同源托管；--web-dir 默认指向这里）
COPY --from=webbuilder /src/frontend/out/renderer /app/web
# office 五件套 + skill（构建期已内置；bin/skill 双目录，ENV 注入配置）
COPY --from=builder /out-office/bin/ /opt/astorm/office/bin/
COPY --from=builder /out-office/skill/ /opt/astorm/office/skill/

ENV ASTORM_DATA_DIR=/data
# office 工具：CLI 目录（core 回退读取）+ 内置 skill 目录（经配置 office.skillDir 注册）
ENV ASTORM_OFFICE_BIN_DIR=/opt/astorm/office/bin
ENV ASTORM_OFFICE_SKILL_DIR=/opt/astorm/office/skill
# 页面渲染：astorm-render 助手（说 WebRender 的 stdin/stdout 协议）+ 容器内 Chromium
# （script 仅占位——助手忽略该参数；core 的 WebRender 要求 bin/script 均非空才拉起服务）
ENV ASTORM_WEB_RENDER_BIN=/usr/local/bin/astorm-render
ENV ASTORM_WEB_RENDER_SCRIPT=builtin
ENV ASTORM_CHROME_BIN=/usr/bin/chromium
# 同源 Web UI（窗口直接开 http://<host>:8787 即是完整前端）
ENV ASTORM_WEB_DIR=/app/web
# MCP 子进程的包缓存落进数据卷：容器重建不必重下 npx/uvx 包；npm 走国内镜像
# （与仓库 .npmrc 同源；境外部署可 `-e NPM_CONFIG_REGISTRY=https://registry.npmjs.org` 覆盖）
ENV NPM_CONFIG_CACHE=/data/.cache/npm
ENV NPM_CONFIG_REGISTRY=https://registry.npmmirror.com
ENV UV_CACHE_DIR=/data/.cache/uv
ENV UV_DEFAULT_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple
ENV PIP_INDEX_URL=https://pypi.tuna.tsinghua.edu.cn/simple
# 降权运行：Linux 端沙箱是 passthrough，容器内 root 会把“唯一权限边界”也去掉。
# /data 由命名卷初始化时继承属主；bind mount 需保证目录对 uid 10001 可写。
RUN useradd --system --uid 10001 --create-home astorm \
    && mkdir -p /data && chown -R astorm:astorm /data
USER astorm
WORKDIR /data
VOLUME /data
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8787/health || exit 1
ENTRYPOINT ["/usr/local/bin/astorm-server"]
