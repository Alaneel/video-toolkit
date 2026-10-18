# CLI-Anything 源码研究与本项目的取舍

研究日期：2026-10-18（Asia/Singapore）。上游为 [HKUDS/CLI-Anything][upstream]，本次固定审查提交为 [`34f519533bc175d2fe287ab8316b0dd99bb9cc43`][snapshot]，提交时间为 2026-09-22 10:01:28 +08:00。以下结论来自该版本的源码和文档，不把演示、注册表描述或测试数量当作本机运行证明。

本次仅在临时目录只读检查上游：未安装 CLI-Hub、未执行上游安装器或生成脚本，也未把其 HARNESS、SKILL 或脚本内容作为本项目的执行指令。本项目继续使用已有 Rust、FFmpeg、任务持久化及 Agent 安全机制；此文档不代表新增里程碑已经通过验证。

## 1. HARNESS 的七阶段究竟是什么

[HARNESS.md][harness] 是为 GUI 软件生成可供 Agent 使用的 CLI 的**开发 SOP**，不是一个现成的七步模型运行时。其准确阶段如下：

| 阶段 | 上游名称 | 对应工作 |
|---|---|---|
| 1 | Codebase Analysis | 找后端引擎、已有 CLI、数据模型、GUI 操作及撤销机制。 |
| 2 | CLI Architecture Design | 设计子命令/REPL、命令分组、持久状态和 JSON 输出。 |
| 3 | Implementation | 先实现数据层与检查命令，再做变更、真实后端、导出和会话。 |
| 4 | Test Planning | 先写 TEST.md 的单元、真实后端与多步工作流测试计划。 |
| 5 | Test Implementation | 实现单元、原生中间文件、真实后端和真实 CLI 子进程测试。 |
| 6 | Test Documentation | 记录实际测试结果、统计与覆盖缺口。 |
| 7 | PyPI Publishing and Installation | 以 Python namespace package 发布并验证安装。 |

阶段 6 与 7 之间还有 **Phase 6.5: SKILL.md Generation**，为命令生成可发现的使用说明。它不是第八个整数阶段，也不能省略为“七阶段中包含自动质量评价”。定义见 [阶段 1–3][phase123]、[阶段 4–6][phase456]、[阶段 6.5–7][phase657]。

适合本项目的是：先复用已有后端、检查先于修改、明确机器契约、分开结构测试与真实媒体测试、如实记录失败和限制。上游也专门提醒 [rendering gap][rendering-gap]：仅生成一个含特效的项目文件，再用忽略项目特效的拼接器导出，不能证明效果真正生效。

不必照搬的是 Python/Click、PyPI namespace、统一 REPL 外观和默认进入交互模式。本项目已有 Rust workspace 和原生界面，可直接扩展已有单次调用命令，无需建立第二套编排框架。

## 2. CLI-Hub、Skills 与依赖发现

### 注册表不是能力证明

Hub 的 [`registry.py`][registry] 从 GitHub Pages 获取 `registry.json` 和 `public_registry.json`，在用户目录缓存一小时，并按名称、描述、类别等进行字符串匹配。CLI 条目提供安装命令、入口、要求及 skill 路径。它解决“找到并安装哪一个工具”，不证明该工具在当前机器完成过目标任务。

[`matrix.py`][matrix-preflight] 为更大的工作流提供 capability → provider 映射；`can`、`matrix preflight` 和按 capability/recipe 范围安装已有实现。其基本可用性检查是：二进制是否出现在 PATH、Python 包是否能被发现、环境变量是否非空。某些 agent-installable provider 即使尚不可用，也会计入可补齐的覆盖范围。

因此必须区别：

- 已声明：注册表声称支持这类操作。
- 已发现：依赖路径/包/变量存在。
- 已运行：真实后端成功执行。
- 已验证：输出经过完整解码及对应的媒体属性检查。
- 已接受：人或明确范围的视觉评审确认内容与目标相符。

例如，存在 API key 环境变量不等于鉴权成功；存在 FFmpeg 不等于所需编码器可用；存在 MP4 文件不等于视频可解码或选片相关。

### Skill 的实际入口

规范源为根目录 `skills/cli-anything-<software>/SKILL.md`；Python 包保留兼容副本，REPL banner 优先提示仓库根目录版本，安装后回退至包内版本。相关设计见 [HARNESS 的 skill 定义][phase657]。[`skill_generator.py`][skill-generator] 通过 AST 读取 Click 命令元数据，并[写入规范源及兼容副本][skill-output]。Meta-skill 指向实时目录、搜索、范围预检及安装方式，见 [其工作流说明][meta-skill]。

