# AIProf 项目架构与功能全景

## 1. 项目定位

AIProf 是一套面向 AI/GPU 工作负载的**开源性能分析平台**，将三段能力串成端到端流水线：

```
无侵入采集 → 报告聚合 → LLM 智能归因
```

目标用户：训练/推理基础设施工程师、模型工程师、SRE。

**核心价值主张**：在不修改任何业务代码、不重启目标进程的前提下，对生产环境 GPU/PyTorch 任务完成一次完整的性能剖析——从 Python 调用栈到 CUDA kernel 时间线再到显存快照——5 秒出火焰图，再让 LLM 自动给出可读的瓶颈结论。

---

## 2. 系统架构

### 2.1 整体拓扑

```
                     ┌─────────────────────────────────────────────────────────┐
                     │                    aiprof-server                         │
                     │                                                         │
                     │  ┌───────────────┐   ┌──────────────────┐              │
  浏览器 ◀──HTTP──▶ │  │ dashboardSvr  │   │ analysis_summary │              │
  :17000             │  │ (Node/Express)│   │  (PyInstaller)   │              │
                     │  │               │   └──────────────────┘              │
                     │  │  • WS 任务分发│                                      │
                     │  │  • REST API   │   ┌──────────────────┐              │
                     │  │  • 静态 SPA   │   │  AI 分析通路      │              │
                     │  │  • 上传接收   │   │  • local-agent   │              │
                     │  └───────┬───────┘   │  • openclaw      │              │
                     │          │            └──────────────────┘              │
                     │          │                                              │
                     │  结果落盘: /var/lib/aiprof/result/<analysisId>/          │
                     └──────────┼──────────────────────────────────────────────┘
                                │
                    WebSocket + HTTP multipart upload
                                │
                     ┌──────────┼──────────────────────────────────────────────┐
                     │          ▼              aiprof-client                    │
                     │  ┌───────────────┐                                      │
                     │  │  WS Agent     │  ← 拉任务 / 回报状态                  │
                     │  │  (Rust)       │                                      │
                     │  └───────┬───────┘                                      │
                     │          │                                              │
                     │  ┌───────▼──────────────────────────────┐              │
                     │  │    CollectionFramework (Rust)         │              │
                     │  │                                      │              │
                     │  │  ┌─────────┐ ┌─────────┐ ┌────────┐│              │
                     │  │  │  Pyki   │ │  CUPTI  │ │MemMon  ││              │
                     │  │  │(Python栈)│ │(Kernel) │ │(RSS)   ││              │
                     │  │  └─────────┘ └─────────┘ └────────┘│              │
                     │  └───────┬──────────────────────────────┘              │
                     │          │                                              │
                     │  ┌───────▼───────┐                                      │
                     │  │    dashrs     │  → tar.gz 打包 + multipart 上传       │
                     │  └───────────────┘                                      │
                     │                                                         │
                     │  运行要求: --pid=host, CAP_SYS_PTRACE, --gpus all       │
                     └─────────────────────────────────────────────────────────┘
```

### 2.2 通信协议

| 通道 | 方向 | 用途 |
|------|------|------|
| WebSocket `/ws` | server → client | 任务分发(NEW_TASK)、心跳、状态回报 |
| HTTP `POST /api/results/upload` | client → server | 采集产物 tar.gz multipart 上传 |
| HTTP REST `/api/v1/...` | 浏览器 → server | 前端所有 CRUD 操作 |

全系统**只需一个端口(17000)**对外暴露。

---

## 3. 模块详解

### 3.1 aiprof-client — 采集侧

#### CollectionFramework (Rust workspace)

| crate | 职责 |
|-------|------|
| `CollectionFramework` (主 bin) | 插件调度、detector 执行、CLI 入口 |
| `client/` | WS agent — 连 server 拉任务、驱动 CF、回报生命周期 |
| `dashrs/` | tar.gz 打包 + HTTP multipart 上传 |
| `fifo/` | 进程间 FIFO 通信协议 |
| `ossrs/` / `slsrs/` | 可选的对象存储/日志上传 |

#### 采集插件

