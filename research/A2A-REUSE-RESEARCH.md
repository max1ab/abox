# A2A 复用调查

核查日期：2026-11-03。仅调研，未安装、未实测候选，也未修改当前通信部署。

## 结论

A2A 有持续维护的官方 SDK、CLI，以及实际云平台集成，不属于缺少采用的协议。但“协议生态活跃”不等于“已有成熟工具能无缝接管已打开的 Codex/Claude 会话”。后者仍是本项目的核心筛选条件。

现阶段可考虑复用官方 SDK 来实现协议层；没有足够依据仅凭此次调查替换已实测跨机收发成功的 agmsg。

## 活跃度快照

以下 star 和提交日期通过 GitHub 官方 REST API 查询；“提交”指默认分支最新提交，不把其他分支 push 当成默认分支更新。

| 仓库 | Star | 默认分支最近提交（UTC 日期） | 发布情况 | 定位 |
|---|---:|---|---|---|
| [协议 A2A](https://github.com/a2aproject/A2A) | 26,403 | 10月30日 | v1.0.1，5月28日 | 规范，非运行程序 |
| [官方 Python SDK](https://github.com/a2aproject/a2a-python) | 2,225 | 11月2日 | v1.3.0，10月27日 | 协议客户端/服务端库 |
| [官方 JS/TS SDK](https://github.com/a2aproject/a2a-js) | 649 | 10月27日 | v1.3.0，9月29日 | 协议客户端/服务端库、持久存储实现 |
| [官方 Rust SDK](https://github.com/a2aproject/a2a-rs) | 104 | 11月2日 | workspace 含独立版本的 CLI/传输组件，不能把 CLI release 当 SDK release | Rust 协议类型、客户端、Axum 服务端、流式传输 |
| [官方 A2A CLI](https://github.com/a2aproject/a2a-cli) | 174 | 10月30日 | v0.3.1，10月16日 | 查询 Agent Card、发消息、跟踪任务；有 Windows 发行入口 |
| [a2a-adapter](https://github.com/hybroai/a2a-adapter) | 104 | 11月3日 | v0.2.16，11月3日 | 包装 Claude/Codex 等运行时；不是接管已打开的交互会话 |
| [toli](https://github.com/arjunvlama/toli) | 1 | 10月22日 | v0.1.2，10月22日；含 Windows x64/ARM64 包 | MCP 接本机 agent、A2A 跨机，规模很小 |

均未归档。官方 Python 最近10次提交跨10月27日至11月2日，JS 跨10月14日至27日，CLI 跨10月16日至30日；不是仅有一个多年未更新的仓库。提交数量、star、组织支持数均不能直接换算真实生产用户量。

## 采用证据

- Linux Foundation 于2026年4月9日宣布超过150个组织支持 A2A，并声称已有企业生产部署。这是项目方披露，不是独立用户统计：[公告](https://www.linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year?hs_amp=true)。
- AWS AgentCore 有具体 A2A 服务部署、Agent Card、会话与认证接入文档：[官方文档](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-a2a.html)。
- Microsoft Foundry 提供远程 A2A agent 接入，当前文档列出正式 `a2a` 工具类型，区分旧 preview 类型：[官方文档](https://learn.microsoft.com/en-us/azure/ai-foundry/agents/how-to/tools/agent-to-agent)。
- Google ADK 提供 `to_a2a()` 和 A2A server 入口；其 Python 集成仍标注 Experimental，应保留这个限定：[官方教程](https://github.com/google/adk-docs/blob/main/docs/a2a/quickstart-exposing.md)。

这些支持“有人持续投入和采用”，不能证明所有 coding CLI 原生支持它。

## 轮子的实际边界

### 官方 SDK、CLI

[官方 SDK 清单](https://a2a-protocol.org/latest/sdk/)包括 Python、Go、Java、JavaScript、.NET、Rust。官方 SDK 可以复用协议对象、Agent Card、任务状态、流式接口等。JS SDK 还提供数据库任务存储；Rust workspace 提供客户端及基于 Axum 的服务器。

它们不是现有 Codex/Claude 会话的适配器，也不是直接兼容 agmsg SQLite 数据格式的同步引擎。使用 SDK 仍需实现任务执行和本机已有会话的映射；不能因为支持 SSE 就认为自动解决离线可靠投递、数据库同步及会话唤醒。

### a2a-adapter：不满足当前会话要求

[官方 README](https://github.com/hybroai/a2a-adapter#readme)明确：通过 `claude -p`、`codex exec` 非交互式调用；Claude、Codex 每次请求启动新进程。因此“已有本地 agent”指已安装的工具，不是用户已经打开的聊天。不能推荐为当前需求的直接替代。

### toli：方向接近，成熟度不足

[README](https://github.com/arjunvlama/toli#readme)描述每台机器一个 Go daemon，本地通过 MCP 接已有 agent，对外用 A2A，支持跨机任务和唤醒；提供 Windows 包。

但有两个重要限制：

1. 首次配置 MCP/通知后，需要启动或重启会话才能加载。不能声称给任意未配置的运行会话零重启接入。加载后收到任务唤醒是否适用于当前 CLI/Desktop 版本，尚未实测。
2. 仓库10月19日创建，1 star，最近默认分支提交10月22日，只有很短维护历史。它不是已死项目，但不符合用户对经过足够使用验证的长期适配底座的偏好。

跨机要求两端 daemon 互相可达；它不提供中心中转网络，文档建议 Tailscale 或可信网络/HTTPS 入口，拓扑与当前中心邮箱服务器不同。

## 对当前项目的判断

- A2A 值得保留为标准接口选项，不必因为担心无人采用而排除。
- 官方 JS SDK 比社区会话包装工具更有复用依据；Rust SDK 是官方项目，但使用规模明显更小，需先做协议/平台兼容验证。
- 暂无本次已确认的、大规模维护且直接覆盖“Windows/Mac、已有 Codex/Claude 会话、跨机可靠消息、自动唤醒”的一站式轮子。
- 不据此次调查启动迁移或安装。若下一步评估，应先测试候选对现有真实会话的路由/唤醒，再决定协议和存储改造。
