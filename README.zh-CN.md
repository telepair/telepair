[English](README.md) | 简体中文

# telepair

**终端版 Google Docs。** 直接在浏览器里和队友实时共享你的终端会话。

telepair 是一个开源的 Web 终端协作工具。在任意机器上运行它,打开浏览器,邀请队友以不同权限查看、操作或共同驾驶同一个终端会话。

## 特性

- **实时协作** — 多人共处同一个终端会话,输出实时流式同步
- **基于角色的权限** — Owner(所有者)、Operator(操作者)、Viewer(观察者)三种角色,控制谁能输入、改窗口大小,或仅围观
- **邀请链接** — 发一个链接,让别人以指定角色加入你的会话
- **虚拟目标(Virtual Targets)** — 在 YAML 配置里把命令(SSH、psql、htop 等)注册为命名目标,一键拉起
- **内置聊天** — 终端旁边自带聊天侧栏,协作沟通不离场
- **单一二进制** — 一个可执行文件同时跑 agent、control、gateway 三个角色;集群化是已规划的 future work
- **Web 前端** — SolidJS + xterm.js,无需安装任何客户端

## 快速上手

### 先决条件

- Rust 1.85+(edition 2024)
- Node.js 18+

### 构建

```bash
# 构建后端
cargo build --release

# 构建前端
cd web && npm install && npm run build && cd ..
```

### 运行

```bash
# 启动 telepair(全角色,默认端口 7700)
./target/release/telepair --web-dir web/dist
```

首次运行会自动创建管理员用户,并把 token 打印到控制台:

```
INFO telepair: === First run: admin user created ===
INFO telepair: Admin token: <your-token>
INFO telepair: Save this token — it won't be shown again!
```

浏览器打开 `http://localhost:7700`,粘贴该 admin token 登录即可。

### 邀请协作者

1. 在 Dashboard 上点一个 target,启动会话
2. 点击顶部 **Invite**(邀请)按钮
3. 选一个角色(Operator 或 Viewer),复制生成的邀请链接
4. 把链接发过去——协作者打开即用,**无需 token、无需账号**。首次点击会自动生成一个一次性 guest 用户,其 token 会缓存在当前浏览器标签中,持续到会话结束

## 架构

telepair 是一个 Cargo workspace,由可组合的角色构成:

```
┌─────────────────────────────────────────────────┐
│                  telepair-cli                    │
│             (single binary entry point)          │
├────────────────┬────────────────┬────────────────┤
│  telepair-agent│telepair-control│telepair-gateway│
│  PTY management│  auth, sessions│  HTTP, WS, UI  │
│  virtual targets│  storage       │  API endpoints  │
├────────────────┴────────────────┴────────────────┤
│                  telepair-core                    │
│        types, traits, protocols, storage          │
└─────────────────────────────────────────────────┘
```

| Crate | 职责 |
|-------|------|
| `telepair-core` | 共享类型、Storage trait、协议定义、权限模型 |
| `telepair-agent` | 基于 portable-pty 的 PTY 拉起,虚拟目标引擎 |
| `telepair-control` | 会话生命周期、目标注册表、认证服务 |
| `telepair-gateway` | Axum HTTP/WS 服务器、REST API、静态文件服务 |
| `telepair-cli` | CLI 参数解析、初始化、服务器启动 |

### 部署形态

当前 telepair 以单节点二进制方式发布:agent、control、gateway 都跑在同一个进程里。把它们拆到不同主机上组成集群是已规划的 future work —— 详见 `crates/telepair-cli/src/main.rs` 中(当前隐藏的)角色 flag。

## 配置

### CLI 选项

```
telepair [OPTIONS] [COMMAND]

Commands:
  admin    Admin operations (token recovery, user management)
           e.g. `telepair admin show-token` prints the saved admin token

Options:
      --host <HOST>                Server bind address [default: 0.0.0.0]
      --port <PORT>                Server port [default: 7700]
      --config <PATH>              Path to config file
      --targets <PATH>             Path to targets config [default: ~/.telepair/targets.yaml]
      --web-dir <PATH>             Path to web frontend dist directory
      --allowed-origins <LIST>     Comma-separated absolute-URL CORS allowlist.
                                   Unset defaults to loopback dev origins
                                   (http://localhost:5173, http://127.0.0.1:5173).
                                   Parse failures are fatal at startup.
      --allow-any-origin           Allow any origin (Access-Control-Allow-Origin: *).
                                   Only safe in dev or behind a CORS-enforcing proxy.
                                   Mutually exclusive with --allowed-origins (wins).
```