| 插件 | 注入方式 | 采集目标 | 输出 |
|------|----------|----------|------|
| **Pyki** | ptrace 远程注入 libloader.so → 加载 pyki wheel 到目标进程 | Python 调用栈 + torch.profiler (activities=[cpu,cuda]) + memory_history | chrome-trace JSON、memory snapshot pickle.gz、folded stacks |
| **CUPTI (cuprof)** | CUDA_INJECTION64_PATH 注入 libcuprof.so | CUDA kernel 事件、显存拷贝、NVLink | chrome-trace JSON |
| **Memory Monitor** | procfs 轮询 | 目标进程 RSS 峰值 | 单值 |

#### 采集选择逻辑 (config.yaml)

```yaml
indicators:
  - name: GPU        # 优先 Pyki(需 torch≥2.1 + Python 3.9-3.12)；退化到 CUPTI
  - name: Torch      # torch.profiler CPU+CUDA 融合时间线
  - name: PyStack    # 纯 Python 调用栈(无 GPU 也能采)
  - name: Snapshot   # torch.cuda.memory._record_memory_history (torch≥2.3)
```

每个指标按 collector 优先级逐个执行 detector；第一个全部通过的 collector 被选中。

#### 注入流程(Pyki)

```
CF 主进程
  │
  ├─ 1. profiler_init() — 初始化 libprofiler.a
  ├─ 2. profiler_attach(pid) — ptrace attach 目标
  │     └─ 远程 mmap + dlopen libloader.so 到目标地址空间
  ├─ 3. libloader.so 在目标内:
  │     ├─ pip install pyki wheel (from vendored .whl)
  │     ├─ 启动 pyki profiling session
  │     └─ 通过 unix socket 回报进度
  ├─ 4. profiler_detach() — ptrace detach
  └─ 5. 等待 pyki 完成 → 收集产物
```

### 3.2 aiprof-server — 服务侧

#### dashboardServer.js (Node/Express BFF)

核心职责:
- **WebSocket 管理**: client 注册、心跳、任务下发
- **REST API**: 采集触发、记录查询、结果下载、GPU 进程发现
- **上传接收**: multipart → 解压 → 触发 analysis_summary
- **静态服务**: Vite React SPA + Perfetto 资源

#### analysis_summary (Python → PyInstaller onefile)

| 文件 | 职责 |
|------|------|
| `analysis.py` | 主聚合逻辑: 解析 chrome-trace → GPU/CPU 利用率、Top Kernel、内存时间序列 |
| `flamegraph.py` | Python 调用栈折叠 → 火焰图数据 |
| `gmem.py` | CUDA 显存快照解析 + 碎片/尖峰检测 |
| `diff_analysis.py` | 两次采集 A/B 对比分析 |
| `report.py` | 报告模板渲染 |

聚合产出 `summary.json` 落盘到 `<RESULT_DIR>/<analysisId>/`，前端直接读取渲染。

#### AI 分析通路

| 模式 | 实现 | 特点 |
|------|------|------|
| **local** | `local-profiling-agent/` (Node.js) | 解析 chrome-trace → 7 种专用工具(GPU idle / Kernel / Launch overhead / Memory / Periodicity / Statistics / DataProfile) → LLM 多轮 refine → 综合结论 |
| **openclaw** | `openclaw agent --local` | Agentic 循环 + MCP 工具链，多轮工具调用深度归因 |

两条通路均只需配置 OpenAI 兼容 LLM 的 Base URL + API Key。

##### local-profiling-agent 工具链

```
traceIngest.js  ← 解析 chrome-trace JSON 为内存结构
     │
     ├─ analyzeStatistics.js   — 整体 GPU/CPU 利用率统计
     ├─ analyzeKernel.js       — Top-N kernel 耗时 + occupancy 分析
     ├─ analyzeGpuIdle.js      — GPU 空闲段归因(host-bound / sync / launch)
     ├─ analyzeLaunchOverhead.js — kernel launch 开销分析
     ├─ analyzeMemory.js       — 显存使用模式 + 峰值定位
     ├─ analyzePeriodicity.js  — 周期性模式检测(data loader stall / GC)
     └─ getDataProfile.js      — 数据加载 pipeline 诊断
          │
          ▼
     planner.js   → 决定调哪些工具
     synthesizer.js → LLM 综合所有工具输出 → markdown 结论
```

### 3.3 ui/webapp — 前端

**技术栈**: Vite + React 18 + TypeScript + Ant Design + ECharts + react-markdown

#### 页面结构

