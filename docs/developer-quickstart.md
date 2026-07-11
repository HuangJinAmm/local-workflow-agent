# 开发者快速上手

这份文档面向准备扩展 `local-workflow-agent` 的开发者，重点回答三个问题：

1. 如何新增一个本地工具
2. 如何接入一个新的 LLM Provider
3. 如何接入 MCP Server，并让智能体用到它

本文以当前仓库代码实现为准，优先讲最短路径和实际改动点。

## 1. 扩展入口总览

如果你只想快速定位入口，先看这张表：

| 目标 | 主要入口文件 |
| --- | --- |
| 新增工具 | `src/tools/mod.rs`、具体工具文件 |
| 接入 OpenAI 兼容 Provider | `src/api/providers/openai_compat_providers.rs`、`src/api/registry.rs` |
| 接入全新协议的 Provider | `src/api/provider.rs`、`src/api/providers/*.rs`、`src/api/registry.rs` |
| 接入 MCP Server | `src/core/mod.rs` 中的 `McpServerConfig`、`src/mcp/mod.rs` |
| 让 GUI/CLI 能拿到扩展能力 | `src/agent/turn.rs`、`src/ui/handler.rs`、`ToolContext` / `ProviderRequest` |

## 2. 新增一个工具

### 2.1 工具系统长什么样

