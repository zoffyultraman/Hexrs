# Hexrs (`HexOverlay`)

面向《英雄联盟》**海克斯大乱斗 / 斗魂竞技场（Arena / 海斗）** 的极轻量级桌面端辅助工具，基于 **Rust + GPUI** 原生 GPU 加速架构打造，覆盖**选人决策**与**局内海克斯三选一识别**全流程。

---

## 核心特性

* **极致轻量与零帧率损耗**：静态链接单文件便携版 `HexOverlay.exe`（无需额外安装 VC++ 运行库），原生 GPU 加速渲染 + 按需触发 WGC 抓屏，平时休眠零开销。
* **选人阶段 · 大厅自动吸附侧边栏**：
  * 通过 `LCU WebSocket` 实时驱动，自动吸附于客户端右侧边缘（320px 暗黑海克斯风面板），支持鼠标自由交互点击。
  * **英雄梯度与备选池高亮**：展示 `S+ ~ D` 梯度徽章、胜率/登场率及流派标签，自动标亮备选池高胜率英雄。
  * **双层团队推荐**：基于本地 SQLite 搭档矩阵推荐 Top 5 搭档，并实时诊断阵容短板（如`缺魔法伤害`、`缺前排开团`、`缺硬控`）。
  * **战前快速预览**：点击任意英雄展开核心海克斯 Top 3 与核心三件套。
* **局内阶段 · 透明穿透悬浮窗**：
  * **按需 WGC + `<2ms` pHash 识别**：仅在支持海克斯的模式且检测到选卡界面时触发抓屏，通过 64-bit DCT-II 感知哈希匹配 3 张海克斯卡片，在卡片上方悬浮展示梯度、平滑胜率与金色 **`⚡ 完美协同`** 徽章。
  * **动态出装参考**：结合 `Live Client Data API (127.0.0.1:2999)` 实时经济与已购装备，展示推荐三件套与完成状态。
  * **默认穿透 + `Alt` 热键切换**：默认开启 `WS_EX_TRANSPARENT` 鼠标穿透绝不干扰操作；按住 `Alt` 临时解锁交互与详情操作，松开即恢复穿透。
* **数据闭环与静默热更新**：
  * 启动及托盘触发后台 `Zstd` 增量热更新（`manifest.json` + SHA-256 校验 + 原子替换 `meta.db`），可同步更新云端 ROI 坐标配置。
  * 系统托盘支持快捷切换**显示悬浮窗**、**匿名众包战绩上报**、**检查数据热更新**与**打开数据目录**。

---

## 架构概览

```text
crates/
├── hex-core/     # 进程监控 (sysinfo)、LCU WebSocket/REST、Live API (2999)、状态机调度
├── hex-vision/   # Windows.Graphics.Capture 按需抓屏、多分辨率 ROI、DCT-II pHash 匹配
├── hex-data/     # 内存只读 SQLite 引擎、双层推荐、Zstd 静默热更新、脱敏 EOG 上报
└── hex-ui/       # GPUI 选人侧边栏 & 局内透明悬浮窗、Win32 吸附/穿透、系统托盘
scripts/
├── fetch_cdragon.py  # 拉取 CommunityDragon 英雄/装备/海克斯元数据并计算一致 pHash
└── calc_meta.py      # 计算贝叶斯平滑胜率与协同 Lift，导出 meta.db.zst 与 manifest.json
```

---

## 下载与使用

1. 前往仓库 [Releases](https://github.com/zoffyultraman/Hexrs/releases) 页面下载最新版 `HexOverlay.exe`（便携单文件，解压/双击即可运行）。
2. 启动后程序常驻系统托盘（默认开启匿名胜率数据库与热更新检查），打开《英雄联盟》客户端进入选人或对局后会自动切换模式：
   * **选人阶段**：自动吸附于大厅右侧。
   * **对局阶段**：默认全屏透明鼠标穿透，按住 **`Alt`** 键可临时解锁交互。
3. **关于安全软件误报提示**：由于内测阶段便携版 `.exe` 暂未配置商业代码签名证书，部分 Windows Defender 或安全软件可能会提示拦截，建议在确认由本仓库 GitHub Actions 公开编译后手动加入信任/白名单。

### 可选环境变量

* `HEXRS_MANIFEST_URL`：自定义热更新 `manifest.json` 地址（默认指向仓库 `data` 分支）。
* `HEXRS_TELEMETRY_URL`：配置匿名战绩众包上报接收端 URL（未配置时自动跳过网络上报）。

---

## 合规与隐私承诺

* **绝不读取游戏内存、绝不注入 DLL、绝不修改封包、绝不使用 `SetWindowsHookEx` 全局钩子**。
* 所有能力严格限定在官方本地接口（LCU / Live Client Data API）、官方 `Windows.Graphics.Capture` 抓屏及 `GetAsyncKeyState` 轮询范围内，无任何自动点击或代操作行为。
* 战绩上报与本地缓存均在反序列化阶段即剥离所有召唤师名称、Riot ID、PUUID 与对局 ID，仅保留匿名英雄/名次/装备/海克斯 ID。