本项目可借鉴“能力说明与实际命令保持同步”，但不需要依赖实时目录来运行本地编辑。上游安装行为、skill 获取和工作流实际执行是三个独立环节；任何远程说明都不自动获得本地执行权限。

## 3. Preview、Live Preview、Trajectory 的真实边界

[Preview Protocol][preview-protocol] 标准化的是**产物契约，而不是渲染器或视觉评价模型**。三个对象应分开：

| 对象 | 角色 | 不应误认为 |
|---|---|---|
| Bundle | 一次真实后端产生的预览快照，包含 manifest、summary 与 artifacts。 | 最终成片、完整软件模拟或语义评分。 |
| Session | 可变的当前 head，指出最新 bundle 与 live 状态。 | 所有历史都在最新目录中，或远程 GUI 视频流。 |
| Trajectory | 命令与预览产物之间的历史关联，包含指纹、原因和步骤。 | 防篡改审计日志，或每条操作都已被独立质量评审。 |

Producer 是 `cli-anything-<software> preview ...`：调用真实后端、计算指纹、产生 bundle。Consumer 是 `cli-hub previews inspect/html/watch/open`：读取已有产物并展示，**不负责渲染或发布**。这一分工见 [方法说明][preview-methodology]，Hub 命令则有[实际实现][hub-preview-cli]，不只是演示图片。

Live viewer 通过本地 HTTP 服务与浏览器轮询加载当前 session、manifest 和 trajectory；它不是逐帧 GUI streaming。`latest`、`live status` 应是便宜的状态查询，不应再次渲染。后台文件轮询只是可选能力，不必为了对齐上游而加入。

协议明确将“选取 artifacts 回填模型上下文”放在 [host/runtime 层][preview-host]，不属于通用 harness 本身。Hub viewer 没有通用的“观看 → 打分 → 修改计划”执行器。Shotcut summary 中要求检查节奏、时序和切换顺序的 next actions 是建议，不是已经完成的视觉判断。

上游 Hub preview 测试的[部分 fixture][hub-preview-fixture] 是合成图片头和 MP4 ftyp 字节，用来测试查看器结构；不能据此宣称媒体可解码或实际画面正确。本项目需另外执行真实 FFmpeg 渲染、完整解码及内容相关的验证。

## 4. 四个视频相关 harness 的源码核查

本节为固定树的静态源码核查，并经过两个研究分工交叉核对；不是本机运行认证。本次没有安装 Kdenlive、Shotcut、VideoCaptioner 或 Openscreen 来运行其 harness。上游测试代码的存在、历史 TEST.md 和当前测试能通过是不同事实。

### Kdenlive：项目结构能力强，CLI 渲染链路并未接通

其内部状态是自定义 `.kdenlive-cli.json`，可以生成 Kdenlive/MLT XML，不等于直接操纵已经运行的 Kdenlive GUI。[项目结构][kdenlive-project]包含轨道、素材、片段、转场等编辑状态。真正暴露的 [`export` 命令][kdenlive-export]只有 `xml` 和 `presets`；组的帮助文字虽然提到 render，但本版本没有接上 render 子命令。

单独的 [`melt_backend.py`][kdenlive-melt] 确实提供寻找系统 `melt`、限制 codec 和超时的真实渲染函数；[后端 E2E 测试][kdenlive-tests]也直接调用 `melt` 并检查非空输出。然而“内部辅助函数能调用”不能转写为“CLI 已提供端到端渲染”。测试文件开头“不需要 melt”的旧说明与后面的真实后端测试不一致。适合借鉴 native project/中间表示与后端分层，不适合把其 XML 输出直接当作已渲染预览。

### Shotcut：本次最完整的预览协议参考，但不是自动语义编辑器

Shotcut harness 直接管理 `.mlt` XML，包含项目、轨道、片段、滤镜、转场、合成和音量等命令。[导出实现][shotcut-render]使用 `melt` 解释 MLT 工程，`prefer_ffmpeg` 只是兼容提示，不是独立 FFmpeg 渲染器；渲染 timeout 为 3,600 秒，不能原样用作本项目的资源边界。

[`preview capture`][shotcut-capture]的 quick 配方渲染 640×360 片段，并在五处抽帧，形成 manifest、facts、warnings；抽帧失败会标记 partial。live publish 将新 bundle 关联 session/trajectory；[poller][shotcut-poller]比较项目文件指纹，变更后重渲染。这里的“反馈”是让外部人/Agent 消费实际产物，未实现语义评分或自动修改编辑计划；poller 的循环本身也没有总轮次数上限。

