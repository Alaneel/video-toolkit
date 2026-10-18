# 结构化 CLI 与有界视频反馈循环

本分支在现有 `agent-task`、`EditPlan`、FFmpeg 后端、任务锁和执行账本上增加 JSON 命令面与两种有界反馈循环：

- **离线显式候选 `iterate`**：调用者提交计划数组，真实渲染并选择首个客观通过的候选，不调用模型。
- **模型自评修订 `smart-iterate`**：获得单独上传许可后，渲染当前计划，将稀疏输出图像和原片观察交给真实 provider；模型可接受、表示不确定或提出有证据的修订，再在轮数/任务预算内渲染复评。

两种循环均不自动保存候选为 `Task.plan`、不批准或导出最终成片。模型自评不是独立人类评分、完整视频理解或效果保证；候选仍需明确采用和人工确认。

此文描述代码契约和可复现操作；测试用例及运行命令在末节，实际通过结果必须另附运行证据，不能由文档存在推定。上游版本研究、来源与设计取舍见 [CLI-Anything 研究](cli-anything-study.md)。

## 机器接口

在完整源码 checkout 中构建 `video-toolkit`，以下命令共用现有入口：

```sh
cargo build --locked --bin video-toolkit
./target/debug/video-toolkit agent-task --json discover
```

`--json` 仅作用于 `agent-task`，也可放在其子命令之后。普通成功和失败均在标准输出返回一个 `video-agent-cli/v1` JSON 对象，而不是在前面夹入 `Workspace:`、进度或原始后端错误：

```json
{
  "schema": "video-agent-cli/v1",
  "ok": true,
  "command": "discover",
  "result": {},
  "error": null
}
```

失败时 `ok=false`、`result=null`，`error` 含稳定的 `code` 和固定说明。现有代码为 `Invalid`、`Io`、`Json`、`TimedOut`、`Cancelled`、`ModelResponseRejected`。这些机器错误不回显原始文件路径、JSON 内容、provider 响应或 FFmpeg 错误。成功退出码为 0，运行/安全检查失败为 1，参数解析错误为 2；参数错误使用 `command="parse"`。`--help`/`--version` 保留标准帮助行为，不承诺 JSON envelope。旧 Toolkit 命令不因此获得全局 `--json`。

注意两个不同的成功层次：`ok=true` 代表命令完成，不一定代表 `result.objective_pass=true`。例如一次成功渲染可给出“时长不符合目标”的客观失败结果。这正是显式候选循环能够继续的条件。

成功的任务响应可能含本机源路径、意图及其他私有记录；单 envelope 和错误脱敏不是任务数据匿名化。不要直接把完整 `show`、trajectory 或任务目录上传。

## 命令与返回值

`WORKSPACE` 是已有任务目录，不是整个仓库；创建任务时由返回的 `result.workspace` 获取。

| 命令 | 行为与主要结果 |
|---|---|
| `discover` | 返回 `video-agent-capabilities/v1` 能力声明和外部依赖说明。不打开任务、不调用 FFmpeg、不探测安装、不安装 Hub。`dependency_installation_checked=false`。 |
| `validate WORKSPACE [--plan FILE]` | 验证当前或指定 `EditPlan`，返回 digest、revision、clip_count、计划/目标时长及 `sources_verified`。不保存或批准计划；`semantic_evaluation="not_evaluated"`。 |
| `preview WORKSPACE` | 渲染当前保存计划，返回 `video-agent-preview/v1` bundle，包含任务内相对路径、MP4 digest 和客观报告；无需最终成片批准。 |
| `quality WORKSPACE` | 重新核对最新预览的计划、媒体 hash、源素材及完整解码，返回 `video-agent-objective-quality/v1`。不消耗新的渲染次数，但会使用执行预算并追加轨迹事件。 |
| `preview-latest WORKSPACE` | 读取最新预览的历史记录，未捕获时可返回 null；不重新验证源或媒体。 |
| `trajectory WORKSPACE` | 读取 `video-agent-trajectory/v1`，含任务绑定、持久尝试数、历史事件及最新 head；同样不是新鲜媒体证明。 |
| `iterate WORKSPACE --candidates FILE [--max-attempts 1..3]` | 先尝试当前计划，再顺序尝试显式候选；选首个客观通过者后停止。返回每次 bundle、停止原因和可选 `selected_plan`；任务计划保持不变。默认最多 3 次，包括当前计划。 |
| `smart-iterate WORKSPACE [--max-rounds 1..3] --consent-preview-upload` | 当前计划与模型提出的后续修订共计最多3轮，每轮为真实预览与稀疏图像 review。默认3轮；单独同意缺失时，在打开任务和 provider 配置前拒绝。 |
| `smart-status WORKSPACE` | 读取 `video-agent-smart-feedback/v1` 的持久轮次、review、停止原因、可选候选及事件；不配置 provider、不调用模型、不重新渲染或证明文件仍未改变。 |
| `smart-adopt WORKSPACE --run-id ID` | 明确采用对应完成 run 的已选候选；重新核对源/预览/计划和提出者证据。真正的新模型候选保存为新 revision 并撤销旧批准，仍不批准或导出。 |

