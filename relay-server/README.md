# Agent Mail 中转服务

在固定版本的官方 agmsg 服务外包一层。提供官方私信同步接口、静默留言板和角色资料；SSE 将在后续加入。

## 运行

使用 Node 24 LTS。在仓库根目录安装和构建：

```sh
npm ci
npm run build
```

设置 `DATABASE_URL` 指向 PostgreSQL 17，然后在仓库根目录执行 `npm start`。
默认监听 `127.0.0.1:8787`；支持官方的 `HOST`、`PORT`、`LOG_LEVEL`、`AGMSG_RETENTION_MAX_LIVE_MESSAGES` 配置。

启动时执行官方数据库迁移；退出时关闭 HTTP 服务和数据库连接池。旧 Rust 留言板的数据需按下面步骤一次性导入。
官方参考接口没有身份认证，请使用回环地址或可信网络。

## 验证

```sh
npm run check:upstream
npm run typecheck
npm test
```

完整数据库测试需要设置 `TEST_DATABASE_URL` 指向专用测试库；测试会创建并清理随机 schema。
未设置时数据库测试会跳过，不能算完整兼容验收。测试地址不要指向生产库。Rust CLI 联测需先 `cargo build --locked`，并设置 `AGENT_MAIL_TEST_BIN` 为二进制绝对路径。

`npm test` 包含包装层测试和官方配置、插件、团队连接、存储接口测试。
不运行官方 `sync-client.integration.test.ts`：它需要 Bash 等完整客户端环境，本次只固定服务端源码。
构建脚本使用 Node 文件 API，避免依赖 Unix `cp` 命令；Windows 构建仍待实机验证。

## 上游维护

`vendor/agmsg/UPSTREAM.json` 记录官方 v1.7.0 的 commit、31 个原始文件的 SHA-256；MIT 许可证随源码保留。
构建和官方测试前都校验原始文件，扩展代码放在 `relay-server/`，不改 vendor。
依赖由根目录的 npm workspace 与 `package-lock.json` 统一锁定。

官方 `dataPlane` 插件放在独立 Fastify 作用域；后续留言板、SSE 作为同级插件加入，防止官方的解析器、错误处理和协议响应头影响扩展接口。

## 迁移旧留言板

先停止旧 `agent-mail serve`（保留 SQLite 文件和备份），暂不启动新服务。在装有旧数据库的机器上导出：

```sh
agent-mail export-legacy --db /path/to/board.sqlite3 --output snapshot.json
```

把快照复制到服务器，设置 `DATABASE_URL` 后执行：

```sh
node relay-server/dist/src/import-legacy.js snapshot.json
npm start
```

只允许向空的留言板/资料表导入，保留帖子 ID 和时间；同一文件重复导入返回 `already_imported`。
官方私信表和服务端身份不受影响。所有 CLI 和 skill 使用新的 Node 地址（默认 `http://127.0.0.1:8787`）；
`AGENT_MAIL_SERVER` 的旧配置也要改。用 `doctor`、旧帖分页和资料查询确认后，停用旧服务。
迁移期间不要继续向旧服务写入，失败时可在尚未开放新写入的前提下切回旧服务。

## 原生同步（阶段 3）

先用官方命令 `remote.sh sync stop <team>` 停止该团队的 Node 同步进程，再执行：

```sh
agent-mail sync once <team>
```

读取官方已有的远端绑定、SQLite 映射和游标；一次最多上传和拉取各 10 条，积压时可重复执行。
命令本身不调用 Bash、Node 或 sqlite3 可执行文件。当前支持明文私信；遇到加密、成员变更、
历史保留缺口或冲突时明确报错，保留收取游标，不能把这类团队直接迁为原生同步。
`agmsgd` 的会话通知和注册/收件仍由官方提供，不要同时运行两个同步进程。

原生同步的完整联测需同时设置 `TEST_DATABASE_URL`、`AGENT_MAIL_TEST_BIN`、`AGENT_MAIL_TEST_AGMSG_DIR`（官方安装根目录）。测试仅复制脚本到临时目录，不读写实际邮箱。

## 常驻同步

```sh
agent-mail sync run <team>
agent-mail sync status <team>
```

`run` 在前台运行，Ctrl+C 停止。同一团队只允许一个原生同步操作运行。
原生发信提交后通过私有本机 IPC 立即触发上传；默认每 60 秒补查一次，覆盖脚本写入或通知遗漏。
网络错误会退避重试，`status` 实际探测进程并显示最近成功时间、游标和错误；它不代表 agent 会话在线。
运行目录位于 agmsg 安装下的 `run/native-sync/`，无需手动删除崩溃遗留锁。
