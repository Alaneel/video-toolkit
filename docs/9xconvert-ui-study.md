# 9xConvert 2.2.8：界面与功能实现研究

日期：2026-10-18（Asia/Singapore）。本轮按用户更新后的明确授权，研究已安装样本的业务 JavaScript、CSS、页面结构、格式/工具注册表和相关 preload IPC；研究不再限于元数据。静态研究覆盖业务注册表与实现分支；另对已有窗口做界面观察。本文记录参考行为、视觉参数和本仓库差距，不把静态扫描等同逐功能运行验证。

## 证据与范围

源 ASAR：`/Applications/9xconvert.app/Contents/Resources/app.asar`，SHA-256 `d424d510e353b41de2bea71882d92da0170aa935213c1936e081383fd046524a`。本次仅把 `out/`、`locales/` 和 package manifest 提取到 `work/9xconvert-research/` 研究；完整 hash 清单在该目录 `research-manifest.json`，不在项目仓库。

| 本地证据 | 内容 | SHA-256 |
| --- | --- | --- |
| S1 `out/renderer/assets/index-l3MvBoNs.js` | React 页面、路由、格式/工具注册、默认设置、布局尺寸 | `8c137241e7b7529d0d379047e57b99dbce92e751c7400ddd74a9215dcbc9024b` |
| S2 `out/renderer/assets/index-DU4tTQ03.css` | 浅/深色语义颜色、字体与utility样式 | `72e1f5acd3fadd6df2688319baac5c074b3fb5c838885962b9cf7b2045612b58` |
| S3 `out/preload/index.cjs` | contextBridge 与转换、预览、历史、draft、preset API | `b8c29f9bf0d445b52fff63dab6fd433f69e08ca1e2ef49cbd076b4204eed3854` |
| S4 `out/main/index.js` | FFmpeg命令构建、队列调度、主进程IPC注册 | `58c4dfa07a8ad0f059dcc426ffb4493ddd101b3218fa8a0a3e70e57878689f68` |
| S5 `out/main/chunks/ImageConfig-CJa_MOnG.js` | Sharp/HEIC图片转换与编辑分支 | `1ea0d4c609750ba5728bbf4fe944a33ba6c0a8c1f26885bc8aef2b14e394d8df` |
| S6 `out/main/chunks/PdfConfig-BEJ1KAd4.js` | PDF结构处理/渲染/多输出 | `bbf2be1b5f03b4ef53699f5fe7add5477f8da71efd3caa17df43dd82a14890bd` |
| S7 `out/main/chunks/DocumentConfig-B4vHji2N.js` | Word、HTML、Markdown、EPUB转换 | `8327201ea9b03f6106c1c8ce64ee1694c3b544129463138b0219dd9a357492ff` |

准确 UTF-8 字节区间、完整资产清单、语义tokens、格式表、183个解释后的注册条目和功能差距在 `feature-inventory.json`。这些 S 编号是本地来源标记，不冒充 REA Evidence ID。静态语法说明实现分支存在，不能单独证明所有路径运行可达或所有转换质量可靠。

REA 使用 `analyze_javascript_application(detail:"summary")`。首次严格完整性策略因 unpacked Sharp ARM64 addon 的 declared/observed hash 不一致而拒绝；改为 `record-and-continue` 保留该矛盾执行。声明 hash `05dba5ee40609b0ac94e14e591c515b3d9bd813ff5939625ff9a4bb78eb7c628`，实际 `75dcfb15fc2b64333757c182f517b798475f8a87d88545fed649276c91566ba4`。其原因未知，不能仅凭这一点判断为恶意或断言是签名后处理。全包分析已完成：parent Evidence `ev_0360fc16d8ba702fbb96ac16defa5af85db4b9f9d721d69c0020117aee402165`，summary view `ev_8d481c27c817d7fe2c96e5a142da6f06c8d2377a89e0543f9cb5fbe53b2195fe`。扫描4,874个JavaScript文件、86,552,587 bytes文本，17个parse failures；20,879 graph nodes，21,677 edges，但graph/semantic coverage为partial，不能称完整语义恢复。共7个unpacked native integrity矛盾被标为observed-untrusted。summary与8模块page均因携带大unknown列表超过10MiB MCP传输限制；官方export工具已保存231,669,034-byte完整bundle在研究目录，未重复运行大分析。小型metadata摘录在 `work/9xconvert-rea-ui-summary.json`，记录来源ID与coverage。研究没有把超大bundle加入仓库。

## 已观察的视觉系统