`validate --plan`、`edit --plan` 和 `iterate --candidates` 从普通、非符号链接的 JSON 文件读取，输入上限为 **65,536 字节**，不是 stdin 或可执行脚本。`iterate` 的文件必须是 `Vec<EditPlan>`，即 JSON 数组，不是 envelope 或任务 JSON。最多 `max_attempts - 1` 个候选，每个候选 revision 必须与当前保存计划一致；队列先通过领域校验再开始渲染。

`EditPlan` 使用整数毫秒和半开区间 `[start_ms,end_ms)`；包含 `revision`、`clips`、`summary`、`audio_policy`。片段引用任务已有 `source_id`，不带文件路径；音频策略为 `preserve_with_silence` 或 `mute`。应从当前计划生成候选，保留真实的 reason、uncertainty 和 evidence provenance，不伪造观察证据或把手动区间标成模型选择。

## 可复制的零模型演示

下面使用 **合成画面/音频**，只演示客观反馈和显式候选，不证明视觉语义质量。需要 FFmpeg、ffprobe，以及仅用于操作 JSON 的 `jq`。示例不会读取 API key、调用模型或使用已有用户任务。请在项目根目录运行；保留输出目录供检查。

```sh
set -eu
VTK_BIN="$PWD/target/debug/video-toolkit"
VTK_DEMO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/video-feedback-demo.XXXXXX")"

# 自建测试素材，不修改任何现有原片。
ffmpeg -v error -nostdin -n \
  -f lavfi -i 'testsrc2=size=320x180:rate=30:duration=3' \
  -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=3' \
  -c:v libx264 -preset ultrafast -pix_fmt yuv420p -threads 1 \
  -c:a aac -shortest "$VTK_DEMO_DIR/source.mp4"

"$VTK_BIN" agent-task --json create "$VTK_DEMO_DIR/source.mp4" \
  --workspace-root "$VTK_DEMO_DIR/tasks" \
  --intent 'MANUAL synthetic feedback demo; no semantic or model-quality claim' \
  --target-seconds 2 > "$VTK_DEMO_DIR/create.json"
VTK_TASK="$(jq -er '.result.workspace' "$VTK_DEMO_DIR/create.json")"

"$VTK_BIN" agent-task --json manual-draft "$VTK_TASK" \
  > "$VTK_DEMO_DIR/draft.json"

# 手动构造只有1秒的当前计划，故意不满足2秒目标。
jq '.result.task.plan | .clips[0].end_ms = 1000' \
  "$VTK_DEMO_DIR/draft.json" > "$VTK_DEMO_DIR/short-plan.json"
"$VTK_BIN" agent-task --json edit "$VTK_TASK" \
  --plan "$VTK_DEMO_DIR/short-plan.json" > "$VTK_DEMO_DIR/edited.json"

"$VTK_BIN" agent-task --json validate "$VTK_TASK"
"$VTK_BIN" agent-task --json preview "$VTK_TASK" \
  > "$VTK_DEMO_DIR/preview.json"
jq '.result.quality | {objective_pass, semantic_evaluation, checks}' \
  "$VTK_DEMO_DIR/preview.json"
"$VTK_BIN" agent-task --json quality "$VTK_TASK"
"$VTK_BIN" agent-task --json preview-latest "$VTK_TASK"

# 从保存后的实际revision构造候选数组，不改其手动来源说明。
jq '[.result.task.plan | .clips[0].end_ms = 2000]' \
  "$VTK_DEMO_DIR/edited.json" > "$VTK_DEMO_DIR/candidates.json"
"$VTK_BIN" agent-task --json iterate "$VTK_TASK" \
  --candidates "$VTK_DEMO_DIR/candidates.json" --max-attempts 2 \
  > "$VTK_DEMO_DIR/iteration.json"
jq '.result | {stopped_reason, selected_plan, needs_human_review, task_plan_unchanged}' \
  "$VTK_DEMO_DIR/iteration.json"
"$VTK_BIN" agent-task --json trajectory "$VTK_TASK"
```

