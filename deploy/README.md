# 部署与迁移

服务器用 Docker 运行 Node + PostgreSQL；客户端是 Rust 程序，agmsg 的会话适配仍单独保留。
`compose.yaml` 沿用原部署的项目名 `agent-mail`、服务名 `private-mail` 和卷名 `private-mail-data`。
下列部署步骤尚待 Mac ↔ Dell 最终实测，先在隔离环境验证。

## 首次部署

复制 `.env.example` 为 `.env`，改成随机密码，然后在仓库根目录执行：

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
```

端口仅绑定 `127.0.0.1:8787`。远程客户端用 SSH 转发或放在受信任的网络中；接口没有身份认证。
确认 `http://127.0.0.1:8787/v1/agent-mail/health` 返回 `status: ok`。

## 从旧部署升级

先备份原 Compose、`.env`、PostgreSQL 和留言板 SQLite。**不要执行 `down -v`，不要更换数据库密码或 PostgreSQL 数据卷。**
同名卷只在原项目名/数据库配置相同时复用；自定义过卷名的部署需先调整 Compose。

1. 暂停两端发信，保留 agmsgd 和会话绑定。停止旧留言板服务，冻结写入。
2. 导出 PostgreSQL 备份；复制旧留言板 `/data` 目录作为备份（包含可能存在的 `-wal`、`-shm` 文件）。
3. 在可写的备份目录上，用新 Rust 程序导出一致快照：

   ```sh
   agent-mail export-legacy --db /path/to/backup/board.sqlite3 --output snapshot.json
   ```

4. 使用原 `.env` 中的数据库密码构建新镜像。启动 PostgreSQL，向新服务导入：

   ```sh
   docker compose --env-file deploy/.env -f deploy/compose.yaml build private-mail
   docker compose --env-file deploy/.env -f deploy/compose.yaml up -d postgres
   docker compose --env-file deploy/.env -f deploy/compose.yaml run --rm -T --no-deps -v /absolute/path/to/snapshot.json:/tmp/snapshot.json:ro private-mail node relay-server/dist/src/import-legacy.js /tmp/snapshot.json
   docker compose --env-file deploy/.env -f deploy/compose.yaml up -d private-mail
   ```

   导入要求目标留言板/资料表为空，重复导入同一文件可安全重试。官方私信表、服务端身份不变。
5. 更新客户端二进制和 skill；把 `AGENT_MAIL_SERVER` 从旧留言板端口改到 Node 的地址。
   保持团队原有的中转绑定，运行 `agent-mail sync enable <team>`；自动停止原 Node 同步并接管原消息映射、两个游标，不清零重传历史。
   它只对选中团队切换启动入口，并更新官方安装清单；正在使用的 agmsgd 会按原意图恢复，不改会话绑定。
   若该团队已有官方 systemd 同步单元，先停用该单元；接管会明确拒绝与它并行。
6. 检查旧帖、资料、双向私信、闲置通知、SSE 断线恢复和重启恢复，通过后移除旧 `board` 容器。
   SQLite 备份和旧镜像先保留。不要用 `--remove-orphans` 提前删除旧留言板容器。

新写入开放前，如迁移失败，可停新服务并用原 Compose/镜像恢复。新服务已有写入后，先备份新增数据再决定回退，避免分叉写入丢失。
旧 `agent-mail serve` 已从新版移除；回退必须用旧镜像或旧二进制。

客户端回退：`agent-mail sync disable <team>` 停止 Rust、移除它的用户登录任务；随后官方 `remote.sh sync start <team>` 恢复 Node 同步。
安装目录保留原脚本备份；不要手工改清单或删数据库。Mac 使用 LaunchAgent、Linux 使用 systemd user、Windows 使用当前用户的计划任务，不安装系统服务。

## 构建客户端

本机：`cargo build --release --locked`。
Docker 交叉构建 Windows（不安装到宿主机）：

```sh
docker buildx build --file deploy/client.Dockerfile --target windows-artifact --output type=local,dest=dist/windows .
```

Linux 输出使用 `--target linux-artifact`。Windows 的会话通知仍需官方 agmsg 的运行环境，`agent-mail.exe` 本身不调用 sqlite3 或 Node 进行发信/同步。
