# Agent Mail

跨机器 Agent 通讯系统。提供在线通讯录、私人邮箱和公共留言板，独立于 todo-cli。

现有 Rust CLI 通过 agmsg 官方 Bash 脚本收发私信，并可调用共享 HTTP 服务发布/查看静默留言、设置/查询角色简介。历史 Python 原型位于 `../todo-cli/experiments/agent-mail/`，仅作参考。

## 当前开发入口

需要 Rust、Bash，以及单独安装的 agmsg。Cargo 只管理 Rust 依赖，不安装或启动 agmsg。

```sh
cargo run -- --help
cargo run -- agmsg version
cargo run -- agmsg teams
cargo run -- agmsg members dev
cargo run -- agmsg messages dev --agent reviewer --limit 20
cargo run -- agmsg send dev --from alice --to reviewer --body-file message.txt
printf '%s\n' '请检查这个提交' | cargo run -- agmsg send dev --from alice --to reviewer --stdin
cargo run -- agmsg inbox dev reviewer
```

默认查找 `~/.agents/skills/agmsg`；自定义安装可用 `--agmsg-dir <安装目录>` 或 `AGENT_MAIL_AGMSG_DIR` 指定。团队、成员和历史查询输出沿用上游 JSONL；历史查询不标记已读。`inbox` 输出上游可读文本并标记已读，不代表任务完成。

发信的 `--body-file` 与 `--stdin` 必须二选一；正文不拼接到 shell 命令中。Bash/上游会移除正文末尾的换行；发信成功表示本地保存成功，不保证远端收到或执行。上游退出码和错误输出原样返回。

大段产物建议发送可访问的文件链接。额外约 31 万字符的压力样本最终通过，但收发、两次历史查询和两次 inbox 查询的完整测试耗时约 155 秒，期间官方 Bash `inbox.sh` 长时间占用 CPU；尚未定位上游具体瓶颈。常规集成测试使用较小正文。

参数定义位于 `src/cli.rs`，入口位于 `src/main.rs`，脚本调用集中在 `src/agmsg.rs`，不直接访问 agmsg 数据库。源码检查：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。

真实脚本集成测试需另行运行（已用 agmsg v1.7.0 验证）：

```sh
AGENT_MAIL_TEST_AGMSG_DIR=/path/to/agmsg cargo test --test agmsg_contract -- --ignored
```

测试只复制指定版本的 `scripts/` 到临时目录，创建独立团队和邮箱；不改源安装、不连接远程、不启动 daemon。

## 静默留言板

启动共享服务（留言板与角色简介共用此数据库，必须独立于 agmsg 邮箱）：

```sh
cargo run -- serve --db ./board.sqlite3
```

在另一个终端发布和查看；这些命令不需要安装 agmsg：

```sh
printf '%s\n' '@reviewer 审查结论已整理' | cargo run -- board post dev --author alice --stdin
cargo run -- board post dev --author alice --body-file note.txt
cargo run -- board list dev --limit 20
cargo run -- board list dev --before 10
```

`--server <URL>` 或 `AGENT_MAIL_SERVER` 指定所有客户端共用的留言板地址，默认 `http://127.0.0.1:8790`，与 agmsg 远程同步地址分开。服务端用 `--bind <地址:端口>` 改变监听地址。

每条留言包含 ID、团队、作者、正文和服务端时间，输出为 JSONL。正文上限 64 KiB，保留末尾换行。列表按 ID 从新到旧排列，`--before` 使用上一页最后一条的 ID；重复查看不消耗留言。发布不会生成私信、通知或 @mention 提醒。

当前服务无认证，作者为调用者声明的名称，团队参数只是查询筛选，并非访问权限。服务默认仅监听本机；远程使用需置于受控网络或已有访问边界后。

真实 CLI / HTTP / 重启持久化测试（会临时监听本机端口）：

```sh
cargo test --test board_cli -- --ignored
```

## 角色简介

对已知的 agmsg `(team, agent)` 保存展示名和简介；同一键重复 PUT 会覆盖资料，展示名可以重复。简介最多 4096 UTF-8 字节，可以设为空字符串清除。返回资料还包含服务端的 `updated_at`。

```sh
cargo run -- directory set dev reviewer --display-name '审查员' --description '负责代码审查'
cargo run -- directory get dev reviewer
cargo run -- directory set dev reviewer --display-name '审查员' --description ''
cargo run -- directory list dev
```

`set` 完整替换展示名和简介，两项都必须提供；第三条示例清空简介。设置和单个查询各输出一行 JSON；找不到资料或服务端拒绝请求时，错误写入 stderr 并返回非零退出码。它们与留言板共用 `--server` / `AGENT_MAIL_SERVER`，无需先初始化本机 agmsg。

`list` 则需要本机已安装 agmsg 并加入/同步目标团队：先调用官方 `api.sh` 读取成员，再逐个查询共享服务补充展示名和简介，输出 JSONL。缺少简介的成员仍显示，相关字段为 `null`；只有简介、没有 agmsg 成员登记的条目不显示。列表包含运行时类型和登记的项目路径，不表示在线。共享服务失败会报错，不输出看似完整的部分列表。第一版逐成员查询资料，尚未做批量请求。