这个流程先捕获一次 1 秒预览，然后 iterate 再试当前的 1 秒计划和显式的 2 秒候选，最多共使用三次任务预览。预期的算术差异是 `target_duration_alignment`；实际结果仍应检查 JSON，而非只看退出码。若媒体工具、编码器、任务状态或预算不满足条件，命令失败，不替换为模拟产物。

### 离线候选的人工采用

`selected_plan` 例如 `previews/03-<digest>/plan.json`，是**相对任务目录**的路径，不是相对当前工作目录。它仅表示首个客观通过的候选。先审阅对应的 `attempts[].media_path` 视频与候选内容，再明确应用：

```sh
VTK_SELECTED="$(jq -er '.result.selected_plan' "$VTK_DEMO_DIR/iteration.json")"
jq . "$VTK_TASK/$VTK_SELECTED"

# 仅在人工决定采用后执行；edit会生成新revision并撤销旧批准。
"$VTK_BIN" agent-task --json edit "$VTK_TASK" --plan "$VTK_TASK/$VTK_SELECTED"
"$VTK_BIN" agent-task --json show "$VTK_TASK"
```

若任务计划在 iterate 后被其他操作修改，保存候选将因 revision 不匹配而拒绝；不要强改候选 revision 来绕过。重新加载、检查和生成候选。

最终批准与导出继续使用原有、独立的人工关口：审阅 `show` 中当前计划及 `result.plan_digest` 后，显式执行 `approve WORKSPACE --plan-digest DIGEST`，再执行 `export WORKSPACE`。上面的可复制流程故意不自动批准或导出；真实用户素材应在原生界面或可用播放器中检查内容和音轨。不能把选中的 what-if 预览直接改名当最终成片。

## 真实 provider 的预览自评修订

### 图像与判定的范围

当前生产 review 提示版本为 `video-agent-preview-review-v2`。每次最多发送 **6 张真实渲染输出 JPEG + 8 张任务已有的原片 JPEG**，并包含意图、当前候选计划、源身份/尺寸/时长和客观报告。它们是有标签、hash 和时间记录的实际图像，不是文件路径暗示或模型“记住了”未发送帧。程序不附带原片路径、原视频、音频或密钥，但图像、意图和计划自由文本本身仍可能含敏感信息，需在同意前检查。Agent 命令中只有原有 `analyze` 和这个明确授权的 `smart-iterate` 调用云模型，其他本地反馈命令不会替代它们偷偷联网。

输出图像的 `output_time_ms` 来自输出视频的实际解码时间；其 `source_time_ms` 只是根据剪辑时间线映射出的**名义原片位置**，不是重新实测的原片 PTS。原片 JPEG 则沿用任务实际解码得到的 PTS。模型可以用输出图像指出问题，但修订片段只能引用**本次确实发送、属于该 source 且实际时间位于 `[start_ms,end_ms)` 内的原片 sample ID**，不能把输出预览图或未发送的历史 sample 当作原片选片依据。

v2 为所有符合条件的原片 sample 以同一套算术生成可选合法时间窗口，说明证据锚点不是必须裁切的边界：一个区间内有合法的已发送 source anchor 即可，不要求引用全部图像，也不应机械裁成最早/最晚锚点的跨度。窗口是时长/边界的算术示例，**不是相关性标注、视觉排序器或预先选定答案**。模型仍可提出其他合法范围、选择少于目标时长或返回不确定；不能据一个锚点声称看过整个区间。

