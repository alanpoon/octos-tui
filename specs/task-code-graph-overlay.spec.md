spec: task
name: "/graph 代码图浏览覆盖层"
inherits: project
tags: [tui, capability, code-graph, navigation, overlay]
estimate: 2d
---

## 意图

octoscode 目前没有任何代码图入口：符号索引、引用查找、调用链追踪、影响面
分析这些结果只能以自由文本混在 assistant 回复里，不可导航。本任务加一个
能力门控的 `/graph` 覆盖层，把服务器 code-graph 方法的返回渲染成可上下选择
的只读行列表，选中行把 `path:line` 送进 composer 当作下一轮上下文。

## 已定决策

- 新增能力常量 `APPUI_FEATURE_CODE_GRAPH_V1 = "code.graph.v1"`；服务器未广告
  时 `/graph` 不出现在 `/` popup，直接键入则渲染 Unsupported。
- 消费四个只读方法：`graph/symbols/list`、`graph/refs/find`、
  `graph/callers/trace`、`graph/blast-radius`；四者均以 `required_methods_any`
  参与门控。
- 命令走既有 `CommandSpec` + `MenuId` 注册机制与 `Store` reducer，不新建 UI
  框架、不新增 crate 依赖。
- 覆盖层只读：选中行只把 `path:line` 文本插入 composer，绝不写文件、不发起
  编辑或工具调用。
- 单次查询最多渲染 200 行；超出部分截断，末行显示剩余条数。
- 查询失败时在覆盖层顶部渲染一行错误，保留上一次成功的结果行不清空。
- 所有新增可见文案同时落 `locales/en.yml` 与 `locales/zh.yml`。

## 边界

### Allowed Changes
- src/menu/registry.rs
- src/menu/types.rs
- src/model.rs
- src/store.rs
- src/app/render.rs
- locales/en.yml
- locales/zh.yml
- specs/**

### Forbidden
- 不在客户端做符号解析或建索引——代码图真相全部来自服务器。
- 不在能力未广告时本地伪造可用状态。
- 不从覆盖层触发任何写操作或工具调用。
- 不新增 crate 依赖。

## 排除范围

- 服务器侧 code-graph 方法的实现。
- 图形化调用图绘制。
- 跨仓库 / 跨语言的符号解析。

## 完成条件

场景: 服务器广告 code.graph.v1 时 /graph 列出符号行
  测试: graph_command_lists_symbols_when_capability_advertised
  假设 mock 服务器广告 `code.graph.v1` 且 `graph/symbols/list` 返回 3 个符号
  当 用户运行 `/graph`
  那么 覆盖层渲染 3 行符号
  并且 每行含符号名与 `path:line`

场景: 能力未广告时 /graph 不出现在命令 popup
  测试: graph_command_hidden_without_code_graph_capability
  假设 mock 服务器不广告 `APPUI_FEATURE_CODE_GRAPH_V1` 对应的 `code.graph.v1`
  当 用户打开 `/` 命令 popup
  那么 popup 条目中不含 `graph`

场景: 能力未广告时直接键入 /graph 渲染 Unsupported
  测试: graph_command_renders_unsupported_when_capability_absent
  假设 mock 服务器不广告 `code.graph.v1`
  当 用户键入 `/graph` 并回车
  那么 状态区渲染 Unsupported
  并且 不发出任何 `graph/*` 请求

场景: 查询失败时保留上一次结果并显示错误行
  测试: graph_query_error_keeps_previous_rows
  假设 覆盖层已渲染 3 行上一次成功的符号结果
  当 后续 `graph/refs/find` 返回错误
  那么 覆盖层顶部渲染一行错误文案
  并且 上一次成功的 3 行结果被保留、未清空

场景: 结果超过 200 行时截断并显示剩余条数
  测试: graph_result_truncates_beyond_row_cap
  假设 `graph/blast-radius` 返回 250 个条目
  当 覆盖层渲染该结果
  那么 渲染 200 行结果
  并且 末行文案含剩余条数 "50"

场景: 覆盖层新增文案在 en 与 zh 语言包同时可解析
  测试: graph_overlay_strings_resolve_in_en_and_zh
  假设 locales/en.yml 与 locales/zh.yml 均已加载
  当 以 en 与 zh 两个 locale 解析 `/graph` 覆盖层的全部新增文案 key
  那么 两个语言包都返回译文而非回退到 key 本身

场景: 选中行把 path:line 注入 composer 且不改文件
  测试: graph_row_selection_injects_reference_into_composer
  层级: 单元
  替身: mock 后端 graph/symbols/list fixture
  假设 覆盖层已渲染符号行 `src/store.rs:42`
  当 用户选中该行并确认
  那么 composer 文本含 "src/store.rs:42"
  并且 未发出任何写文件或工具调用请求
