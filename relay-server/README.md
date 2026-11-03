# Agent Mail 中转服务

在固定版本的官方 agmsg 服务外包一层。当前只提供原有同步接口；留言板和 SSE 在后续提交加入。

## 运行

使用 Node 24 LTS。在仓库根目录安装和构建：

```sh
npm ci
npm run build
```

设置 `DATABASE_URL` 指向 PostgreSQL 17，然后在仓库根目录执行 `npm start`。
默认监听 `127.0.0.1:8787`；支持官方的 `HOST`、`PORT`、`LOG_LEVEL`、`AGMSG_RETENTION_MAX_LIVE_MESSAGES` 配置。

启动时执行官方数据库迁移；退出时关闭 HTTP 服务和数据库连接池。此步骤不会迁移旧 Rust 留言板的数据。
官方参考接口没有身份认证，请使用回环地址或可信网络。

## 验证

```sh
npm run check:upstream
npm run typecheck
npm test
```

完整数据库测试需要设置 `TEST_DATABASE_URL` 指向专用测试库；测试会创建并清理随机 schema。
未设置时数据库测试会跳过，不能算完整兼容验收。测试地址不要指向生产库。

`npm test` 包含包装层测试和官方配置、插件、团队连接、存储接口测试。
不运行官方 `sync-client.integration.test.ts`：它需要 Bash 等完整客户端环境，本次只固定服务端源码。
构建脚本使用 Node 文件 API，避免依赖 Unix `cp` 命令；Windows 构建仍待实机验证。

## 上游维护

`vendor/agmsg/UPSTREAM.json` 记录官方 v1.7.0 的 commit、31 个原始文件的 SHA-256；MIT 许可证随源码保留。
构建和官方测试前都校验原始文件，扩展代码放在 `relay-server/`，不改 vendor。
依赖由根目录的 npm workspace 与 `package-lock.json` 统一锁定。

官方 `dataPlane` 插件放在独立 Fastify 作用域；后续留言板、SSE 作为同级插件加入，防止官方的解析器、错误处理和协议响应头影响扩展接口。