review 只有 `accept`、`revise`、`uncertain` 三类。`revise` 需有问题说明、相关证据和合法非空替换片段，只有改说明、而有序 source/time 范围不变时按无编辑进展停止；不自动反复缩短片段。`accept` 不等于人工接受：目标时长是偏好，保留片段可能比目标短，`target_duration_alignment` 警告不会被擦除；其余客观检查失败不能被模型覆盖。`uncertain`、无进展、到达轮数上限或安全错误都不伪造成已选成片。最后一轮才新提出、尚未渲染复评的修订不会成为 `selected_plan`。

确定性 `quality` 的 `semantic_evaluation` 继续是 `not_evaluated`；smart 报告另标为 `model_self_assessment_of_sparse_frames_not_human_or_full_video_evaluation`。两者不能合并成“视觉质量通过”。模拟 provider 用例与真实 OpenAI 调用分开记录；界面将非生产来源明确标为模拟，不冒充真实模型作品。

### CLI：先观察，再单独同意发送预览

以下复用上例自己生成的合成任务和变量；已有真实任务也可以使用，但必须先确认你有权向 provider 发送其中的图像与意图。`sample` 是本地操作；**带 `--consent-preview-upload` 的命令会发送上述数据并使用配置的真实 API**，不要为阅读文档而直接执行。需要在本机私下配置 provider 凭据，缺失时拒绝，不回退为模拟。

```sh
VTK_SMART_TASK="$VTK_TASK"
"$VTK_BIN" agent-task --json sample "$VTK_SMART_TASK"
"$VTK_BIN" agent-task --json smart-status "$VTK_SMART_TASK"

# 只在你明确同意上传这些合成/自有素材的稀疏图像后执行。
"$VTK_BIN" agent-task --json smart-iterate "$VTK_SMART_TASK" \
  --max-rounds 3 --consent-preview-upload > "$VTK_DEMO_DIR/smart.json"
jq '.result | {run_id, phase, max_rounds, stopped_reason, selected_plan,
  needs_human_review, task_plan_unchanged, matches_current_plan,
  rounds: [.rounds[] | {round, verdict: .review.verdict,
    objective_pass: .preview.quality.objective_pass}]}' "$VTK_DEMO_DIR/smart.json"
```

此例演示的是 API/状态流程，不承诺该意图会得到 `accept` 或某段好看的视频。非零退出时先用 `smart-status` 检查已持久状态，不循环重发 API，不删除计数或扩预算。

同一 base-plan digest 的 run 固定初次的 `max_rounds`；恢复时必须使用相同轮数。completed 的相同 run 返回缓存结果，不因重入、更新提示版本或点击按钮再发送请求；重开任务不会重置 Task 预算。比如成功取得上述报告后，可按其固定值显式恢复：

```sh
VTK_SMART_ROUNDS="$(jq -er '.result.max_rounds' "$VTK_DEMO_DIR/smart.json")"
"$VTK_BIN" agent-task --json smart-iterate "$VTK_SMART_TASK" \
  --max-rounds "$VTK_SMART_ROUNDS" --consent-preview-upload
```

需要新的开发验证时明确新建任务，并保留旧 run；不要删 journal 逼迫已经完成或费用未知的调用重发。

### 模型候选不能通过普通 edit 冒充人工片段

smart 的 `selected_plan` 也是相对任务的产物路径，但**不能照搬离线示例，直接普通 `edit` 以绕过模型来源校验**。先在原生界面的安全播放入口重新验证并审阅候选；只有决定采用后，明确执行：

```sh
# 仅当报告确实有 selected_plan，且你审阅并决定采用后执行。
jq -e '.result.selected_plan != null' "$VTK_DEMO_DIR/smart.json"
VTK_SMART_RUN="$(jq -er '.result.run_id' "$VTK_DEMO_DIR/smart.json")"
"$VTK_BIN" agent-task --json smart-adopt "$VTK_SMART_TASK" --run-id "$VTK_SMART_RUN"
"$VTK_BIN" agent-task --json show "$VTK_SMART_TASK"
```

adopt 核对的是实际**提出该修订的** review call，而不是只凭后续 accept call；保留已发送原片证据、reason/uncertainty 和模型来源。新候选变成新 revision，旧批准撤销；若原计划本身被 accept、内容完全未变，则不制造一份“AI 新计划”或伪造新的 AI 出处。无选中候选、run ID 不符、计划已改、源变化、媒体被改或已无法重验时拒绝。采用后再次查看当前 digest、人工批准，最后独立 `export`；任何模型判定和旧 digest 都不替人完成这些步骤。