- 浅色默认视觉：白色主表面 `#ffffff`、轻灰 `#fafafa`；正文 `#171717`、次级 `#525252`、说明 `#737373`、弱字 `#a3a3a3`。细线 `#f5f5f5`，强线 `#e5e5e5`。
- 品牌蓝 `#3b82f6`，hover/strong `#2563eb`，浅蓝区 `#eff6ff`。活动导航、主按钮、进度共享同一语义色。
- 各文件类别有自己的语义色：视频蓝、音频紫、图片绿、GIF橙、PDF红、文档靛、字幕琥珀、EPUB青绿。色块主要出现在小图标底、chips和卡片标记，正文仍保持中性色。
- 深色有完整变量覆盖：主面 `#202124`、次面 `#26272b`、字 `#f5f5f5`；不是单纯把整个应用染成深灰。
- 字体为DM Sans配系统回退，400/500/600/700。正文约13–14.5px，辅助11–12.5px，页标题22/26px、landing标题30px。常用圆角10/12/16px，轻卡片双阴影，大留白与有边界的工具卡并用。S1/S2。

这些是观察值，独立实现不必逐值复制。建议保持video-toolkit自己的品牌、系统字体与图标，用一致的语义色、间距层级、卡片/分栏组织提升可读性。

## 导航与页面布局

**全局导航**是88px窄rail，图标20px置48×40px圆角10px tile，11px标签在下方。顶部应用标志，底部账户/支持/版本。六个一级目的地为Home、Library、Conversions、Editors、Explore、Settings；Conversions有活动数量badge。对应路由 `/`、`/library`、`/conversions`、`/editors`、`/tools`、`/settings`。S1。

**Home**中央内容最大820px。页面顶端26px标题；主体30px意图标题、60px高圆角16px搜索框，支持Cmd/Ctrl+K；最近使用和常用flow chips；From File/From URL以340×42px分段切换。文件模式使用272px高虚线dropzone，主按钮44px高，旁边次级添加文件夹。右侧宽屏显示可调Queue栏：默认320px，最小260、最大560，带条目卡、计数、查看全部和底部storage卡。已添加文件后切换成工作列表/配置，不让空态与处理态混杂。S1。

**Editors landing**展示Video、Audio、Image、GIF、PDF、Subtitle、Documents、EPUB八个studio卡，响应式2/3/4列；每卡有彩色图标、名称、工具数、简短用途。编辑器内部使用顶栏文件名、重命名、格式/大小、删除、Guided/Advanced切换；内容区把预览、时间线、可折叠工具inspector分开。inspector默认400px（320–640），时间线可调高度。进一步核对源结构，工具栏是布局中的 `order-first` 区域，即大屏位于预览的起始侧；不能仅凭“inspector”习惯称它固定在右侧。S1。

**Explore**按类型过滤并搜索工具/flow；二级列表默认240px（168–360）。**Library**有集合/收藏、网格/列表、搜索/排序、预览、打开/定位/复制路径、转一次、编辑、移除、草稿重连原文件等入口，集合栏默认272px（190–420）。**Conversions**独立呈现活动队列而不是把全部历史塞进转换表单。**Settings**使用默认300px（200–420）分类列表，设置内容最大760px，分类覆盖外观、默认输出、命名、flows、drafts、转换、下载、通知、桌面集成、快捷键。S1/S3。

## 功能注册与实现组织

S1格式表有16种类别。静态工具数组有108条定义；另有两个数据/文档目标生成器，按源码中的明确字面列表解释出75条。因此本次记录183个工具注册条目；这不是183项经过运行验证的能力，也不等于183个互相独立的转换引擎。完整ID/类别/engine/jobKind/progress都在JSON清单。

| 家族 | 已观察的工具族 | 已观察的引擎/路径 |
| --- | --- | --- |
| Video（21静态项） | 转格式、压缩、trim、speed、volume、resize、crop、rotate/flip、FPS、reverse、抽音频/帧、mute、merge/split、字幕烧录、水印、denoise、加音轨、thumbnail | FFmpeg builder，per-file/multi-input/multi-output及time-based进度 |
| Audio（13静态项） | 转格式、压缩/bitrate、trim、speed/volume/reverse、merge/split、fade、normalize、pitch、denoise | FFmpeg |
| Image（16静态项） | 转格式、压缩、resize/crop/rotate/flip、水印、blur/sharpen/effects、metadata、stitch、images→GIF/video/PDF/DOCX | Sharp/libvips；部分FFmpeg、PDF/Word JS分支 |
| GIF（7静态项） | 转格式、压缩、resize、trim、speed、reverse、extract frames | FFmpeg |
| PDF（13静态项） | 转格式、merge/split/rotate、compress、水印/metadata、images、Word/HTML/MD/TXT/EPUB | pdf-lib结构编辑、pdfjs文本/渲染、文档render路径 |
| Subtitle（5静态项+生成转换） | 格式互转、shift timing、FPS sync、encoding、clean、TXT/CSV/JSON | 文本解析/编码转换 |
| Document/TXT/MD/HTML/EPUB（33静态项+生成转换） | 排版/文本转换、PDF/Word/图片、TOC、encoding | Markdown-it、Turndown、Mammoth、docx、BrowserWindow打印/渲染、ZIP多输出 |
| JSON/YAML/XML/CSV/LOG | 解析、互转及文本/网页/文档输出 | JS数据格式处理，独立worker操作 |