| 路径 | 页面 | 功能 |
|------|------|------|
| `/aiprof` | 主入口 | 采集/记录/设置三 Tab |
| CapturePage | 采集触发 | 选实例 → 自动发现 GPU 进程 → 配置参数 → 开始 |
| AiRecordTable | 记录列表 | 状态轮询、参数展示、删除、跳转详情 |
| LlmSettingsPanel | LLM 设置 | Base URL / API Key / 模型名 配置 |
| `/aiprof/result` | 报告页 | ProfileTab + AI 结论卡片 |

#### 报告页 ProfileTab 区块

```
┌─────────────────────────────────────────────────────┐
│  ① AI 分析结论 (markdown, 可切换 local/openclaw)     │
├─────────────────────────────────────────────────────┤
│  ② 设备信息 + 概览统计 (GPU型号/利用率/SM占用)        │
├─────────────────────────────────────────────────────┤
│  ③ 执行延迟分布 (Host→Device pipeline 各阶段耗时)     │
├─────────────────────────────────────────────────────┤
│  ④ Top Kernel 表 (name/duration/grid/block/occupancy)│
├─────────────────────────────────────────────────────┤
│  ⑤ GPU 显存使用情况 (Reserved/Allocated 时间序列)      │
├─────────────────────────────────────────────────────┤
│  ⑥ CUDA 显存快照 (MemoryViz iframe + 下载)           │
├─────────────────────────────────────────────────────┤
│  ⑦ Python 调用栈火焰图 (SVG 内联渲染)                 │
├─────────────────────────────────────────────────────┤
│  ⑧ Perfetto 时间线 (iframe 嵌入 ui.perfetto.dev)     │
└─────────────────────────────────────────────────────┘
```

---

## 4. 数据流

```
用户点击「开始分析」
       │
       ▼
  POST /api/.../start_ai_analysis
  {instance, pids, timeout, metrics:[GPU,Torch,PyStack,Snapshot]}
       │
       ▼
  dashboardServer → WS NEW_TASK → aiprof-client
       │
       ▼
  CollectionFramework:
    1. detect: 逐个 detector 检查环境(Python版本/torch/CUDA/pip)
    2. collect: 选中的 collector 执行注入 + 采集
    3. package: dashrs 把 output/ 打成 tar.gz
    4. upload: POST multipart → server
       │
       ▼
  dashboardServer 收到上传:
    1. 解压到 RESULT_DIR/<analysisId>/
    2. 调 analysis_summary 聚合 → summary.json
    3. 标记状态为 completed
       │
       ▼
  前端轮询状态 → 跳转报告页 → 渲染 summary.json
       │
       ▼
  用户点击「AI 分析」:
    → local: 调 local-profiling-agent → LLM → markdown
    → openclaw: spawn openclaw agent → agentic 循环 → markdown
       │
       ▼
  结论落盘 ai_conclusion.json，前端 react-markdown 渲染
```

---

## 5. 部署架构

### 5.1 双镜像设计

| 镜像 | 基础镜像 | 核心内容 | 资源需求 |
|------|----------|----------|----------|
| `aiprof/server` | node:22-bookworm-slim | dashboardServer + webapp dist + analysis_summary + local-agent + openclaw(可选) | 无 GPU，2C4G 起 |
| `aiprof/client` | nvidia/cuda:12.2.0-runtime | CollectionFramework + Pyki/CUPTI vendored .so + pyki wheel | 需 GPU + ptrace 权限 |

### 5.2 部署形态

```
形态 A: 单机 compose (开发/演示)
┌──────────────────────────────────────┐
│  docker-compose.yml                   │
│  server (bridge) ←→ client (host)    │
└──────────────────────────────────────┘

形态 B: 跨机 (生产)
┌─────────────────┐         ┌─────────────────────────┐
│  server 机器     │◀──WS───│  client 机器 (GPU)       │
│  (可无GPU)       │  HTTP   │  --pid=host --gpus all   │
│  :17000          │         │  CAP_SYS_PTRACE          │
└─────────────────┘         └─────────────────────────┘

形态 C: K8s
┌─────────────────────────────────────────────────┐
│  server: Deployment + ClusterIP Service          │
│  client: DaemonSet (GPU 节点 nodeSelector)        │
└─────────────────────────────────────────────────┘
```

---

## 6. 功能清单

### 6.1 采集能力