### 原生界面的独立反馈区域

用现有原生应用打开任务：`video-toolkit agent --workspace WORKSPACE`。在人工审阅/结果页面的 **AI preview feedback** 区域：

1. 先生成原片 storyboard 并保存/放弃未保存的人工作业；没有 source samples、源失效或执行预算耗尽时不启动反馈。
2. 单独勾选预览上传许可。原先的 `cloud_consent` 不会自动勾上它；切换任务也不会沿用许可。
3. 新 run 默认最多2轮；先加载同 base 的已有状态时，恢复它原先固定的1–3轮，按钮显示实际值。后台任务和既有取消按钮控制渲染/请求，不阻塞主界面；取消不是 provider 零费用保证。
4. 查看每轮客观检查、模型 verdict/summary、问题证据和建议区间。**Load saved feedback status** 只是历史读取，不发图，不会丢弃未保存的人工编辑。
5. **Re-check & play latest AI preview** 经核心重新核验指定 run 的最后预览再用固定 ffplay 播放；UI 不打开模型提供的任意路径。base 已变时禁用播放/采用。
6. **Adopt reviewed candidate · still needs approval** 是独立的明确写入动作；不会自动点击或接着批准/导出。采用新候选后回到现有保存计划的人工审阅/确认流程。

没有新增后台 watch、HTTP server、文件自动轮询、完整时间线或任意编辑器控制。GUI 使用的是同一套 Rust 编排和安全边界，不是另一个平行 Agent。

## 预览与质量检查范围

预览复用现有固定媒体策略：H.264 MP4、640×360 letterbox，片段顺序硬切；保留音频时使用第一音轨并为无音轨片段补静音，或按计划静音。它不是原片质量直通，也没有新加字幕、转场、任意滤镜或 GUI 时间线。当前报告的检查项为：

- `plan_structure`：已有源、时间范围与音频策略校验接受。
- `full_decode`：视频/音频流通过 FFmpeg 完整解码；解码失败不会产生通过报告。
- `duration_matches_plan`：观测时长与计划时长差不超过 `max(150ms, 计划时长的1%)`。
- `preview_dimensions`：640×360。
- `audio_policy`：音轨是否存在与计划策略相符；不是音质或静音能量评估。
- `target_duration_alignment`：计划与目标差不超过 `max(250ms, 目标时长的5%)`；纯算术，不代表选片相关性。

`objective_pass` 是这些检查的合取；`matches_current_plan` 另行表示该预览是否仍对应当前保存计划，并非语义质量。此客观报告始终保留 `semantic_evaluation="not_evaluated"` 和 `needs_human_review=true`；smart 的稀疏模型自评单独保存，不重写这些字段。代码输出固定编码参数，不代表报告已经独立测量帧率、画面自然度、内容相关性、音画同步或主观节奏。

`preview-latest` / `trajectory` 的 `verification_scope` 明确为历史元数据；bundle 中“捕获时已验证”也不是永久有效证明。需要再次信任时用 `quality`：它重新校验 bundle 路径、计划 digest、媒体 SHA256、源素材和完整解码。它不重新渲染；媒体被改、源发生变化或预算耗尽会拒绝。保存时的 `quality.json` 不会因历史读取自动更新。

## 有界修订、轨迹与失败恢复

离线 iterate 每次最多 **3 次**，包含当前计划；当前计划一旦客观通过，立即停止，后续显式候选不会运行。候选用完返回 `candidate_or_attempt_limit_reached`，选中后返回 `objective_checks_passed_requires_review`。只有成功渲染但客观检查未通过才继续下一份显式候选；校验、媒体、预算或取消错误直接结束。

smart 的当前计划和模型建议的后续版本共计最多 **3 轮**，不是在每轮重新获得3次免费重试。两种循环与独立 `preview` 共用每任务 **8 次**预览尝试上限；已开始的失败不退款，预取消、尚未开始的操作不消耗计数。smart 也共用 Task 的模型调用数、输入/输出 tokens、本地估算费用、总执行时长、媒体/模型 timeout 与 frame credits；没有专用的免费预算。原片 sample 的获取已按既有规则计数，重复发送已验证 source JPEG 不等于新抽帧；每份新输出观察在抽取前先预约共享 frame credit，失败/取消保留预约，匹配同一观察 journal 后可复用或核对再生，不清零计数。