本项目的工具统一实现 `src/tools/mod.rs` 里的 `Tool` trait：

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn permission_level(&self) -> PermissionLevel;
    fn input_schema(&self) -> Value;
    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult;
}
```

其中：

- `name()`：模型看到的工具名，必须稳定
- `description()`：给模型的工具说明
- `permission_level()`：声明风险级别
- `input_schema()`：JSON Schema，决定模型如何构造参数
- `execute()`：真正的工具逻辑

### 2.2 最短接入步骤

新增一个工具，通常只需要 4 步：

1. 在 `src/tools/` 新建一个文件，例如 `my_tool.rs`
2. 在文件里实现 `Tool`
3. 在 `src/tools/mod.rs` 里加 `pub mod my_tool;` 和必要的 `pub use`
4. 把工具注册到 `all_tools()`

### 2.3 代码模板

下面是一个最小工具模板：

```rust
use super::{PermissionLevel, Tool, ToolContext, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct MyTool;

#[derive(Debug, Deserialize)]
struct MyToolInput {
    text: String,
}

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str {
        "MyTool"
    }

    fn description(&self) -> &str {
        "Do something useful with the given text."
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Input text"
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let params: MyToolInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
        };

        ToolResult::success(format!("Echo: {}", params.text))
    }
}
```

### 2.4 注册工具

在 `src/tools/mod.rs` 里做三处修改：

1. 增加模块声明

```rust
pub mod my_tool;
```

2. 如果需要，对外 re-export

```rust
pub use my_tool::MyTool;
```

3. 把它加进 `all_tools()`

```rust
pub fn all_tools() -> Vec<Box<dyn Tool>> {
    vec![
        // ...
        Box::new(MyTool),
    ]
}
```

完成后，CLI 和 GUI 通过 `all_tools()` 都能拿到这个工具。

### 2.5 写工具时的几个关键约定

#### 1. 优先正确设置权限级别

常见选择：

- `PermissionLevel::None`：纯内存计算
- `PermissionLevel::ReadOnly`：读文件、读网络、查资源
- `PermissionLevel::Write`：改文件
- `PermissionLevel::Execute`：执行命令
- `PermissionLevel::Dangerous`：高风险操作

这会影响权限系统和 GUI 的确认弹窗行为。

#### 2. 通过 `ToolContext` 拿运行时能力

`ToolContext` 很重要，常用字段有：

- `working_dir`：当前工作目录
- `resolve_path()`：把相对路径解析到工作区
- `check_permission*()`：主动做权限检查
- `mcp_manager`：访问已连接的 MCP 服务
- `user_question_tx`：把问题发给前端，等待用户回答
- `config`：读取全局配置

如果你的工具要读写文件，尽量使用：

- `ctx.resolve_path(...)`
- `ctx.check_permission_for_path(...)`

这样行为会和现有工具保持一致。

#### 3. 输入解析失败要返回工具错误，不要 panic

推荐模式：

```rust
let params: MyInput = match serde_json::from_value(input) {
    Ok(v) => v,
    Err(e) => return ToolResult::error(format!("Invalid input: {}", e)),
};
```

#### 4. 返回值尽量可读

当前主路径会把工具结果作为文本回填给模型，因此：

- 结果最好是清晰的文本
- 如果是结构化结果，优先返回 pretty JSON 字符串
- 错误信息要具体，便于模型或用户继续处理

### 2.6 什么时候需要改 GUI

多数工具不需要单独改 GUI。

原因是当前 GUI 只消费 `TurnEvent`：

- 文本增量
- 工具开始
- 工具结果
- 错误 / 取消 / 完成

只要你的工具注册进 `all_tools()`，GUI 就能看到调用过程。

只有在下面两种情况下，才需要额外改前端：

1. 你想做特殊展示效果
2. 你的工具需要和用户交互

如果需要和用户交互，优先复用 `AskUserQuestion` 一类的模式，而不是自己造新通道。

### 2.7 工具接入后的自测清单

- 工具能被 `all_tools()` 返回
- `find_tool("MyTool")` 能找到
- 输入 schema 合法
- 权限行为符合预期
- 错误输入时不会 panic
- 至少补一个单元测试

## 3. 接入一个新的 Provider

本项目接 Provider 有两条路线：

1. 目标服务兼容 OpenAI 接口：走最省事的“OpenAI 兼容 Provider”路线
2. 目标服务协议完全不同：实现一个新的 `LlmProvider`

### 3.1 路线 A：接入 OpenAI 兼容 Provider

这是最推荐的方式，也是当前仓库支持最多 Provider 的扩展路线。

核心文件：

- `src/api/providers/openai_compat.rs`
- `src/api/providers/openai_compat_providers.rs`
- `src/api/registry.rs`

#### 步骤 1：在 `openai_compat_providers.rs` 增加工厂函数

参考现有的 `groq()`、`deepseek()`、`qwen()`、`ollama()`：

```rust
pub fn mycloud() -> OpenAiCompatProvider {
    let key = std::env::var("MYCLOUD_API_KEY").unwrap_or_default();
    OpenAiCompatProvider::new(
        "mycloud",
        "MyCloud",
        "https://api.mycloud.com/v1",
    )
    .with_api_key(key)
}
```

如果有特殊行为，再通过 `with_quirks(...)` 指定：

- context overflow 关键字
- 是否流式返回 usage
- 是否需要默认温度
- 是否不需要 API Key
- 是否需要特殊 reasoning 字段

#### 步骤 2：把 Provider ID 挂进 `provider_for_id()`

```rust
match provider_id {
    "mycloud" => Some(mycloud()),
    // ...
}
```

#### 步骤 3：在 `src/api/providers/mod.rs` 里导出

如果希望外部直接调用工厂函数，就加到 `pub use openai_compat_providers::{ ... }`。

#### 步骤 4：补 Provider ID 常量

在 `src/core/provider_id.rs` 里增加常量不是绝对必须，但强烈推荐：

```rust
pub const MYCLOUD: &'static str = "mycloud";
```

#### 步骤 5：补 API Key 环境变量解析

在 `src/core/mod.rs` 的 `api_key_env_vars_for_provider()` 中加入：

```rust
"mycloud" => &["MYCLOUD_API_KEY"],
```

这样 `Config::resolve_provider_api_key("mycloud")` 才能自动工作。

#### 步骤 6：必要时补 `provider_from_config()` 分支

`src/api/registry.rs` 里已经能通过 `provider_for_id()` 兜住很多兼容 Provider，但如果你的 Provider 有这些特殊情况，建议显式加分支：

- 需要处理自定义 `api_base`
- 是本地服务，无需 API Key
- 需要别名，例如 `mycloud` / `my-cloud`

### 3.2 路线 B：实现全新的 `LlmProvider`

如果目标 Provider 不是 OpenAI 兼容协议，就需要直接实现 `LlmProvider`。

核心 trait 在 [src/api/provider.rs](file:///d:/3-ai-project/local-workflow-agent/src/api/provider.rs)：

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &ProviderId;
    fn name(&self) -> &str;
    async fn create_message(&self, request: ProviderRequest) -> Result<ProviderResponse, ProviderError>;
    async fn create_message_stream(&self, request: ProviderRequest)
        -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>;
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;
    async fn health_check(&self) -> Result<ProviderStatus, ProviderError>;
    fn capabilities(&self) -> ProviderCapabilities;
}
```