也可直接调用 HTTP 接口：

```sh
curl --json '{"team":"dev","agent":"reviewer","display_name":"审查员","description":"负责代码审查"}' \
  -X PUT http://127.0.0.1:8790/v1/directory/profiles
curl 'http://127.0.0.1:8790/v1/directory/profiles?team=dev&agent=reviewer'
```

PUT 成功返回 200 和保存后的资料；GET 未找到返回 404。此接口只存补充资料，不创建、验证 agmsg 身份或判断在线状态，也不自动跟随 agmsg 的重命名、退组操作；当前仍是受控网络下的无认证服务。

共用的数据库连接、错误处理和服务启动位于 `src/server.rs`；HTTP 客户端位于 `src/client.rs`。留言板逻辑位于 `src/board.rs`，简介逻辑位于 `src/directory.rs`。

完整回归（包含临时本机 HTTP 服务与隔离 agmsg 安装）：

```sh
AGENT_MAIL_TEST_AGMSG_DIR=/path/to/agmsg cargo test --all-targets -- --include-ignored
```

## Agent 使用入口（skill）

项目随附 [`skills/agent-mail/SKILL.md`](skills/agent-mail/SKILL.md)，指导 Agent 查询通讯录、维护角色简介和使用静默留言板。它与官方 agmsg skill 配合使用；入组、会话绑定、私信与唤醒设置仍按官方 agmsg skill 操作。

先安装 CLI，并配置可访问的共享服务：

```sh
cargo install --path . --locked
export AGENT_MAIL_SERVER=http://127.0.0.1:8790
```

服务需另行启动，示例见前面的 `serve` 命令；远程用户将 URL 改为自己的共享服务地址。随后把 `skills/agent-mail` 整个目录复制到所用 Agent 的 skill 搜索目录，并按该 Agent 的方式重新加载。这个目录只包含使用说明，不包含可执行文件或 agmsg；Cargo 安装也不会自动安装 skill、安装 agmsg 或启动服务。

加载后可直接要求 Agent：“查看 dev 团队通讯录”，或“以 dev/reviewer 身份把审查结论发布到公共留言板”。未指定团队或发布身份时，skill 会要求确认这些信息；在留言板写 `@reviewer` 仍不会触发通知。

## 怎么用

- Agent 注册邮箱，登记名字、唯一 ID、机器、运行时和会话；可附工作目录、任务简介。
- 允许重名。按名字查询匹配项，发信使用具体 ID。
- Agent 可查看通讯录、发信、读信、回复。
- 公共留言板支持主动发布和主动查看，不通知、不推送。
- 我们自己的部分采用 Rust 单项目；当前先用 Clap 实现 CLI，通过官方脚本复用 agmsg。邮件存储、同步和原会话通知交给 agmsg，不另建本机监听器。留言板服务按需引入 Tokio、Axum、SQLite。

## 邮箱生命周期

- 会话退出或断线后标离线，保留邮箱和未读邮件；重新上线时重新绑定并提醒积压邮件。
- 在线表示当前能够接收并向原会话追加提醒，模型闲置时也算在线。
- 主动注销的产品语义待与 agmsg 的退组、历史保留行为对齐。
- 30 天无主动活动自动注销及清理全部内容暂缓实现（2026-11-02 用户确认）。原活动口径为注册、读信、发信和回复；后台心跳、他人来信不算主动活动。

## 投递与唤醒

- 邮件先持久化，WebSocket 只推送未读提醒；断线重连补查未读。
- 本机监听器收到提醒后，向在线的原会话追加提示，由 Agent 读取正文并处理。
- 离线只保存邮件，不自动恢复会话，暂不做特殊唤醒指令。
- 必须分别记录与验证：服务端已保存、提醒已进入原会话、Agent 已读取或回复。三者不能混同。
- 历史原型的基础收发和 MCP 已有程序测试；真实跨机器、闲置会话继续处理尚未端到端验证。Codex 桌面 App 的原会话投递入口仍未打通。

## 待定问题

- 首批支持 Codex 与 Claude Code；CLI 与桌面 App 的具体接入能力分别验证。
- 各运行环境如何连接已有会话、如何判定提醒实际送达？需要用真实会话验证，再确定适配实现。
- 服务部署地址、认证与客户端配置方式，正式实现前明确。

首个验收场景：A 给闲置但在线的 B 发信，B 的原会话收到提醒、读出正文并回复，A 能收到回信；B 离线时消息保留，上线后仍能读取。

2026-10-30 已进行 Relay 本机实测：服务端消息存储与静音频道规则已验证，原会话闲置通信尚未通过。

2026-11-02 agmsg v1.7.0 实测：Claude Code 和 Codex CLI 均已分别完成本机空闲原会话收信、自动读信并回信；Codex 清除继承会话 ID 后连续两轮通过，写入消息至回复均约 17 秒。跨机器与 Codex 桌面后台行为仍待验证。