预览在渲染前要求 1–32 个片段、**计划总时长不超过 30 秒**；仍受任务已有单片段/输出时长限制。保存后的美化 JSON 计划（含末尾换行）也不超过 **65,536 字节**，保证自己生成的 `plan.json` 能被重新读取。无效队列在任何预览计数或渲染前整体拒绝。单个分片与合成 MP4 在产生后必须不超过 **64 MiB** 才能接受。这是前置计划边界及后验文件接受检查，**不是操作系统磁盘/CPU 配额**，也不保证临时文件从不超额或多份产物总计小于64 MiB。总执行时间、各媒体步骤 timeout、源哈希和全解码使用已有执行账本；本模块没有第二套独立的免费执行预算。现有 OS 孤儿子进程等限制继续适用，见 [既有运行边界](video-agent.md#state-cancellation-and-recovery)。

任务根的 `preview-trajectory.json` 保存尝试计数与有限事件，当前事件上限为128，事件种类为 started/published/failed/rechecked。执行前**先保存 started 轨迹和计数，再保存 task.history**，两份文件不是一个跨文件事务。新实现可在排他所有者下，从身份和顺序均有效的领先 started 记录向上补齐缺少的 Task history，并写 `preview_history_recovered`；保留全部已用次数，不推断渲染已经完成，也不把半成品认作已发布预览。只读历史查询不承担此修复写入。反向回滚、任务已有记录却缺失轨迹或无效事件仍拒绝，不能删文件换回 credit。

产物位于 `previews/<attempt>-<plan_digest>/`，包含 `preview.mp4`、`plan.json`、`quality.json`、`bundle.json` 和 `manifest.json`。目录与媒体路径采用固定的任务相对引用，拒绝被替换的符号链接和越界引用。使用任务锁、单文件原子轨迹写入及新建/无覆盖发布，源素材不被修改。中断或后续写入失败可能留下不完整目录；只有完整保存 published 状态的 bundle 才成为 latest。取消或失败保留已经开始的 attempt，不授予批准、不扩大预算；当前实现不恢复中途预览编码、不自动清理历史产物。状态可合法恢复且预算允许时可复用严格匹配的已发布预览，否则新尝试重新渲染；原有最终 export 的 verified-shard 恢复保证**不适用于 preview 的帧内恢复**。

smart 增加独立的 `smart-feedback.lock` 排他 run 锁，避免两个调用同时预约外部模型；每个 base-plan digest 对应 `feedback-runs/smart-<digest>.json`，有固定轮数、有界阶段状态机与单调事件、最多96个事件与512 KiB状态上限，使用原子写入和 hash/身份校验。`smart-feedback-head.json` 是当前引用，不是可以重置全部历史的授权。输出观察在 `feedback-frames/` 绑定 bundle digest、实际时间、图像 hash 和预约/发布 journal；原片 source 图像仍须存在于真实任务 samples 中。

模型阶段的恢复区分费用是否已知：

- `Calling`：新所有者无法证明外部调用未发生，保留预约为 unknown usage，停止且不自动重发；孤立的 Task 调用预约也不能按“没有返回”直接退款。例外是 Task 已持久记录匹配的 `feedback_not_sent`：保留 known-unsent 结算结果，不再次退款，也不误归为 unknown。
- `Received`：先持久保存实际 response/decision，再结算 Task 费用和消费决策；跨写入中断后，在 run、源、调用与观察 journal 身份/hash 仍匹配时，可复用已存结果并幂等结算，不重复 API。缺失、回滚或无效 journal 不会触发静默重发。
- `not_sent`：只有当前活跃所有者确定尚未进入 provider 时，退还 token/估算费用预约；仍保留调用尝试 credit，不把它抹成从未尝试。
- `Completed`：相同 base 的重入使用持久结果，不重新评分；取消、无进展和不确定也是应保留的停止结果，不等于成功。

这些是保守的本地账本与已完成阶段复用，不是跨文件全局事务、签名、防篡改或断电安全证明，也不是操作系统配额。不能把本地费用估算当 provider 发票或账户绝对消费上限。

manifest 参考上游 `preview-bundle/v1`，同时保留本项目的具体 bundle/quality schemas。**本次未安装 CLI-Hub 或做真实互操作验证**，不声称任意 Hub 版本可完整消费本项目 summary。

## 安全与权限

反馈命令沿用本地任务和源指纹校验，模型不控制路径、可执行文件、shell 或编码器参数；不安装外部工具、不新增遥测、不获取新凭据。预览和 iterate 不产生云请求，不调用保存计划、批准或最终导出接口，也不赋予新的云同意；正常成功处理保留既有计划、批准和最终导出。源素材失效仍按原有保护机制处理，不能据此保留无效旧批准。任务状态/计数/账本会更新，这不是无写入查询。

真实模型初次规划仍是已有 `analyze`；新增 `smart-iterate` 只在单独预览上传许可下审阅/修订。两者都需要私下配置的真实凭据，预算来自同一任务；许可不扩大模型的工具权限。不要将没有模型调用的示例或测试标为真实模型作品，也不要将合成色块的顺序检查当作语义评分。公开二进制仍受现有 [依赖许可门槛](video-agent.md#dependency-notices) 约束。

## 测试与复现

自动化测试分开覆盖 JSON 命令、真实合成媒体与明确模拟的 review provider，不要求 API key，也不接触真实用户任务。当前用例包括：

- CLI 单 envelope、能力声明不假装依赖已安装、参数/操作错误脱敏、帮助行为及 JSON 范围。
- 预取消不创建任务、iterate 参数/候选数量边界。
- 真实合成媒体的切片顺序、音轨补静音、源 hash 不变、未经批准的预览不修改计划/批准/最终导出。
- 当前计划不对齐目标时选择显式候选，始终保留语义未评估标记。
- 空/超长/片段过多时拒绝且不回退原片，媒体/源篡改的 fresh quality 拒绝。
- 持久八次尝试上限、重开和取消、被替换的管理目录/媒体符号链接。
- smart 独立上传同意、固定轮数、source/preview实际图像与证据边界、显式采用后仍需批准。
- 同一 run 单所有者、未知费用不重发、Received 已存决策复用、not_sent 与共享预算结算、原片变化时拒绝采用。
- source/rendered观察预约、preview started→Task history恢复、缺失/回滚或伪造状态拒绝。

从项目根目录运行：

```sh
cargo test --locked --test agent_cli
cargo test --locked -p video_agent --test feedback
cargo test --locked -p video_agent --test feedback -- --ignored --test-threads=1
cargo test --locked --test agent_cli -- --ignored --test-threads=1
cargo test --locked -p video_agent --test smart_feedback
cargo test --locked -p video_agent --test smart_feedback -- --ignored --test-threads=1
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

真实媒体用例使用 `#[ignore]` 分组，各 `--ignored` 命令必须显式运行；默认单元测试通过不能替代它们。smart 媒体用例的 review provider 明确标为模拟：它们证明真实渲染、证据、预算和权限边界，不是实际 OpenAI 语义质量。CI 的已有全 workspace `--tests -- --ignored` 步骤会纳入这些媒体测试，而不是只运行普通单元测试。

### 首个客观切片的历史实测：2026-10-18

环境为 macOS Apple Silicon，Rust 1.87.0；严格 Clippy 另以 Rust 1.99.0 执行。以下是加入 smart 模型修订循环**之前**的首个客观切片记录，基线为 `41f866fe0afb4d0f989dc0a5311c6665014f8da6`；保留作历史，不是当前全部 smart 改动的验收数量，也不是已发布 tag 或新 GitHub CI 结果。

- `cargo test --locked --workspace --all-targets`：145 项普通测试通过；真实媒体用例另行显式执行。
- 完整 workspace `--tests -- --ignored --nocapture --test-threads=1`：当时的35项真实媒体用例通过；随后新增的二进制 JSON 全链路用例单独显式通过，共36个不同媒体用例。新反馈测试全部使用合成素材，不调用模型。
- `python3 -m unittest discover -s scripts -p 'test_*.py' -v`：78项通过；`evals/video-agent` 同类命令：48项通过，既有真实调用记录未被改写。
- 格式检查和全 workspace 严格 Clippy 通过。现有 `block 0.1.6` 的 future-incompatibility 提示仍存在；未以更新依赖或绕过许可门槛掩盖它。

首次完整媒体运行在沙箱中被禁止读取测试子进程身份，因而停止且不计通过；核对仅操作自有进程组后，外部权限下完整重跑通过，未跳过崩溃用例。测试证明范围仍是客观媒体与权限边界，不是语义质量、跨平台运行或公开二进制发布批准。

### 本轮完整本地验证：2026-10-18

主执行者在全部 smart 改动上完成以下检查；它们不是上面的首个客观切片历史统计，也不是新 GitHub CI 或跨平台验收：

- `cargo test --workspace --locked --all-targets`：179项普通测试通过，49项真实媒体测试保持 ignored 分组。
- 完整 workspace 显式 `--ignored --test-threads=1`：49个不同顶层真实媒体用例通过。输出另有3个子进程测试结果，不重复加为52项。
- Rust 1.99.0 全 workspace / all-targets 严格 Clippy、格式检查与 `git diff --check` 通过。
- `python3 -m unittest discover -s scripts -p 'test_*.py' -v`：78项通过。
- `python3 -m unittest discover -s evals/video-agent -p 'test_*.py' -v`：61项通过，含新增13项 smart 公开摘要冻结/一致性测试。它们不重调模型、不读取私有任务、不重新解码媒体，也不等于语义评分。

### 两个真实 Luna smart 开发 run：保留未接受结果

两次都使用本地生成的6秒红→蓝合成素材，人工初始计划为 `[0,2000)` ms，目标为仅选蓝色、约2秒。真实调用与实际预览的脱敏记录见 [smart 开发证据](smart-feedback-evidence.json)：记录输入/输出 tokens、本地估算费用、计划/MP4 SHA256、时间范围、prompt 版本和 provider response/request ID 是否存在，不公开原始 ID、自由文本或私有路径。运行时是基线 `41f866fe0afb4d0f989dc0a5311c6665014f8da6` 加未提交修改；v2 源文件/调试二进制 hash 另存，不能冒充一个尚不存在的提交。

- review v1：实际3次调用。从红色预览开始，模型建议 `[3267,5768)` ms（计划2.501秒，观测2.500秒），再建议 `[4767,5768)` ms（计划1.001秒，观测1.000秒）；后两轮目标时长算术检查未通过。第三次仍建议相同范围，因 `revision_has_no_edit_changes` 停止。3个 verdict 都是 `revise`；命令正常返回不等于模型接受。
- review v2：另建任务，实际2次调用。第一轮模型建议 `[4000,6000)` ms，已经真实渲染，2秒观测时长及全部客观检查通过；第二次模型响应被拒绝，以 `model_response_rejected` 停止，没有形成可接受决策。保留请求 journal 的实际发送观察，不将其说成已通过验证的 round review，也不猜测公开摘要未收录的具体拒绝原因。

这两次共5次真实调用，输入29,050 tokens、输出1,654 tokens，本地估算费用合计 `$0.00445825`，不是 provider 发票或账户消费上限。**两次都没有 accept、没有 selected_plan、没有采用/批准/最终导出；原人工计划 revision 1 未变。** 两种未接受结果均保留，不因 v2 候选通过客观检查而改写为成功。5个实际预览 MP4 均已独立重哈希；主执行者随后对5份视频/可选音轨重新完整解码，均通过。解码、时长及稀疏蓝色采样图仍不能证明所有输出帧都是蓝色，也不是人工语义评分、运动/音频理解或 held-out 成绩。

主执行者还用新版原生应用打开 v2 实际任务，加载保存状态：显示固定3轮、独立上传同意默认未勾选、响应拒绝停止原因、采用按钮禁用；fresh 预览重验通过并启动播放器，人工初始 revision 1 / `[0,2000)` ms 保持不变。随后在 VLC 原生文件选择器精确打开 v2 第二轮 MP4，实际看到蓝色画面及播放到00:01；这是独立播放观察，不是所有帧蓝色或播放器完整退出状态的证明。此走查基于实际运行观察和相符的保存元数据，不声称连续录屏；**未实际覆盖真实 accepted 候选的 GUI 采用→批准→导出路径**。该路径的模拟测试不替代真实验收。

旧的兔子素材真实调用、人工删段后的20秒成片及其历史证据保持独立，不能算作本次 smart 循环已成功。以上均是未签名的本地开发证据，没有独立审计或防篡改证明。