#### 推荐接入步骤

1. 在 `src/api/providers/` 新建实现文件，例如 `mycloud.rs`
2. 实现 `LlmProvider`
3. 在 `src/api/providers/mod.rs` 里 `pub mod mycloud;`
4. 在 `src/api/registry.rs` 里把它接到 `provider_from_config()`
5. 如果需要运行时兜底，再接到 `runtime_provider_for()`

#### 最关键的不是网络请求，而是这三件事

##### 1. 正确声明 `ProviderCapabilities`

这会直接影响：

- 是否暴露工具调用
- 是否允许图片输入
- 是否允许 PDF / Document 输入

如果这里写错，上层可能会把不支持的内容发给模型。

##### 2. 把流式输出翻译成统一 `StreamEvent`

当前整个运行时，包括 `agent::run_turn`，吃的都是统一流事件。

所以你要做的不是把底层流原样透传，而是翻译成：

- `MessageStart`
- `ContentBlockStart`
- `TextDelta`
- `InputJsonDelta`
- `ThinkingDelta`
- `MessageDelta`
- `MessageStop`
- `Error`

##### 3. 保持 `ProviderRequest` / `ProviderResponse` 语义一致

上层已经统一了消息结构、工具定义和停止原因。新 Provider 最好适配这一套，而不是反向污染上层逻辑。

### 3.3 Provider 接入后的最小验证

至少验证这几件事：

1. `provider_from_config(config, "mycloud")` 能返回实例
2. API Key 能通过 `Config::resolve_provider_api_key("mycloud")` 读到
3. `health_check()` 能区分“没配 key”和“服务不可达”
4. `create_message_stream()` 能输出完整 `StreamEvent`
5. `capabilities()` 与真实服务一致

## 4. 接入 MCP

### 4.1 当前 MCP 集成到什么程度

当前仓库已经有完整的 MCP 客户端层：

- 配置结构：`McpServerConfig`
- 连接入口：`McpManager::connect_all()`
- 远程工具调用：`call_tool()`
- 资源读取：`list_all_resources()` / `read_resource()`
- Prompt 能力：`list_all_prompts()` / `get_prompt()`

同时，本地工具系统已经内置两个与 MCP 资源相关的工具：

- `ListMcpResources`
- `ReadMcpResource`

也就是说，最短路径下你不需要先做“远程 MCP 工具直通”，只要把服务接上，模型就已经能通过资源型工具读 MCP 暴露的数据。

### 4.2 MCP 配置长什么样

`McpServerConfig` 定义在 `src/core/mod.rs`：

```rust
pub struct McpServerConfig {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
    pub server_type: String, // stdio | sse | http
}
```

支持三种传输：

- `stdio`
- `sse`
- `http`

### 4.3 最短接入步骤

#### 步骤 1：准备 `McpServerConfig`

例如本地子进程型 MCP：

```json
{
  "name": "filesystem",
  "command": "node",
  "args": ["./my-mcp-server.js"],
  "env": {
    "ROOT": "D:/workspace"
  },
  "type": "stdio"
}
```

例如远程 HTTP MCP：

```json
{
  "name": "remote_docs",
  "url": "https://example.com/mcp",
  "type": "http"
}
```

注意：