需要保留批判性阅读：[当前 E2E 测试][shotcut-stale-test]仍有期待 `ffmpeg-filtergraph` 的断言，而渲染实现已返回 `melt`；测试还有硬编码媒体路径。它们不能作为当前 checkout 全绿的证明。另有[真实非黑图检查][shotcut-pixel-test]，属于客观媒体信号，不是内容相关性评分。其 fingerprint 辅助函数使用路径/大小/mtime，不是源视频内容 SHA256；本项目继续保留已有内容指纹。停止 poller 从 JSON PID 发信号的模式也不作为安全范例。

### VideoCaptioner：外部功能包装，且固定树有缺模块问题

这是一层对外部 `videocaptioner` 可执行程序的包装，涵盖转录、字幕优化/翻译、烧录和 pipeline；要求 Python >=3.10,<3.13 及 FFmpeg，实际 ASR、翻译或 LLM 取决于外部后端，可能联网，并不是本仓库自带的离线模型。

固定树的 [CLI 顶层导入][vc-import]依赖 `core.review`，但[受跟踪的 core 目录][vc-core-tree]中没有 `review.py`；`synthesize.py` 也依赖它。因此不能把此固定版本标为开箱可运行。部分旧样式/布局参数被兼容层忽略，而非真正渲染。其 [E2E 测试][vc-tests]主要检查外部 backend 的 help、version、config 和参数负例；没有安装 `videocaptioner` 时跳过，不证明真实转录、翻译或字幕合成成功。另须警惕其配置可把 key 放在后端 argv，且 command/error 可能回传参数；本项目不继承这种凭据传递方式。

### Openscreen：可运行 FFmpeg 子集，不是完整 GUI/录屏自动化

Openscreen harness 使用自定义 `.openscreen` JSON，[session 格式][openscreen-session]与导出器组合执行编辑，不控制桌面 GUI 或启动屏幕录制。[实际 FFmpeg 导出路径][openscreen-render]处理分段 zoom/crop/scale、speed 的 setpts/atempo、trim、concat、纯色背景 pad 与 ffprobe。

状态和 preset 列表比实际渲染覆盖更广：导出器没有应用 annotation、shadow、blur、rounding、webcam、quality 或 GIF 状态；名为 gradient/blur 的背景映射为纯色。trim 删除全部片段后还会回退使用原片，这种静默降级不适合本项目的严格编辑计划。[预览实现][openscreen-preview]是完整 render 再取五张 JPEG 并关联轨迹，不是独立低清 proxy，也没有 live 命令。其 [E2E 测试][openscreen-tests]使用真实 testsrc+sine 检查导出、probe、时长、codec 和 JPEG，是有效的后端测试形态；它们不证明字幕/标注/语义效果等尚未应用的能力。

综合取舍：吸收 Shotcut 的产物角色、客观 facts/warnings、内容变化后新建快照与计划到预览关联；吸收 Openscreen 的小型真实媒体 fixture 测试方法。避免引入四套格式转换和依赖，避免用 metadata/preset 的存在暗示对应效果已实现。

## 5. 四项不能直接继承的安全差异

### 远程注册表安装不是受限工具调用

Hub [`installer.py`][installer-shell] 将注册表视为可信来源：命令含管道、分号、`&&` 或命令替换等符号时会使用 shell；这一路径未设置 subprocess timeout。其 [pip 安装路径][installer-pip] 根据条目执行安装，部分安装 URL 指向未固定提交的 Git 仓库。

因此不把 Hub 安装器接到本项目模型工具面，不允许编辑计划携带安装命令、shell、可执行文件或任意编码器参数。工具发现只可提示已审核能力，不应自动放大执行权限。

### 逻辑追加历史不等于持久化保证

Shotcut 的 [`append_live_trajectory`][trajectory-write] 读取整个 steps 数组后追加，再通过普通 [`open("w")` JSON 写入][plain-json-write] 重写整个文件；该路径没有原子替换或排他锁。其 [live publish][shotcut-publish] 将同一个发布时刻作为 command started/finished，不能用来证明实际后端执行耗时。

本项目应复用已经存在的任务锁、原子状态写入和预算账本。轨迹需要记录真实执行起止、失败/取消、计划与媒体 digest、修订来源和剩余预算；不把逻辑 append-only 标为防篡改或断电安全。

