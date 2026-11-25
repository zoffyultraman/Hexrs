# 《英雄联盟》海克斯与全流程对局助手开发规划（Rust + GPUI）

## 1. 项目定位与核心优势

本项目旨在打造一款面向《英雄联盟》海克斯大乱斗（Arena / 海斗）及娱乐模式的极轻量级桌面端辅助工具，覆盖**选人决策**与**局内海克斯三选一识别**全流程。

相较于市面上基于 Electron / Chromium 的同类产品（普遍存在安装包 100MB+、内存占用 500MB+、团战掉帧等痛点），本项目采用 **Rust + GPUI** 原生 GPU 加速架构，核心差异化优势为：
* **极致轻量：** 单文件便携版 `.exe`（目标体积 $< 15\text{MB}$），后台内存占用 $< 50\text{MB}$。
* **零帧率损耗：** 硬件加速渲染 + 按需触发的零拷贝抓屏，高刷屏下丝滑无卡顿。
* **本地毫秒级响应：** 全量胜率与协同数据以压缩 SQLite 形式缓存于本地，对局内查询延迟 $< 1\text{ms}$，不受网络波动影响。
* **绝对合规安全：** 坚持纯信息展示路线，绝不读内存、绝不注入 DLL、绝不使用全局键盘钩子。

---

## 2. 技术架构设计

### 2.1 整体分层架构

```text
+-------------------------------------------------------------------+
|                     GPUI 渲染层 (hex-ui)                           |
|  - 选人侧边栏 (吸附客户端大厅、可交互点击)                          |
|  - 局内悬浮窗 (无边框透明置顶、鼠标穿透、热键切换交互态)            |
+-------------------------------------------------------------------+
                 ^ (GPUI Entity / Channel 事件驱动)
+-------------------------------------------------------------------+
|                 核心业务与状态机调度 (hex-core)                     |
|  - 进程监控 (LeagueClientUx.exe / League of Legends.exe)          |
|  - 窗口状态机切换 (大厅吸附态 <-> 局内穿透态 <-> 后台休眠态)        |
+-------------------------------------------------------------------+
          ^                                       ^
+-------------------------------+   +-------------------------------+
|    客户端接口采集器 (LCU/Live)  |   |     高速视觉识别 (hex-vision)  |
| - sysinfo 提取启动端口/Token   |   | - windows-capture (WGC抓屏)   |
| - tokio-tungstenite 订阅选人   |   | - ROI 相对坐标裁剪            |
| - reqwest 轮询 2999 Live API   |   | - pHash 感知哈希匹配 (< 2ms)  |
+-------------------------------+   +-------------------------------+
          v                                       v
+-------------------------------------------------------------------+
|                 本地数据引擎与上报 (hex-data)                       |
|  - rusqlite 内存只读查询   - Zstd 静默热更新   - 对局结束脱敏上报   |
+-------------------------------------------------------------------+
```

### 2.2 核心技术栈选型
* **开发语言与 UI 框架：** Rust + GPUI（Zed 开源的 GPU 加速 UI 框架）
* **Windows 系统交互：** `windows-rs`（获取 `HWND`、窗口坐标吸附、透明置顶与鼠标穿透控制）
* **异步与网络：** `tokio`、`reqwest`（`rustls-tls` 处理自签名证书）、`tokio-tungstenite`（LCU WebSocket）
* **进程与系统托盘：** `sysinfo`、`tray-icon`
* **视觉抓屏与匹配：** `windows-capture`（基于 `Windows.Graphics.Capture`）、`image`、自定义 pHash 特征库
* **本地存储与压缩：** `rusqlite`、`zstd`

---

## 3. 核心功能规划

### 3.1 选人阶段（Champ Select）—— 大厅侧边栏模式
完全基于 LCU WebSocket 事件（`OnJsonApiEvent_lol-champ-select_v1_session`）实时驱动，无需抓屏。

1. **英雄强度与梯度评级（S+ / S / A / B / C / D）**
   * 实时展示当前预选、锁定或随机英雄的梯度徽章（色块区分）。
   * 展示核心胜率（或前四吃分率 / 第一吃鸡率）、登场率及当前版本主流玩法标签（如“穿甲流”“特效流”“前排控制”）。
   * **备选池（骰子池）高亮：** 自动扫描备选池，将池内梯度最高的 2~3 名英雄置顶标亮，辅助快速抢人。
2. **智能团队英雄推荐（双层算法）**
   * **历史协同推荐（Synergy）：** 根据队友已选/预选英雄，毫秒级查询本地搭档矩阵，置顶推荐配合胜率最高的 Top 5 英雄，并标注具体组合胜率（如“与亚索搭配胜率 56.4%”）。
   * **阵容短板诊断（Comp Balance）：** 实时计算己方物理/魔法伤害占比、前排承伤能力、硬控数量；当阵容失衡时弹出简短预警（如“缺魔法伤害”“缺前排开团”），并自动在推荐列表中为对应补位英雄加权提排。
3. **战前快速预览**
   * 点击推荐英雄可展开其核心海克斯 Top 3 与核心三件套，帮助玩家提前确定打法与召唤师技能。