> 丢了 admin token?执行 `telepair admin show-token` —— 它会读取 `~/.telepair/admin_token` 中缓存的 token(权限 0600,首次启动时写入一次)。

### 虚拟目标(Virtual Targets)

在 `~/.telepair/targets.yaml` 里定义命名命令:

```yaml
targets:
  - name: production-db
    display: "Production DB"
    command: psql
    args: ["-h", "db.internal", "-U", "readonly", "production"]
    env:
      PGPASSWORD: "${PROD_DB_PASS}"
    tags: [database, production]

  - name: staging-ssh
    display: "Staging SSH"
    command: ssh
    args: ["deploy@staging.example.com"]
    admin_only: true
    tags: [server, staging]

  - name: monitor
    display: "System Monitor"
    command: htop
    tags: [monitoring]
```

`${VAR}` 语法的环境变量会在启动时展开。在某个 target 上设置 `admin_only: true` 后,只有管理员能基于它创建会话——普通用户命中时会收到 `403 Forbidden`。一个默认的本地 shell target 始终可用。

### 数据目录

telepair 的所有数据放在 `~/.telepair/`:

```
~/.telepair/
├── telepair.db       # SQLite 数据库(users、sessions、participants、invites)
└── targets.yaml      # 虚拟目标定义(可选)
```

## 权限

| 能力 | Owner | Operator | Viewer |
|------|-------|----------|--------|
| 查看终端输出 | Yes | Yes | Yes |
| 向终端输入 | Yes | Yes | No |
| 调整终端大小 | Yes | Yes | No |
| 发送聊天消息 | Yes | Yes | Yes |
| 创建邀请链接 | Yes | No | No |
| 关闭会话 | Yes | No | No |

## 开发

```bash
# 后端测试(107 项)
cargo test --workspace

# 前端单元测试(112 项)
cd web && npm test

# 浏览器 E2E 测试(36 项,需要运行中的 server + Chromium)
cd web && npx playwright install chromium    # 仅首次需要
cd web && npm run e2e                        # server 自动起或复用 :7700

# 前端类型检查
cd web && npm run type-check

# 开发模式:后端 :7700,前端 :5173 带代理
cargo run                          # 终端 1
cd web && npm run dev              # 终端 2
```

E2E 测试基于 Playwright,要求前端已构建(`npm run build`),同时要么有一个已在 7700 端口运行的 telepair server,要么让它通过 `cargo run` 自动拉起。admin token 从 `~/.telepair/admin_token` 读取。

### 环境变量

```bash
# 调整日志级别
RUST_LOG=debug ./target/release/telepair
```

## 文档

| 文档 | 说明 |
|------|------|
| [架构](docs/architecture.zh-CN.md)([English](docs/architecture.md)) | Crate 结构、数据流、广播通道、安全模型 |
| [REST API](docs/api.zh-CN.md)([English](docs/api.md)) | HTTP 端点参考,含请求/响应示例 |
| [WebSocket 协议](docs/protocol.zh-CN.md)([English](docs/protocol.md)) | JSON 消息类型、二进制帧格式、权限校验 |
| [部署](docs/deployment.zh-CN.md)([English](docs/deployment.md)) | Systemd、Docker、nginx 反向代理、安全清单 |
| [贡献](CONTRIBUTING.zh-CN.md)([English](CONTRIBUTING.md)) | 开发环境、代码风格、测试、PR 流程 |

## 技术栈

| 层 | 技术 |
|----|------|
| 后端 | Rust、axum、tokio、sqlx (SQLite)、portable-pty |
| 前端 | SolidJS、TypeScript、xterm.js、Vite |
| 协议 | JSON over WebSocket(控制 + 协作),二进制帧(终端 I/O) |
| 认证 | 基于 token,存储时做 SHA-256 哈希 |
| 存储 | SQLite(通过 sqlx 异步) |

## 许可证

[MIT](LICENSE)
