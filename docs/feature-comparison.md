# 功能对照：9xConvert 静态观察与 video-toolkit 实际覆盖

2026-10-18。参考样本为 9xConvert 2.2.8；来源 hash 与结构见 [UI 研究](9xconvert-ui-study.md) 和 [功能清单](feature-inventory.json)。参考侧记录静态声明及实现分支，我方记录已接通入口和真实测试，不把静态发现视为 9xConvert 的运行验收。

## 指标不能混比

- **参考工具注册条目**：9xConvert 业务 registry 有 108 个明文字面定义，另由两个明文字面生成器生成 75 条，共 183 个静态注册条目。条目包括编辑操作、转换入口和多文件工作流，不代表 183 种输出格式或 183 项运行验证。
- **我方媒体输出目标**：`file_conversion` 有 13 个目标枚举：四种视频、五种音频、四种图片。
- **我方操作种类**：`file_workflows` 有 15 种注册操作：九种 PDF、四种字幕、一种文档转换、一种下载。文档转换本身支持六种输出，下载又有直接文件和媒体模式；不能把操作数当成格式数。
- **全仓已有能力**：编辑、GIF、batch/profile 和 Video Agent 另有既有工作流。不能仅凭新 crate 的枚举推断全仓缺少某功能，也不能用“183”和“13”或“15”计算覆盖率。

## 格式类别对照

| 领域 | 9xConvert 静态来源 | video-toolkit 已接通能力 | 差距或边界 |
| --- | --- | --- | --- |
| 视频转格式 | S1：MP4/MKV/MOV/M4V/AVI/WebM/FLV/WMV/MPG | 共享入口实测 MP4/MKV/MOV/WebM。既有 `common::VideoFormat` 还含 AVI/FLV/WMV/TS/M4V/MPEG/VOB/OGV 映射，供 clipper/splitter/merger 选择 | 旧映射不等于统一“任意视频转换”入口；其余目标需按编码与容器验证后开放 |
| 音频转格式/提取 | S1：MP3/WAV/FLAC/M4A/AAC/OGG/WMA/Opus；S4 有编码分支 | 共享入口实测 MP3/M4A/WAV/FLAC/OGG，可提取视频首音轨 | 裸 AAC、WMA、Opus 输出尚未开放；M4A 内 AAC 不等于裸 AAC 文件 |
| 图片输入 | S1：JPG/JPEG/PNG/BMP/TIFF/WebP/TGA/AVIF/SVG/HEIF/HEIC/ICO；S5 有 HEIC 解码 | PNG/JPEG/WebP/TIFF/TIF/GIF/BMP/HEIC/HEIF；HEIC/HEIF→PNG/JPG 已真实验证 | TGA/AVIF/SVG/ICO 尚未接通；GIF/多页图片转换取首帧/页 |
| 图片输出 | S1：JPG/PNG/WebP/TIFF/AVIF/HEIF/ICO | PNG/JPG/WebP/TIFF 已真实验证 | AVIF/HEIF/ICO 输出未开放 |
| GIF | S1 独立 GIF studio 和视频互转关系 | 全仓已有视频→GIF、palette 优化、宽度/FPS/目标大小调优、GIF 逻辑背景透明和批量处理 | 尚无完整 GIF 编辑 studio；首帧图片转换不保留动画 |
| PDF | S1/S6：merge/split/rotate/compress/watermark/metadata，以及图像/文档分支 | 九种操作已接入 GUI/CLI/MCP：合并、拆分、旋转、压缩、文字水印、元数据、转图片、提取文本、图片转 PDF。已验证页数/选页、内容、PNG/JPG 实际解码及 HEIC 输入 | 无 OCR；文字提取和 PDF→文档不保留原 PDF 排版；水印不是涂黑；数字签名、复杂表单等不保证保留 |
| 字幕 | S1/SubtitleConfig：SRT/VTT/ASS/SSA/SBV/SUB/SMI/LRC、时间/FPS/编码清理及文本输出 | 四种操作已接入 GUI/CLI/MCP；八种定时格式互转，另可导出 TXT/CSV/JSON；偏移、FPS 重定时、清理与编码处理有真实测试 | 以文本/时间模型转换，不保全样式；SUB 仅 MicroDVD 文本；LRC 尾 cue 推断及拒绝条件见专项文档 |
| Word/文本/Markdown/HTML/EPUB | S1/S7：结构/文本转换、PDF/Word/图片导出、TOC | TXT/MD/HTML/DOCX/RTF/ODT/EPUB/可搜索 PDF 八类输入→TXT/MD/HTML/DOCX/PDF/EPUB 六类输出；48 个组合已跑真实引擎，另验证表格与 DOCX 图片 | 重建语义布局，不能承诺字体、分页、复杂对象或精确位置；无 OCR、文档转图片或完整交互文档编辑器 |
| JSON/YAML/XML/CSV/LOG | S1/DataConfig：解析、互转、可读输出 | 尚无通用数据文件转换后端 | 需要明确数据类型、表结构、编码与大小限制；字幕 JSON/CSV 导出不等于通用数据格式转换 |