| 功能 | 描述 |
|------|------|
| GPU Kernel 时间线 | CUDA kernel 启停时间、grid/block 维度、SM occupancy |
| Python 调用栈 | 用户态注入取栈，不依赖 py-spy/perf，支持 Python 3.9-3.12 |
| Python↔CUDA 融合时间线 | torch.profiler activities=[cpu,cuda] 产出的完整时间线 |
| CUDA 显存快照 | torch.cuda.memory._record_memory_history snapshot，segment/block 级别可视化 |
| 显存使用时间序列 | Reserved/Allocated 随时间变化，5ms 粒度 bucket 聚合 |
| GPU 进程自动发现 | nvidia-smi 查询当前活跃 CUDA 进程，一键填入 PID |
| Dead PID 校验 | 提交前二次确认 PID 存活，避免 ptrace "No such process" |
| 多指标组合采集 | GPU + Torch + PyStack + Snapshot 可任意组合 |

### 6.2 分析与可视化

| 功能 | 描述 |
|------|------|
| 设备概览 | GPU 型号、显存容量、利用率、SM/warp 活跃度 |
| Top Kernel 排行 | 按耗时排序，含 launch 参数、occupancy |
| 执行延迟分析 | Host→Device pipeline 各阶段（launch / sync / memcpy / compute） |
| GPU 显存使用曲线 | ECharts 交互图表，含 cached-free 计算、收尾归零过滤、低采样点警告 |
| CUDA 显存快照 | MemoryViz 嵌入渲染 segment/block 布局 |
| Python 火焰图 | SVG 内联渲染，支持缩放/搜索 |
| Perfetto 时间线 | 完整 chrome-trace 在 Perfetto UI 中交互查看 |
| A/B 对比分析 | 两次采集 diff：kernel 耗时变化、新增/消失 kernel |

### 6.3 AI 智能诊断

| 功能 | 描述 |
|------|------|
| 多轮工具调用 | local-agent 7 种分析工具 + LLM 综合 |
| Agentic 深度归因 | openclaw MCP 工具链，自动 drill-down |
| 结论持久化 | 分析结果按 analysisId 落盘，跨浏览器可见 |
| 结论缓存管理 | 重新分析前确认弹窗，防止误覆盖 |
| Markdown 渲染 | 表格、代码块、层级标题完整支持 |
| LLM 可配置 | 任意 OpenAI 兼容接口，Base URL + Key 即插即用 |

### 6.4 平台能力

| 功能 | 描述 |
|------|------|
| 多 Client 管理 | server 同时接入多台 GPU 机器，按 clientId 下发 |
| 任务生命周期 | 创建→采集中→上传中→分析中→完成/失败，全程状态可见 |
| 结果永久存储 | 每份报告独立 UUID，重启不丢 |
| 异常恢复 | stuck 状态自动检测 + 手动恢复 |
| 静态资源缓存策略 | Vite hashed asset 长缓存 + index.html no-store |

---

## 7. 技术特色

### 7.1 真正无侵入

- **不改代码**: 不需要在业务代码中 `import` 任何东西
- **不重启进程**: ptrace attach → 远程注入 → detach，目标进程全程在线
- **不装依赖**: pyki wheel 被注入到目标进程的 Python 环境中，用完即弃

### 7.2 Rust 采集框架 + Python 注入的混合架构

- 框架层(调度/打包/上传)用 Rust —— 低开销、无 GC、tokio 异步
- 注入层(torch.profiler / memory_history)用 Python —— 利用 PyTorch 原生 API
- 两者通过 ptrace + libloader.so 桥接，兼顾性能和生态

### 7.3 插件化 Detector 自动选型

不需要用户手动判断"该用哪个采集器"。config.yaml 定义的 detector 链自动检测环境(Python 版本、torch 版本、CUDA 版本、pip 可用性)，按优先级选出最合适的 collector。

### 7.4 LLM 分析不是噱头——有结构化工具链

local-profiling-agent 不是"把整段 trace 扔给 LLM"。它:
1. 先用确定性代码解析 chrome-trace 为结构化指标
2. 由 planner 决定调哪些分析工具
3. 每个工具产出定量结论(如"GPU idle 占比 34%，其中 72% 归因于 host-bound launch")
4. synthesizer 将所有工具输出交给 LLM 做最终综合

这保证了 LLM 的输入是**经过验证的数值**，不是原始 trace 乱猜。

### 7.5 极简部署

- 两个容器、一个端口、一条 WebSocket
- `docker compose up` 五分钟跑通
- 无外部依赖(无 Redis / Kafka / MySQL / MinIO 硬依赖)
- SQLite 即够，数据全落本地磁盘