主进程不是一个统一库独自处理全部类型：它有FFmpeg命令构建器、Sharp worker、PDF/Document/Data/Subtitle config模块、convertManager、下载相关manager与持久层。preload明确暴露转换pause/resume/retry/cancel/queue hold、活动/完成分页查询、进度监听、文件预览、元数据读取、scene/silence检测、presets与draft保存/恢复/重连接口。S3与S4可通过同名字面IPC渠道交叉对应；未据此声称所有并发/恢复细节正确。

图片实现确有HEIC专门路径：`heic-convert`先解码为PNG buffer，再进Sharp；HEIF输出还存在libx265编码与AV1 HEIF fallback分支。PNG压缩有多策略选择较小结果，JPEG明确flatten白底。PDF压缩区分结构优化/嵌入图片重压/整页光栅化等路径，UI也明确表达质量代价。S5/S6。

## 默认值的上下文

默认输出设置为video MP4、audio MP3、image保留源类型、document PDF；设置中的并发值3、draft auto、保留30天、恢复file list、通知成功/失败、interface scale 1。这里的并发设置值不是对账户可用额度或实际同时运行数量的承诺。S1。

预览缓存视频明确使用H.264 veryfast/CRF28、AAC128k；预览音频MP3 128k。它们只属于preview分支，不能称为最终转换通用默认值。社交视频preset例子为1080×1920、30fps、CRF20，需要逐文件trim/crop；其他preset有各自设置。图片普通转换调用Sharp各coder默认值，压缩工具才从用户compress参数推导quality。S1/S4/S5。

## 与当前video-toolkit的实际差距

这是共享转换入口的差距；仓库原有clipper、splitter、merger、GIF/transparency、profile batches和Video Agent另行保留，不能把它们描述为整个仓库不存在。

| 领域 | 当前已实现 | 尚未共享实现/开放 |
| --- | --- | --- |
| 视频输出 | MP4/MKV/MOV/WebM；首视频与可选首音轨 | M4V/AVI/FLV/WMV/MPG；现有common已有部分映射，仍需单独验证开放 |
| 音频输出 | MP3/M4A/WAV/FLAC/OGG | 裸AAC/WMA/Opus；M4A内AAC不同于裸AAC |
| 图片 | PNG/JPEG/WebP/TIFF/GIF/BMP输入，本轮再加入HEIC/HEIF；4个输出；auto-orient、strip、JPEG白底、首帧/页 | TGA/AVIF/SVG/ICO输入；AVIF/HEIF/ICO输出；可配置压缩/resize/crop等编辑与多文件composite |
| 其他家族 | 明确拒绝而非假成功 | PDF、字幕、Word/文本/网页/数据、EPUB |
| 工作流 | 同一个Rust core服务GUI/CLI/MCP；串行批量、进度、取消、验证后无覆盖发布 | 持久队列/历史、草稿、resume/retry、并发预算、复合flows、内容预览、OS quick actions |
| 界面 | 本轮已实现浅色侧栏/工作台、搜索与快捷格式、独立队列、编辑卡片导航与设置页 | 内容预览inspector、通用Library/历史/草稿与更多studio |

本轮core agent实际完成4个HEIC/HEIF输入→PNG/JPG测试，13个输出目标保持不变；其实现仍是ImageMagick/libheif，没有复制heic-convert或Sharp业务代码。9xConvert一侧的表格均为静态观察，未做一一运行对照测试。

建议下一轮把“首页意图与文件入口、右侧队列、按类别工具入口、统一预览/参数inspector”落在现有egui结构中；先把真实已有能力收束清楚，再逐个后端扩展。新UI应只展示已经接通并测试的动作，未来PDF/字幕/文档入口明确标为计划，不显示可执行的空壳按钮。

## 本轮落地

现有 egui 桌面已改为浅色窄侧栏、圆角工作台、搜索/类别筛选、可执行预设、大拖放区、紧凑文件行与独立队列；旧编辑工具分组为卡片入口。加入文件夹递归导入（最多1,000项、跳过符号链接）及HEIC/HEIF输入。此时共享输出目标仍为13个，PDF/文档/字幕等不计入完成。主代理另外验证实际窗口和真实转换；测试记录见交付报告。