## 编辑与工作流对照

| 功能族 | 当前全仓实际覆盖 | 剩余差距 | 证据性质 |
| --- | --- | --- | --- |
| Trim/裁切与 split | `clipper` 时间提取、`splitter` 固定竖条分割，Agent 按采纳计划剪辑拼接 | 固定竖条分割不等于时间/场景 split 或完整交互 crop 编辑器 | 本地源码/API；既有回归测试 |
| 音轨编辑 | `merger` 组合指定视频首视频流与指定音频首音轨，支持 shortest/copy codec | 尚非任意多轨 timeline 编辑 | 本地源码与既有测试 |
| 水印 | PDF 文字水印已接入共享 workflow；另有既有视频 watermark plugin 源码 | 图片/视频的统一水印入口仍未完成，不能以 plugin 存在代替接通证明 | PDF 真实页内容验证；视频 plugin 仅源码观察 |
| 图像编辑/跨类型输出 | auto-orient、strip、JPEG 白底、首帧/页；图片→PDF 已实现并验证顺序、透明背景和 HEIC | 可调压缩、resize/crop/rotate/flip、水印、blur/sharpen/effects、元数据编辑、stitch，以及统一 images→GIF/video/DOCX 未接通 | 13 目标与 PDF 真实引擎测试；参考侧 S1/S5 静态 |
| URL 下载 | GUI/CLI/MCP 接通 HTTP(S) 直接下载和媒体链接模式；后者支持 original/MP4/WebM/MP3/M4A | 站点支持依赖 yt-dlp 提取器；直接文件只验证传输和大小，不冒称内容已解码 | 本地 loopback 真实测试：重定向、逐字节保留、上限、取消、拒绝覆写、音频提取及完整解码 |
| 批处理/预设 | 既有 batch/profile；两套共享引擎都有串行队列、逐项状态、失败继续和取消；新增 15 项统一 registry | 尚无通用多步骤跨类型流程或持久恢复；已有 profile 不等同参考的全部 flow | 本地请求/registry 与实际 API/CLI/MCP 测试 |
| 历史/草稿 | Video Agent 有持久任务、计划、审阅和恢复 | 通用转换队列仍在内存，没有转换 Library、历史、断点恢复或 retry | 本地架构与 Agent 既有测试 |
| 并发 | 通用队列串行；Agent 另有有界 frame workers | 尚无通用转换并发池与统一资源预算 | 本地源码；参考侧 S1 设置/S4 调度静态 |
| GUI/CLI/MCP | 共享引擎均有三个入口；主页搜索/类别、工具卡片、操作表单、独立队列、设置；CLI 结构化输出、MCP 进度/取消和路径检查 | 尚缺丰富内容预览、完整 inspector 和桌面自动化验收 | 当前实现和真实工具测试；视觉检查不能代替全部交互验收 |
| 自然语言/AI | 既有 Video Agent 可生成并审阅视频编辑计划 | 通用文件转换自然语言规划尚未接入；未建立参考侧可比 AI Agent 证据 | 我方既有实现/测试；参考侧未知 |
| OS 集成 | 当前应用和命令行可执行本地转换，新增下载表单 | Quick Actions、tray 和系统上下文菜单桥接尚未完成 | 本地入口观察；参考侧 S3/S4 静态 |

## 验证范围与后续

`pdf_documents` 的三个真实引擎测试已通过，覆盖九种 PDF 操作、八类文档输入×六类输出及公共 `execute` 发布路径；另渲染并查看了代表性 PDF 与 DOCX 页面。字幕的公共执行测试和 CLI/MCP 黑盒测试、媒体 13 目标测试、loopback 下载测试分别记录其自身行为；这些结果不证明所有任意用户文件都保真。

CI 的常规测试不要求外部 Python 依赖。Linux 的显式真实引擎阶段安装私有 Python 环境、Ghostscript、Pandoc 和 LibreOffice；含 ImageMagick 7 HEIC 的 PDF 综合用例明确安排在 macOS，原有媒体 `real_tools` 检查继续保留。缺少已选中测试所需依赖会失败，不会静默跳过。

后续优先项是数据格式后端、持久队列/历史/恢复、更多图像格式与编辑、通用并发和自然语言规划；PDF、字幕、文档与 URL 已有可运行入口，不再列为未实现类别。操作细节见 [文件 workflows](file-workflows.md)、[字幕](subtitle-workflows.md)、[下载](url-downloads.md)。

REA 主 Evidence `ev_0360fc16d8ba702fbb96ac16defa5af85db4b9f9d721d69c0020117aee402165` 扫描 4,874 个 JavaScript 文件，存在 17 个 parse failures，graph/semantic 覆盖为 partial；另保留七个 unpacked native hash 矛盾。必要 metadata 见 `9xconvert-rea-ui-summary.json`，详细来源编号 S1–S7 见研究报告。这是参考侧静态证据的边界，不应据此宣称 183 项全覆盖。