4. **交互策略：自动吸附**
   * 通过 `GetWindowRect` 监听 `LeagueClientUx.exe` 位置，将宽约 320px 的 GPUI 面板自动吸附在大厅右侧边缘并跟随移动/最小化；此阶段**关闭鼠标穿透**，允许自由点击交互。

### 3.2 局内阶段（In-Game）—— 透明悬浮窗模式
检测到游戏进程 `League of Legends.exe` 启动后，自动隐藏大厅侧边栏并切换至局内透明悬浮窗。

1. **海克斯三选一实时识别与推荐**
   * **按需触发：** 平时休眠以保证零开销；仅在海斗模式且检测到选卡界面特征区域时唤醒 WGC 抓屏。
   * **极速匹配：** 按屏幕宽高比裁剪三张卡片的图标 ROI，通过 pHash 计算汉明距离，耗时 $< 2\text{ms}$ 得出海克斯 ID。
   * **精简展示：** 在三张卡片上方悬浮显示 `S+ ~ C` 梯度徽章、平滑胜率；若与玩家已拥有的海克斯形成强力配合，亮起金色 **⚡ 完美协同** 徽章。
2. **动态出装参考**
   * 结合 `Live Client Data API`（`127.0.0.1:2999`）获取的英雄与当前经济/装备，展示对应海克斯流派的后续推荐出装。
3. **交互策略：默认穿透 + 热键唤醒**
   * 默认开启 `WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW`，鼠标点击直接穿透至游戏内，完全不干扰走位与技能释放。
   * 通过轮询 `GetAsyncKeyState` 监听自定义热键（如按住 `Alt`），临时取消穿透以允许玩家拖拽面板或查看详细流派说明。

---

## 4. 数据管线与算法模型

### 4.1 三源融合采集策略
1. **静态基础字典（CommunityDragon）：** 定时脚本拉取 CDragon 开源解包数据，获取全量英雄、装备、海克斯的 ID、中文名、品质与图标 PNG，自动生成 pHash 特征库。
2. **冷启动胜率（海外官方 API + 公开榜单）：** 初期通过 Riot `MATCH-V5` 接口或公开统计榜单（如 Lolalytics / OP.GG）生成首版基准数据。
3. **国服护城河（客户端众包上报）：** 对局结束时，通过本地 LCU 接口（`/lol-end-of-game/v1/eog-stats-block`）提取脱敏后的全员（8/16 人）英雄、名次、出装与所选海克斯 ID 上报至服务端。1 局游戏贡献 8~16 条纯正国服样本，快速反哺并修正国服胜率模型。

### 4.2 核心评分算法
* **贝叶斯平滑胜率（抑制冷门样本虚高）：**
  设定先验权重 $C$（如 $C = 50$）与基准胜率 $P_{\text{base}}$：
  $$\text{平滑胜率} = \frac{\text{实际胜场} + C \times P_{\text{base}}}{\text{实际总场次} + C}$$
* **协同增益系数（Synergy Lift）：**
  评估英雄在同时拥有海克斯 $A$ 与 $B$ 时的化学反应（团队双英雄搭配同理）：
  $$\text{Lift} = \frac{P(\text{Win} \mid \text{Hero}, A, B)}{P(\text{Win} \mid \text{Hero}, A) \times P(\text{Win} \mid \text{Hero}, B) / P(\text{Win} \mid \text{Hero})}$$
  当 $\text{Lift} > 1.15$ 且样本量达标时，触发前端 **⚡ 完美协同** 提示。

### 4.3 本地 SQLite 数据库结构（`meta.db`）
每日离线计算后打包为 `meta.db.zst`（约 $2\text{MB} \sim 3\text{MB}$），经 CDN 下发静默热更新。

| 表名 | 核心字段 | 用途 |
| :--- | :--- | :--- |
| `hero_tier_stats` | `hero_id`, `tier`, `win_rate`, `pick_rate`, `tags` | 选人阶段英雄 S+~D 评级、胜率与流派标签 |
| `hero_duo_synergy` | `hero_a`, `hero_b`, `duo_win_rate`, `lift_score`, `sample_size` | 选人阶段队友最佳搭档 Top 5 查询 |
| `hero_attributes` | `hero_id`, `ad_weight`, `ap_weight`, `tank_score`, `cc_score` | 选人阶段团队阵容短板实时诊断 |
| `hero_augment_stats` | `hero_id`, `augment_id`, `tier`, `win_rate`, `pick_rate` | 局内单卡海克斯梯度与平滑胜率 |
| `augment_synergy` | `hero_id`, `aug_a`, `aug_b`, `lift_score`, `sample_size` | 局内已有海克斯与新刷海克斯协同判断 |
| `hero_builds` | `hero_id`, `playstyle_tag`, `core_items_json`, `win_rate` | 选人预览及局内动态出装推荐 |

---

## 5. GitHub Actions 自动化编译与运维

利用 GitHub Actions 托管构建与数据更新，实现零本地打包成本与极低服务器开销：

