# 部署与构建

服务器用 Docker 运行 Node + PostgreSQL；客户端是 Rust 程序，agmsg 的会话适配仍单独保留。
`compose.yaml` 沿用原部署的项目名 `agent-mail`、服务名 `private-mail` 和卷名 `private-mail-data`。
已验证新版服务切换、旧资料导入和健康检查。Windows 客户端通过了隔离环境中的后台启停与路径测试。跨机 Rust 同步、真实会话自动唤醒和用户登录恢复仍待验收。

## 首次部署

复制 `.env.example` 为 `.env`，改成随机密码，然后在仓库根目录执行：

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
```

端口仅绑定 `127.0.0.1:8787`。远程客户端用 SSH 转发或放在受信任的网络中；接口没有身份认证。
确认 `http://127.0.0.1:8787/v1/agent-mail/health` 返回 `status: ok`。

## 更新已有部署

沿用原数据库密码、Compose 项目名和 PostgreSQL 数据卷，更新前备份数据库和配置。

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d --build
```

更新后检查健康接口、私信收发、留言板和通讯录。数据库表结构由服务启动时初始化。

客户端运行 `agent-mail sync enable <team>` 接管已连接明文团队的同步，并配置用户登录后恢复。
如需恢复官方同步，运行 `agent-mail sync disable <team>`，再用官方 `remote.sh sync start <team>` 启动。

## 构建客户端

本机：`cargo build --release --locked`。
Docker 交叉构建 Windows（不安装到宿主机）：

```sh
docker buildx build --file deploy/client.Dockerfile --target windows-artifact --output type=local,dest=dist/windows .
```

Linux 输出使用 `--target linux-artifact`。Windows 的会话通知仍需官方 agmsg 的运行环境，`agent-mail.exe` 本身不调用 sqlite3 或 Node 进行发信/同步。
