# Hexrs

面向《英雄联盟》**海克斯大乱斗 / 斗魂竞技场（Arena / 海斗）** 的极轻量级桌面端辅助工具，基于 **Rust + GPUI** 原生 GPU 加速架构打造，覆盖**选人决策**与**局内海克斯三选一识别**全流程。

---

## 核心特性

* **极致轻量与零帧率损耗**：静态链接单文件便携版 `Hexrs.exe`（无需额外安装 VC++ 运行库），原生 GPU 加速渲染 + 按需触发 WGC 抓屏，平时休眠零开销。
* **选人阶段 · 左侧吸附侧边栏 + 无遮挡胜率徽章条**：
  * 通过 `LCU WebSocket` 实时驱动，自动吸附于客户端左侧边缘（`320px` 可交互暗黑海克斯风侧边栏），同时在客户端顶部 10 个备选池头像下方与左侧 5 名队友行右侧叠加两条极窄、零拦截的非阻塞胜率徽章条（不遮挡或拦截任何客户端点击操作）。
  * **全队与备选池胜率展示**：实时呈现全队己方英雄与备选池所有英雄的梯度徽章、胜率与流派标签，自动标亮高胜率英雄。
  * **双层团队推荐与阵容诊断**：基于本地 SQLite 搭档矩阵推荐 Top 5 搭档，并实时诊断阵容短板（如`缺魔法伤害`、`缺前排开团`、`缺硬控`）。
  * **自动展开战前攻略**：自动展开当前选中英雄的核心海克斯 Top 3 与**推荐六神装**（`推荐六神装`），也支持点击任意队友或备选池英雄快速切换预览。
* **局内阶段 · 透明穿透六神装悬浮窗 + WinRT OCR 海克斯评分**：
  * **紧致六神装角落面板**：右上角 / 左上角 `324x204` 2×3 六格装备 HUD（支持通过系统托盘在左上角 / 右上角之间一键切换），永久鼠标穿透（`WS_EX_TRANSPARENT`）绝不干扰游戏操作；结合 `Live Client Data API (127.0.0.1:2999)` 实时同步已购装备与完成状态。
  * **按需 WGC + Windows 原生 WinRT OCR（`Windows.Media.Ocr`）识别**：结合 ARAM 等级节点（`3/7/11/15/21` 级）与阵亡 / 泉水状态门控按需抓屏，精准识别 3 张海克斯卡片，并在 3 张卡片正上方动态对齐渲染评分卡（展示梯度、平滑胜率与金色 **`⚡ 完美协同`** 徽章）。
* **数据闭环与静默热更新**：
  * 启动及托盘触发后台 `Zstd` 增量热更新（`manifest.json` + SHA-256 校验 + 原子替换 `meta.db`），可同步更新云端 ROI 坐标配置。
  * 系统托盘支持快捷切换**显示悬浮窗**、**装备面板位置（左上角 / 右上角）**、**匿名众包战绩上报**、**检查数据热更新**与**打开数据目录**。

---

## 架构概览

```text
crates/
├── hex-core/     # 进程监控 (sysinfo)、LCU WebSocket/REST、Live API (2999)、状态机调度
├── hex-vision/   # Windows.Graphics.Capture 按需抓屏、WinRT OCR (Windows.Media.Ocr)、DCT-II pHash 匹配
├── hex-data/     # 内存只读 SQLite 引擎、双层推荐、Zstd 静默热更新、脱敏 EOG 上报
└── hex-ui/       # GPUI 左侧边栏、非阻塞备选/队友徽章条、角落六神装 HUD、海克斯评分卡、系统托盘
scripts/
├── fetch_cdragon.py  # 拉取 CommunityDragon 英雄/装备/海克斯元数据并计算一致 pHash
└── calc_meta.py      # 融合 czgreat/aramhex-data 计算平滑胜率与六神装，导出 meta.db.zst 与 manifest.json
```

---

## 下载与使用

1. 前往仓库 [Releases](https://github.com/zoffyultraman/Hexrs/releases) 页面下载最新版 `Hexrs.exe`（便携单文件，解压/双击即可运行）。
2. 启动后程序常驻系统托盘（默认开启匿名胜率数据库与热更新检查），打开《英雄联盟》客户端进入选人或对局后会自动切换模式：
   * **选人阶段**：自动吸附于大厅左侧，同时在备选池与队友头像旁显示轻量胜率徽章条。
   * **对局阶段**：永久鼠标穿透渲染角落 `324x204` 六神装面板（可在托盘切换左上角 / 右上角），并在海克斯选卡出现时在卡片上方对齐弹出评分条。
3. **关于安全软件误报提示**：由于内测阶段便携版 `.exe` 暂未配置商业代码签名证书，部分 Windows Defender 或安全软件可能会提示拦截，建议在确认由本仓库 GitHub Actions 公开编译后手动加入信任/白名单。

### 可选环境变量

* `HEXRS_MANIFEST_URL`：自定义热更新 `manifest.json` 地址（默认指向仓库 `data` 分支）。
* `HEXRS_TELEMETRY_URL`：配置匿名战绩众包上报接收端 URL（未配置时自动跳过网络上报）。

---

## 数据来源与开源协议

* **海克斯大乱斗统计数据（英雄胜率 / 海克斯分英雄胜率 / 六神装推荐）**：
  * 项目地址：`https://github.com/czgreat/aramhex-data`
  * 开源协议：**CC BY-NC 4.0**（Creative Commons Attribution-NonCommercial 4.0 International，署名-非商业性使用 4.0 国际许可协议，协议原文：`https://github.com/czgreat/aramhex-data/blob/main/LICENSE`）
  * 授权说明：已获作者在 [czgreat/aramhex-data#4](https://github.com/czgreat/aramhex-data/issues/4) 中明确授权免费开源项目 `Hexrs` 按 CC BY-NC 4.0 协议使用，本项目仅作开源非商业用途并保留完整署名。
* **基础静态元数据与图标**：CommunityDragon (`https://www.communitydragon.org/`)、Data Dragon 及国服静态资源 CDN。
* **本项目代码协议**：GPL-3.0（非商业开源项目）。

---

## 合规与隐私承诺

* **绝不读取游戏内存、绝不注入 DLL、绝不修改封包、绝不使用 `SetWindowsHookEx` 全局钩子**。
* 所有能力严格限定在官方本地接口（LCU / Live Client Data API）、官方 `Windows.Graphics.Capture` 抓屏及系统原生 `Windows.Media.Ocr` 范围内，无任何自动点击或代操作行为。
* 战绩上报与本地缓存均在反序列化阶段即剥离所有召唤师名称、Riot ID、PUUID 与对局 ID，仅保留匿名英雄/名次/装备/海克斯 ID。