1. **Windows 自动编译与发版（`.github/workflows/release.yml`）**
   * **触发机制：** 推送 `v*` 标签或手动触发。
   * **运行环境：** `windows-latest`（自带完整 MSVC 与 Windows SDK）。
   * **静态链接：** 配置 `RUSTFLAGS="-C target-feature=+crt-static"`，彻底消除玩家缺少 VC++ 运行库的问题；`Cargo.toml` 开启 `lto = "thin"` 与 `strip = true` 压缩体积；入口文件配置 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` 隐藏黑框控制台。
   * **构建缓存：** 使用 `Swatinem/rust-cache@v2` 缓存庞大的 GPUI 依赖树，将增量编译耗时压至 1~3 分钟。
   * **自动发布：** 编译产出单文件 `HexOverlay.exe` 并自动打包上传至 GitHub Releases。
2. **定时数据更新管线（`.github/workflows/update-meta.yml`）**
   * 配置 `cron` 定时任务每日凌晨自动运行：拉取最新 CDragon 资源 -> 清洗汇总上报样本 -> 执行贝叶斯与协同算法 -> 生成 `meta.db.zst` 与 `manifest.json` -> 推送至对象存储（Cloudflare R2 / OSS）或独立数据分支。

---

## 6. 8 周研发里程碑

| 阶段 | 周期 | 核心目标 | 关键交付物 |
| :--- | :--- | :--- | :--- |
| **Phase 1：底层基建与窗口闭环** | 第 1–2 周 | 跑通 GPUI 窗口特性与客户端通信 | Cargo Workspace 骨架；GitHub Actions 自动编译链路；能在游戏上透明置顶且鼠标穿透的 GPUI Demo；LCU 与 Live API 基础通信模块。 |
| **Phase 2：选人辅助与视觉识别** | 第 3–4 周 | 落地选人侧边栏与局内海克斯识别 | **第 3 周：** LCU 选人事件解析、大厅右侧自动吸附、英雄 S+~D 评级与团队推荐上线。<br>**第 4 周：** CDragon pHash 库脚本、WGC 抓屏裁剪识别（$< 2\text{ms}$）、冷启动 `meta.db` 本地查询打通。 |
| **Phase 3：UI 打磨与多端适配** | 第 5–6 周 | 达到日常可用的产品级体验 | 暗黑风 UI 视觉精修、⚡ 协同徽章、热键切换穿透态、系统托盘设置面板、1080P/2K/4K 及多 DPI 缩放坐标适配。 |
| **Phase 4：数据闭环与内测分发** | 第 7–8 周 | 实现众包自循环与自动化运营 | 对局结束脱敏上报接口、每日定时离线算力 Workflow、客户端启动静默热更新、单文件便携版小范围玩家内测。 |

---

## 7. 推荐代码仓库结构

```text
hex-overlay/
├── .github/
│   └── workflows/
│       ├── release.yml         # Windows 自动编译、静态链接与 Release 发布
│       └── update-meta.yml     # 每日定时拉取 CDragon 与离线算力打包任务
├── Cargo.toml                  # Workspace 定义及 release 体积优化配置
├── crates/
│   ├── hex-core/               # 进程监控、LCU WebSocket、Live API、窗口状态机
│   ├── hex-vision/             # WGC 抓屏、多分辨率 ROI 裁剪、pHash 图像匹配
│   ├── hex-data/               # SQLite 内存查询、Zstd 解压热更新、战绩脱敏上报
│   └── hex-ui/                 # GPUI 选人侧边栏/局内悬浮窗、Win32 吸附与穿透、托盘
└── scripts/
    ├── fetch_cdragon.py        # 拉取 CDragon 静态元数据与图标，生成 pHash 特征库
    └── calc_meta.py            # 计算贝叶斯胜率、协同 Lift 矩阵并导出 meta.db.zst
```

---

## 8. 合规红线与风险应对

| 风险项 | 潜在影响 | 规避与应对方案 |
| :--- | :--- | :--- |
| **反作弊误报 (ACE / Vanguard)** | 账号封禁或进程被强杀 | 严禁读取游戏内存、注入 DLL、修改封包或使用 `SetWindowsHookEx`；严格限定在官方本地接口 + WGC 抓屏 + `GetAsyncKeyState` 范围内，且不做任何自动点击/代操作。 |
| **GPUI Windows 边缘兼容问题** | 透明窗口黑边或特定驱动渲染异常 | 第 1 周优先在真实对局环境下验证透明穿透 Demo；保持 `hex-ui` 与 `hex-core` 彻底解耦，确保极端情况下可低成本切换渲染壳（如 `egui` / `Tauri`）。 |
| **游戏改版导致坐标偏移** | 选卡 UI 微调引发抓屏识别失效 | 将海克斯卡片 ROI 相对坐标下沉至云端 `manifest.json`，游戏改版时无需重发客户端，直接热更新坐标配置即可修复。 |
| **未签名 `.exe` 杀软误报** | Windows Defender / 安全软件拦截 | 内测期提供开源编译记录与加白指引；规模分发后引入代码签名证书，在 GitHub Actions 打包前集成 `signtool.exe` 自动签名。 |