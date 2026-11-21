# 9xConvert 高层技术栈研究

研究日期：2026-10-18（Asia/Singapore）。对象为本机已安装的 `/Applications/9xconvert.app`，仅作用户授权范围内的只读包元数据、依赖与文件布局观察。未启动目标、未反编译、未读取业务 JavaScript、未提取或复制专有代码、未访问账户或许可证逻辑。研究不构成软件许可或分发权判断。

## 样本身份

| 字段 | 观察值 |
| --- | --- |
| Bundle ID | `app.xconvert.desktop` |
| App version/build | `2.2.8` / `2.2.8` |
| 主程序 | `Contents/MacOS/9xconvert`，arm64 Mach-O，53,024 bytes |
| 最低 macOS | 12.0，来自 plist 与 Mach-O metadata |
| 主程序 SHA-256 | `3d29b095ca91757fc1329bf59f8fbe609b899c093aab7726febbd0a4289966bf` |
| Info.plist SHA-256 | `b4c97b9230fe35fb2417719a8e5c5aa36d3113fdec693a91c024ba19438fdb8c` |
| app.asar SHA-256（整文件） | `d424d510e353b41de2bea71882d92da0170aa935213c1936e081383fd046524a` |
| bundled ffmpeg SHA-256 | `3517542a0c4870d30bfb15267ced3bcd0d4d18e5ec9d1fda087b2841c4c6e4a2` |

## Evidence ledger

- E1，REA `inspect_plist`：`ev_5b8284a2f8270befc109b2af091d2d5fc6aa825678f81e5f9b494cccb6f344ec`。记录 app identity、版本、`NSPrincipalClass=AtomApplication`、格式关联声明。
- E2，REA `inspect_macho`：`ev_2f9e4b5e84afee525f48c871be99cabbb206cc8b2d0e6638322a2f8faa04bd96`。主程序加载 `@rpath/Electron Framework.framework/Electron Framework`，导入 `_ElectronMain` 和 `_ElectronInitializeICUandStartNode`；最低 OS 12.0，arm64。
- E3，REA `inspect_plist`：`ev_49d938bf6214470d4a38c879a80a9d5872adb351cfe6a4105f3edcb1cd1af40e`。Electron framework 的 `CFBundleVersion=41.0.3`、identifier `com.github.Electron.framework`。
- L1，本机目录与 SHA-256 只读观察：Frameworks 包含 Electron Framework、ReactiveObjC、Squirrel、Mantle，以及 Helper、Plugin、GPU、Renderer apps。Resources 包含 `app.asar`、`app.asar.unpacked`、`ffmpeg`、`app-update.yml`。
- L2，ASAR 仅索引头读取：header 长 1,586,861 bytes，header SHA-256 `9ed432261cd2785f7478c26ed0ab18d286527c3d033494556ffbe8ee87250ad7`，与 Info.plist 的 `ElectronAsarIntegrity` 声明相符。索引内 6,105 个非目录条目；根条目为 node_modules、.gitleaks.toml、e2e-suite、locales、out、package.json、resources。未读取这些业务文件内容。整文件 hash 与 header hash 有不同含义，不应直接比较为完整性矛盾。
- L3，unpacked 开源依赖的 package.json 元数据：`sharp 0.34.5`（Apache-2.0），`@img/sharp-libvips-darwin-arm64 1.2.4`（LGPL-3.0-or-later），`better-sqlite3 12.11.1`（MIT），`jszip 3.10.1`（MIT OR GPL-3.0-or-later）。只读取名称、版本与许可证声明，未读库实现或业务代码。ASAR 索引另包含 sharp native addon、libvips、canvas/Skia native addon、better_sqlite3 addon 路径。

E1/E2 的完整 REA 返回记录保存在相邻 `9xconvert-rea-evidence.json`。E3 的完整内容未额外保存，以上保留其 Evidence ID、查询路径和关键字段。L1–L3 是本次独立文件观察，不冒称 REA Evidence。

## 结论与边界

| 问题 | 状态 | 结论与证据类型 |
| --- | --- | --- |
| GUI shell 技术 | 已回答 | **观察**：Electron desktop package，E1/E2/E3 相互印证。没有必要为此启动 Ghidra 深度分析。 |
| 媒体转换依赖 | 部分回答 | **观察**：包内有 63,506,368-byte `Resources/ffmpeg`（L1）。**推断**：至少部分媒体处理可能委托 FFmpeg；本次没有运行或跟踪，不能证明调用方式、版本、编译选项或实际覆盖率。 |
| 图片处理依赖 | 部分回答 | **观察**：sharp/libvips 与 native addon metadata（L2/L3）。**推断**：它们可能承担图片转换。不能据此证明 UI→任务→输出的具体调用链。 |
| 持久化与压缩 | 部分回答 | **观察**：better-sqlite3、jszip 随包分发。**未知**：实际表结构、队列持久化策略、用途。 |
| 声明支持的类别 | 部分回答 | **观察**：plist 声明 Video、Audio、Image、Subtitles、Document 文件关联，E1。**未知**：每种格式转换质量、实际完整支持程度。 |
| 并发、进度、临时文件、输出验证 | 未研究 | 这些行为无法从高层包清单证明；本任务独立实现自己的安全策略，不继续追踪原产品。 |
| 隐私与离线性 | 未研究 | 不把“本地可执行依赖”当作完全离线、无遥测或安全审计证明。未执行运行时与网络观察。 |
| 付费、UID、许可证 | 范围外 | 没有研究、绕过或修改。 |

REA 现有 MCP 已足以完成高层目标。`open_binary` 将 app bundle 解析为其 native launcher；随后使用目标元数据工具，避免启动 Ghidra、进行 JS graph 重建或大规模分析。首次目标未打开时的 `inspect_plist` 返回 `target_unavailable`，目标打开后成功；未进行环境或配置修改。完成后已 `close_binary`。

## 对 video-toolkit 改造的独立设计建议

本机观察只说明“GUI shell + 本地开源处理组件”的组合可行，不要求复用其实现。改造保留现有仓库和工具，定义一个稳定的共享转换核心，CLI、macOS GUI 与 MCP 只做请求/事件适配。核心独立负责输入校验、格式能力、有限并发队列、进度、取消/错误、临时输出、验证后原子提交，避免多个入口复制转换逻辑。使用用户机器自行安装的 FFmpeg、ffprobe、ImageMagick 等依赖，不复制 9xConvert 的打包二进制、资源、界面或代码。PDF 与自然语言规划放在同一个能力/任务协议上扩展；自然语言层只产出可审阅任务，不获得任意 shell 拼接能力。

以上为自主工程选择，来源是任务需求和通用接口设计，不能描述为从 9xConvert 恢复的架构。
