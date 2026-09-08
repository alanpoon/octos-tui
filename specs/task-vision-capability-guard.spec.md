spec: task
name: "非视觉模型下图片挂载的显式门控"
inherits: project
tags: [tui, media, vision, capability, model]
estimate: 0.75d
---

## 意图

图片挂载目前无条件成立：只要 prompt 里有图片路径就挂进 `Message.media`，
不看当前模型能不能吃图。选中一个纯文本模型时，图片要么被服务器静默丢弃、
要么整轮报错，用户看不出发生了什么。本任务用服务器广告的模型能力做提交前
门控：明确拒绝并提示，而不是静默失败。

## 已定决策

- 视觉能力真相只来自服务器：`model/list` 条目上的可选 `vision` 布尔字段，
  客户端不做模型名猜测、不本地伪造。
- 三态处理：`vision = true` 放行；`vision = false` 拒绝本轮 media 并提示；
  字段缺失（未知）按现状放行且不提示，避免旧服务器上出现回归。
- 拒绝的粒度是 media，不是整轮：prompt 文本照常提交，被拒的 media 被清空。
- 提示文案走 `locales/en.yml` 与 `locales/zh.yml`，内容点名当前模型 id 并建议
  切换到支持视觉的模型。
- 门控在提交路径求值，不在挂载路径：切换模型后已暂存的 media 在下一次提交时
  重新判定。

## 边界

### Allowed Changes
- src/model.rs
- src/store.rs
- locales/en.yml
- locales/zh.yml
- specs/**

### Forbidden
- 不根据模型名字符串推断视觉能力。
- 不在 media 被拒时丢弃或改写用户的 prompt 文本。
- 不新增 crate 依赖。

## 排除范围

- OCR / VL 预处理降级（把图片转成文字再发）。
- 服务器侧的模型能力探测与 provider 适配。
- 自动切换到支持视觉的模型。

## 完成条件

场景: 模型广告 vision=true 时图片照常挂载提交
  测试: vision_capable_model_attaches_images
  假设 服务器 `model/list` 为当前模型返回 `vision = true`
  当 用户提交一条带图片路径的 prompt
  那么 本轮 `Message.media` 含该图片
  并且 状态栏不显示视觉能力提示

场景: 模型广告 vision=false 时 media 被拒并提示
  测试: non_vision_model_rejects_image_media_with_hint
  假设 服务器 `model/list` 为当前模型返回 `vision = false`
  当 用户提交一条带图片路径的 prompt
  那么 本轮 `Message.media` 为空
  并且 状态栏显示的提示含当前模型 id

场景: 服务器未广告 vision 字段时保持既有放行行为
  测试: unknown_vision_capability_keeps_existing_attach_behavior
  假设 服务器 `model/list` 条目不含 `vision` 字段
  当 用户提交一条带图片路径的 prompt
  那么 本轮 `Message.media` 含该图片
  并且 状态栏不显示视觉能力提示

场景: media 被拒后 prompt 文本仍照常提交
  测试: rejected_image_media_still_submits_prompt_text
  假设 服务器 `model/list` 为当前模型返回 `vision = false`
  当 用户提交文本为 "看看 ./a.png" 的 prompt
  那么 提交给服务器的文本为 "看看 ./a.png"
  并且 本轮 `Message.media` 为空

场景: 切到非视觉模型后已暂存的 media 在提交时被拒
  测试: model_switch_to_non_vision_rejects_staged_media
  层级: 单元
  替身: mock 后端 model/list fixture
  假设 用户在 `vision = true` 的模型下暂存了一张图片
  当 用户切换到 `vision = false` 的模型并提交该轮
  那么 本轮 `Message.media` 为空
  并且 状态栏显示的提示含切换后的模型 id

场景: model/list 请求失败时按未知处理并放行
  测试: model_list_failure_treats_vision_as_unknown_and_allows_media
  层级: 单元
  替身: mock 后端返回 model/list 错误
  假设 服务器对 `model/list` 返回错误
  当 用户提交一条带图片路径的 prompt
  那么 本轮 `Message.media` 含该图片
  并且 状态栏不显示视觉能力提示

场景: model/list 返回畸形 vision 字段时按未知处理并放行
  测试: malformed_vision_field_treats_capability_as_unknown
  层级: 单元
  替身: mock 后端 model/list fixture
  假设 服务器 `model/list` 条目的 `vision` 字段为字符串 "yes" 而非布尔
  当 用户提交一条带图片路径的 prompt
  那么 本轮 `Message.media` 含该图片
  并且 状态栏不显示视觉能力提示

场景: 拒绝提示文案在 en 与 zh 语言包同时可解析
  测试: vision_rejection_hint_resolves_in_en_and_zh
  假设 locales/en.yml 与 locales/zh.yml 均已加载
  当 以 en 与 zh 两个 locale 解析视觉拒绝提示的文案 key
  那么 两个语言包都返回译文而非回退到 key 本身
