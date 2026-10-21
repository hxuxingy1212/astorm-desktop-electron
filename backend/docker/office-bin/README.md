# docker/office-bin/（可选层）

构建镜像前，把 **Linux** 构建的 astorm-office 五件套放进来：

    astorm-office  json2docx  json2xlsx  json2pptx  json2pdf

来源：sibling 仓库 `../astorm-office`（纯 Rust）在 Linux 上 `cargo build --release`
后拷贝五个二进制到此目录，再 `docker build`（`astorm-office` 是统一入口，需与四个
格式 CLI 同目录，按兄弟可执行文件分发）。

Dockerfile 会把本目录整体拷到 `/usr/local/bin/`，配合 `ASTORM_OFFICE_BIN_DIR`
环境变量自动启用 office 工具；目录为空（只有本 README 与 .keep）时 office 工具
按契约报「未找到 CLI」，其余能力不受影响。