- `type` 字段在 Rust 结构里对应 `server_type`
- 配置里的字符串支持环境变量展开，`mcp::expand_server_config()` 会处理 `${VAR}` / `${VAR:-default}`

#### 步骤 2：连接 MCP Server

```rust
let manager = crate::mcp::McpManager::connect_all(&configs).await;
```

#### 步骤 3：把 `manager` 塞进 `ToolContext`

```rust
let tool_ctx = ToolContext {
    // ...
    mcp_manager: Some(Arc::new(manager)),
    // ...
};
```

只要这一步完成，`ListMcpResources` 和 `ReadMcpResource` 就能工作。

### 4.4 当前最稳妥的 MCP 使用方式

推荐先走“资源接入”路线：

1. 让 MCP Server 暴露资源
2. 通过 `ListMcpResources` 让模型发现资源
3. 通过 `ReadMcpResource` 读取资源内容

这样好处是：

- 接入简单
- 风险较低
- 现有 `all_tools()` 已经包含相关工具
- 不需要改 `ProviderRequest.tools` 的组装逻辑

### 4.5 如果你想让模型直接调用远程 MCP 工具

当前代码里，`McpManager` 已经支持：

- `all_tool_definitions()`
- `call_tool(prefixed_name, arguments)`

也就是说，底层能力已经具备，但默认 `all_tools()` 还没有自动把“每个 MCP Server 的远程工具”展开成本地 `Tool`。

如果你想做这件事，有两条路：

#### 路线 A：写一个本地包装工具

例如写一个 `CallMcpTool`：

- 输入：`server_name`、`tool_name`、`arguments`
- 内部调用：`ctx.mcp_manager.as_ref()?.call_tool(...)`

优点：

- 接入最快
- 不用改现有工具注册机制

#### 路线 B：把 MCP ToolDefinition 动态并入模型工具列表

利用 `McpManager::all_tool_definitions()`，把每个远程工具转成前缀化工具名：

- 例如 `filesystem_read_file`
- 再在执行阶段用 `call_tool()` 路由回对应服务

优点：

- 模型体验更自然

代价：

- 需要改工具定义收集和执行调度逻辑
- 要处理名字冲突、权限和错误映射

如果只是先把 MCP 接进来，建议先做资源型接入，不要一步做太深。

## 5. 推荐开发顺序

如果你现在要扩展仓库，建议优先顺序是：

1. 新增一个简单工具，熟悉 `Tool` / `ToolContext`
2. 再接一个 OpenAI 兼容 Provider
3. 最后接 MCP Server，并先走资源型集成

这个顺序的好处是：

- 本地反馈最快
- 改动范围可控
- 最容易复用现有基础设施

## 6. 每类扩展的验收清单

### 工具

- 已注册到 `all_tools()`
- schema 合法
- 权限级别正确
- 错误输入不 panic
- 有最小单测

### Provider

- 可被 `provider_from_config()` 构造
- API Key / Base URL 解析正常
- `health_check()` 可用
- 流式事件映射正确
- `capabilities()` 准确

### MCP

- `connect_all()` 可连接
- `failed_servers()` 可定位失败原因
- `ToolContext.mcp_manager` 已注入
- `ListMcpResources` / `ReadMcpResource` 可用
- 如做远程工具直通，已覆盖名字冲突和错误处理

## 7. 建议先读哪些源码

按扩展方向分别推荐：

### 看工具

- `src/tools/mod.rs`
- `src/tools/web_fetch.rs`
- `src/tools/mcp_resources.rs`

### 看 Provider

- `src/api/provider.rs`
- `src/api/registry.rs`
- `src/api/providers/openai_compat.rs`
- `src/api/providers/openai_compat_providers.rs`

### 看 MCP

- `src/core/mod.rs` 中的 `McpServerConfig`
- `src/mcp/mod.rs`
- `src/tools/mcp_resources.rs`

---

如果只是想先做一个最小扩展，最推荐的第一步是：照着现有工具模板加一个只读工具，把它跑通。这样你会最快理解这个仓库真正的运行方式。