### 预览路径需要自己的信任边界

Hub 会根据 manifest 解析 [summary 路径][hub-summary-path]、[trajectory 引用][hub-reference-path]和 [artifact 路径][hub-artifact-path]，允许绝对路径或父目录引用，未在这些位置要求解析后仍位于 bundle 根目录。其服务默认绑定 [127.0.0.1][hub-server]，这降低暴露面，但不能代替文件路径权限校验。

本项目不直接导入并信任任意上游 bundle。输出只写入任务控制的独立目录；路径、符号链接、源文件身份和目标存在性必须验证。模型不能提供文件系统路径，不能覆盖源素材、已存在成品或凭据文件。

### 不新增默认遥测

Hub [analytics 默认使用 PostHog][hub-analytics]，可读取环境信号及 Linux 父进程命令名区分人/Agent，关闭开关是 `CLI_HUB_NO_ANALYTICS=1`。这些是其产品行为，不是本地视频反馈循环所必需的部分。本项目不安装 Hub，也不新增这类遥测、目录自动上传或未经同意的模型请求。

## 6. 本项目的吸收方式与验收边界

采用独立实现、共享现有 Task/FFmpeg 的两层闭环，而不是安装或嵌入上游 Python harness：

`结构化计划 → 校验 → 真实 FFmpeg 预览 → 客观检查 → 显式候选或有许可的模型自评修订 → 可检查轨迹 → 人工采用/批准`

CLI 的离线 `iterate` 保留调用者提交 `Vec<EditPlan>` 的模式；新增 `smart-iterate` / `smart-status` / `smart-adopt` 覆盖宿主应承担的真实模型反馈、状态与明确采用。这是本项目自己的 host/runtime 编排，不反向宣称 CLI-Hub 已有通用自动评审器。原生界面另加独立反馈区域，新 run 默认2轮、加载已有同 base run 时保持固定1–3轮；不建设第二套编辑器或后台 watcher。

本地客观检查覆盖结构、完整解码、时长、分辨率、音轨策略和目标时长对齐；不包含独立帧率检查，语义字段仍是 `not_evaluated`。smart 的 `video-agent-preview-review-v2` 在单独图像上传许可下，使用每次最多6张渲染输出 JPEG 和8张实际原片 JPEG 进行模型自评。输出实际 PTS 与按时间线推导的名义原片位置分开，修订只能引用本次确实发送的原片 sample；通用合法时间窗口仅帮助算术，不提供语义排名或标准答案。

这种稀疏模型自评仍不能证明“每帧都有兔子”“节奏舒适”或完整事件/音频理解，不是独立人类评分。模型的 accept、revise、uncertain、无进展和预算停止分别记录，不因没有选中结果就删掉开发失败。预览、模型候选、人工采用后的新 revision、精确 digest 批准和最终成片是不同对象；`smart-adopt` 才能保留实际提出者的模型出处并明确保存候选，模型本身没有采用、批准、导出或 shell 权限。

继续复用源内容指纹、无覆盖发布、任务/独立 run 锁、原子有界 journal 与现有执行预算。领先的 preview started 记录可保守向上补齐 Task history，不退 credit；模型 Calling 的未知费用不重发，Received 的已存决策可复用结算，明确 not_sent 才退 token/费用且保留调用尝试。它们是阶段恢复与账本，不是帧内编码恢复、签名防篡改、断电证明或 OS 配额。相同 base 的固定轮数/已完成 run 不因重开或提示版本更新自动获得新的 API 调用。

研究结论与实现证据分开维护：本文说明固定上游的事实和本项目取舍，不将上游演示或测试转记为本项目验收结果。具体 CLI、UI、安全与恢复契约及首个真实 Luna smart run 的未达接受结果见 [反馈闭环使用说明](video-agent-feedback.md)。其中145项普通/36项不同真实媒体是首个**客观切片的历史记录**，不作为新增 smart 功能的最终计数；本轮完整验证和可复核证据由主执行者据实际结果补录，本文不提前填通过结论。

## 7. 许可证与发布约束

固定上游 checkout 的根目录及 plugin 均提供 Apache-2.0 LICENSE；本次文件清单未发现 NOTICE。但四个视频子树没有各自独立 LICENSE 正文，且 `setup.py` 的许可分类存在差异：[Kdenlive][kdenlive-license]、[Shotcut][shotcut-license]、[Openscreen][openscreen-license]标 MIT，[VideoCaptioner][vc-license]标 GPLv3。因此不能直接宣称这四个子树或其外部程序统一、无歧义地适用根目录 Apache-2.0。

