# CollectionFramework

Rust 采集框架，AIProf client 侧使用。负责在目标机器上以插件形式启动各类
采集器（Pyki / CUPTI），按配置聚合结果并上传回 server。

## 运行时框架

```
[启动]
   ↓
读取配置 config.yaml：规定各类指标支持的环境/版本
   ↓
检测运行环境（RuntimeEnv）
   ↓
初始化采集器（根据配置 & 环境 enable/disable）
   ↓
创建适配器 Adapter（绑定对应平台采集器）
   ↓
定时采集（loop + sleep）
   ↓
调用 Writer 输出结果
```

## 开发环境搭建

```bash
sudo yum install python3-devel   # 或 apt install python3-dev
cargo build --release
```

生成物：`target/release/CollectionFramework`。

## 目录

- `src/plugins/` — 各类插件采集器（pyki / cuprof / pykiLoader）；`pyki/` 为 vendored pyki 全量源码 + 预构建 wheel（`pyki_dev_dir/`）
- `src/tools/` — 通用工具（writer、adapter、compress 等）
- `src/detector/` — 运行时环境检测
- `src/third_party/` — 第三方 vendored 依赖（`cupti/` 为预构建 `libcupti.so.*` 运行时库，非头文件，版权与再分发条款见该目录 `NOTICE`/`README.md`；`profiler/` 为头文件 + 预构建静态库）
- `config.yaml` — 采集器启用/禁用与版本约束

详见 `agent/AIPROF_README.md` 与仓库根 `docs/introduction.md`。
