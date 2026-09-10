# 阿里云 AI Profiling Demo —— 外链跳转设计

## 1. 背景

AIProf 开源版本聚焦**采集侧**能力展示。深度的服务端 GPU 诊断、
以及 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标的采集，
均不在开源范围内，统一通过外链跳转到云端 Demo 环境体验。
开源版本部署在用户自有机器上，不直接调用任何云端分析 OpenAPI。

## 2. 当前实现

阿里云能力入口收敛为**单一顶层入口 + 若干文案提示**：

- **顶层入口**：AIProf 主页面 `CapturePage` Tab 栏最右侧一项，
  label 显示为「**阿里云 AI Profiling Demo ↗**」，点击时不切换 Tab、
  直接 `window.open(ALIYUN_DEMO_URL, '_blank', 'noopener')` 在新标签页打开。
- **前期**：指向「系统运维联盟」社区平台 `https://soma.openanolis.cn`
- **后续**：正式 Demo 环境就绪后，修改配置变量切换到 Demo 地址即可，
  无需改代码

### 配置变量

| 变量 | 位置 | 默认值 |
|------|------|--------|
| `VITE_ALIYUN_AGENT_URL` | `ui/webapp/.env` / `.env.production` | `https://soma.openanolis.cn` |
| `ALIYUN_AGENT_URL` | 服务端环境变量（用于 API 错误提示） | 同上 |

部署时可通过 `.env.production` 或 docker build-arg 覆盖前端变量。

### 前端行为

单一入口 + 3 处文案引流，全部为纯前端 URL 跳转，不发起后端请求：

- **顶层入口**（`CapturePage.tsx` `aliyun-console` Tab）：主要入口，点击新标签页打开配置 URL。
- **采集表单提示**（`CapturePage.tsx` 「数据丰富度」Form.Item tooltip）：
  「如需 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标，请到阿里云操作系统控制台使用」。
- **报告详情提示**（`ProfileTab.tsx` Alert）：
  「如需阿里云 Profiling Agent 云端诊断，请前往顶部『阿里云 AI Profiling Demo』标签页体验」。
- **记录表下载 tooltip**（`AiRecordTable.tsx`）：
  「下载 chrome-tracing JSON，可上传到阿里云 AI Profiling Demo 体验」。

历史遗留：`ProfileTab` 内部仍保留 `mode === 'aliyun'`
的**只读**结论展示（结论卡片 + 对比 Drawer），用于回看以前的云端结论；
开源版本不再提供触发 aliyun 分析的按钮。

「独立 Trace 分析」标签及其 `/api/ai/standalone/*` 端点已整体移除，
该能力由 `AIPROF_EXTERNAL_ANALYZER_URL` 指向的云端分析平台承接。

### 后端行为

- `/api/ai/analyze` 仅支持 `mode = 'openclaw' | 'local'`；其他 mode 返回 400，
  错误消息包含 `ALIYUN_AGENT_URL` 供前端提示跳转。
- 开源版本无任何调用云端分析 OpenAPI 的代码路径。

## 3. 本地分析能力（开源内置）

| 模式 | 说明 |
|------|------|
| 本地 Agent | `local-profiling-agent`：解析 trace → 固定指标工具 → LLM 多轮 refine 综合结论 |
| 本地 OpenClaw | `openclaw` agent CLI：LLM 直接分析 trace 摘要 |

两者均只需配置任一 OpenAI 兼容 LLM 的 API Key（设置页 / 环境变量 `QWEN_API_KEY`），
无外部依赖。本地分析为轻量能力，与云端 Demo 的深度诊断刻意区分。

## 4. 后续规划（Demo 环境）

正式 Demo 环境就绪后，只需修改 `VITE_ALIYUN_AGENT_URL` 为 Demo 地址，无需改代码。