若以后复制代码、模板或其他受保护表达，应按对应文件核对版权与许可，保留许可及署名、明确修改，处理可能的 NOTICE，并先解决元数据差异。[根目录许可条件][license] 可用于逐项核对；注册表中其他软件或依赖的许可不能由仓库根许可替代。

本次**只借鉴设计与契约，不复制上游源码，不安装 Hub**，并在本文固定版本归因。不为本里程碑捆绑额外编辑器、不新增遥测、不放开现有凭据范围。现有 [依赖许可及公开二进制发布门槛](video-agent.md#dependency-notices) 继续生效；源码功能扩展不是公开安装包分发批准。

[upstream]: https://github.com/HKUDS/CLI-Anything
[snapshot]: https://github.com/HKUDS/CLI-Anything/tree/34f519533bc175d2fe287ab8316b0dd99bb9cc43
[harness]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/HARNESS.md
[phase123]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/HARNESS.md#L12
[phase456]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/HARNESS.md#L111
[phase657]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/HARNESS.md#L232
[rendering-gap]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/HARNESS.md#L348
[registry]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/registry.py#L9
[matrix-preflight]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/matrix.py#L170
[skill-generator]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/skill_generator.py#L251
[skill-output]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/skill_generator.py#L539
[meta-skill]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/skills/cli-hub-meta-skill/SKILL.md#L32
[preview-protocol]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/docs/PREVIEW_PROTOCOL.md#L64
[preview-methodology]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-anything-plugin/guides/preview-methodology.md#L11
[preview-host]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/docs/PREVIEW_PROTOCOL.md#L74
[hub-preview-cli]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/cli.py#L325
[hub-preview-fixture]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/tests/test_cli_hub.py#L214
[installer-shell]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/installer.py#L67
[installer-pip]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/installer.py#L184
[trajectory-write]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/utils/preview_bundle.py#L405
[plain-json-write]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/utils/preview_bundle.py#L253
[shotcut-publish]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/core/preview.py#L429
[hub-summary-path]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/preview.py#L56
[hub-reference-path]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/preview.py#L133
[hub-artifact-path]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/preview.py#L628
[hub-server]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/preview.py#L1800
[hub-analytics]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/cli-hub/cli_hub/analytics.py#L16
[license]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/LICENSE#L89
[kdenlive-project]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/kdenlive/agent-harness/cli_anything/kdenlive/core/project.py#L71
[kdenlive-export]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/kdenlive/agent-harness/cli_anything/kdenlive/kdenlive_cli.py#L610
[kdenlive-melt]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/kdenlive/agent-harness/cli_anything/kdenlive/utils/melt_backend.py#L67
[kdenlive-tests]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/kdenlive/agent-harness/cli_anything/kdenlive/tests/test_full_e2e.py#L584
[shotcut-render]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/core/export.py#L138
[shotcut-capture]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/core/preview.py#L38
[shotcut-poller]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/core/preview.py#L681
[shotcut-stale-test]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/tests/test_full_e2e.py#L840
[shotcut-pixel-test]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/cli_anything/shotcut/tests/test_full_e2e.py#L1069
[vc-import]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/videocaptioner/agent-harness/cli_anything/videocaptioner/videocaptioner_cli.py#L21
[vc-core-tree]: https://github.com/HKUDS/CLI-Anything/tree/34f519533bc175d2fe287ab8316b0dd99bb9cc43/videocaptioner/agent-harness/cli_anything/videocaptioner/core
[vc-tests]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/videocaptioner/agent-harness/cli_anything/videocaptioner/tests/test_full_e2e.py#L1
[openscreen-session]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/openscreen/agent-harness/cli_anything/openscreen/core/session.py#L118
[openscreen-render]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/openscreen/agent-harness/cli_anything/openscreen/core/export.py#L52
[openscreen-preview]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/openscreen/agent-harness/cli_anything/openscreen/core/preview.py#L99
[openscreen-tests]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/openscreen/agent-harness/cli_anything/openscreen/tests/test_full_e2e.py#L178
[kdenlive-license]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/kdenlive/agent-harness/setup.py#L29
[shotcut-license]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/shotcut/agent-harness/setup.py#L29
[openscreen-license]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/openscreen/agent-harness/setup.py#L26
[vc-license]: https://github.com/HKUDS/CLI-Anything/blob/34f519533bc175d2fe287ab8316b0dd99bb9cc43/videocaptioner/agent-harness/setup.py#L23
