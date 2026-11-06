# 部署与构建

服务器用 Docker 运行 Node + PostgreSQL；客户端是单个 Rust 程序，内置 SQLite、同步与会话通知。
`compose.yaml` 沿用原部署的项目名 `agent-mail`、服务名 `private-mail` 和卷名 `private-mail-data`。
本轮在临时目录验证原生接入、聚合收件、单后台启停和离线团队隔离。真实会话唤醒、Windows 原生管道和各系统登录恢复仍待目标环境验收。

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

客户端运行 `agent-mail service start`，一个后台负责全部已连接团队及本地通知。`service stop` 停止后台并移除用户登录任务；`debug sync stop/start <team>` 单独暂停或恢复团队。首次替换旧安装须先停用 agmsgd 和旧钩子，避免双引擎。

## 构建客户端

本机：`cargo build --release --locked`。
Docker 交叉构建 Windows（不安装到宿主机）：

```sh
docker buildx build --file deploy/client.Dockerfile --target windows-artifact --output type=local,dest=dist/windows .
```

Linux 输出使用 `--target linux-artifact`（运行环境需兼容构建镜像的 glibc）；需要跨发行版运行时使用 `--target linux-static-artifact`，生成 Linux x86_64 musl 静态程序。客户端不调用 Bash、外置 sqlite3 或 Node。Windows 原生通知和系统登录恢复需在目标机器上另行实测；交叉编译本身不代表真实会话验收。