### 7.6 显存分析完整链路

从 Reserved/Allocated 时间序列(宏观趋势) → CUDA memory snapshot(segment/block 微观布局) → LLM 归因(瓶颈定位)，三层递进，比单看 `nvidia-smi` 或单看 snapshot 都更完整。

---

## 8. 仓库目录索引

```
AIProf/
├── agent/
│   └── collection_framework/       # Rust workspace
│       ├── client/src/              #   WS agent (main.rs, ws.rs, task_runner.rs, profiler.rs)
│       ├── dashrs/src/              #   tar.gz 打包 + multipart 上传
│       ├── src/
│       │   ├── plugins/
│       │   │   ├── pyki/            #   vendored pyki 源码 + 预构建 wheel(pyki_dev_dir/)
│       │   │   ├── pyki_plugin_wrapper.rs #  Pyki 插件包装(Rust)
│       │   │   ├── cuprof/          #   CUPTI(cuprof) 插件
│       │   │   └── pystackCollector/ #  纯 Python 栈采集
│       │   ├── collector/           #   调度器 + 并发控制
│       │   └── main.rs             #   CLI 入口
│       ├── config.yaml              #   detector + collector 配置
│       └── Cargo.toml
├── local-profiling-agent/           # Node.js LLM 分析 agent
│   └── src/
│       ├── tools/                   #   7 种分析工具
│       ├── planner.js              #   工具调用规划
│       └── synthesizer.js          #   LLM 综合结论
├── agent-ai/                        # Python AI gateway (MCP tools)
├── server/
│   ├── dashboard/                   # Node BFF + Python 聚合
│   │   ├── dashboardServer.js      #   主服务(WS + REST + 静态)
│   │   ├── analysis.py             #   二次聚合(chrome-trace → summary)
│   │   ├── gmem.py                 #   显存快照解析
│   │   └── flamegraph.py           #   火焰图数据生成
│   └── services/                    # FastAPI 微服务(collector/query/report)
├── ui/
│   ├── webapp/                      # Vite + React 前端
│   │   └── src/pages/
│   │       ├── AIProf/             #   新版主界面
│   │       │   ├── pages/          #     CapturePage / LlmSettings
│   │       │   ├── components/     #     AiRecordTable / FlameCard / RingChart
│   │       │   └── result/         #     报告页 (ProfileTab + index)
│   │       └── ai_observable/      #   MemoryViz iframe host — AIProf 报告页
│   │                                #   的「CUDA 显存快照」卡通过
│   │                                #   /ai_observable/result?embed=memviz
│   │                                #   嵌入这里；不是遗留代码
│   └── bff/                         # 旧版 Node BFF
├── deploy/docker/                   # Dockerfile + compose + 脚本
├── reproducers/                     # 问题复现工具集
├── docs/                            # 项目文档
└── data/                            # 运行时数据(gitignored)
```

---

## 9. 与同类工具对比

| 维度 | AIProf | py-spy | NVIDIA Nsight | torch.profiler (原生) |
|------|--------|--------|---------------|----------------------|
| 侵入性 | 无(ptrace 注入) | 无(ptrace) | 需 nsys launch 包裹 | 需改代码加 with 块 |
| Python 栈 | ✅ | ✅ | ❌ | ✅(仅 profiler 范围内) |
| CUDA Kernel | ✅ | ❌ | ✅ | ✅ |
| Python↔Kernel 融合 | ✅ | ❌ | ❌ | ✅ |
| 显存快照 | ✅ | ❌ | 部分 | ✅(需手动) |
| 远程采集 | ✅(client/server) | ❌(本地) | ❌(本地) | ❌(本地) |
| LLM 归因 | ✅ | ❌ | ❌ | ❌ |
| Web UI | ✅ | ❌ | 桌面 GUI | TensorBoard |
| 生产环境友好 | ✅(不重启) | ✅ | ❌(需 launch) | ❌(需改代码) |
| 多机管理 | ✅ | ❌ | ❌ | ❌ |

---

## 10. Roadmap 方向(已在架构中预留)

- ROCm (AMD GPU) 采集支持
- NCCL 通信分析(多卡/多机训练)
- RDMA 网络性能关联
- NVTX 用户自定义标注
- DCGM 硬件计数器深度分析
- 多次采集趋势对比 Dashboard
