[English](CONTRIBUTING.md) | 简体中文

# 贡献 telepair

感谢你对 telepair 的贡献兴趣!这份指南会帮你快速上手。

## 开始之前

### 先决条件

- Rust 1.85+(edition 2024)
- Node.js 18+
- SQLite(通过 sqlx 捆绑,无需单独安装)

### 环境搭建

```bash
git clone https://github.com/telepair/telepair.git
cd telepair

# 构建后端
cargo build

# 安装前端依赖
cd web && npm install && cd ..

# 跑一遍测试,确认环境可用
cargo test --workspace
cd web && npm test && cd ..
```

### 开发流程

在两个独立终端里分别启动后端和前端:

```bash
# 终端 1:后端运行在 :7700
cargo run

# 终端 2:前端 dev server 运行在 :5173(自动把 API 代理到 :7700)
cd web && npm run dev
```

浏览器打开 `http://localhost:5173`。Vite dev server 会把 `/api` 和 `/ws` 请求代理到后端。

## 项目结构

```
telepair/
├── crates/
│   ├── telepair-core/       # 共享类型、Storage trait、协议
│   ├── telepair-agent/      # PTY 管理、虚拟目标
│   ├── telepair-control/    # 会话生命周期、目标注册表
│   ├── telepair-gateway/    # HTTP/WS 服务器、REST API
│   └── telepair-cli/        # 二进制入口
├── web/                     # SolidJS + TypeScript 前端
│   └── src/
│       ├── lib/             # API client、WebSocket、协议类型
│       ├── stores/          # 响应式状态(auth、sessions)
│       ├── pages/           # 路由页面
│       └── components/      # UI 组件
├── migrations/              # SQLite schema
└── docs/                    # 文档
```

## 代码风格

### Rust

- Edition 2024,stable toolchain(>= 1.85)
- 使用目录式模块(`foo/bar.rs`),**不要**用 `mod.rs` 风格
- 优先返回 `Result` 而不是 panic
- 提交前跑 `cargo clippy`

### TypeScript

- 开启 strict 模式
- 提交前跑 `npm run type-check`

### Commit

- 遵循 [Conventional Commits](https://www.conventionalcommits.org/) 规范:`feat|fix|chore|refactor|perf|docs|test|ci`
- 英文祈使句("add feature",不是"added feature")
- 必须带签名:使用 `git commit -s`
- 每个 commit 只做一件事(一个逻辑变更)

## 测试

```bash
# 后端:workspace 全部测试
cargo test --workspace

# 前端:单元测试
cd web && npm test

# 前端:类型检查
cd web && npm run type-check
```

- 所有新增/修改的逻辑都必须有单元测试
- 测试文件与被测代码就近放置(Rust 放 `tests/` 目录,TS 在源文件旁 `*.test.ts`)

## Pull Request

1. Fork 仓库,从 `main` 拉一个 feature 分支
2. 写代码,同时补上测试
3. 确认所有测试通过、clippy / type-check 无警告
4. 提交 PR,说明清楚"做了什么"和"为什么"

## 报告 Issue

请在 [github.com/telepair/telepair/issues](https://github.com/telepair/telepair/issues) 创建 issue,包含:

- 你期望的行为 vs. 实际发生的现象
- 复现步骤
- 操作系统、Rust 版本、浏览器版本

## 许可证

提交贡献即表示你同意你的代码以 MIT 许可证发布。
