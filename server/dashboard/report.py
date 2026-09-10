#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""AIProf report generator — reads Analysis_Summary.json and emits a self-contained HTML report.

Purely rule-based, no LLM. The diagnostic conclusion reuses the pre-computed
``conclusion`` field from analysis.py::_get_summary (rule-based Chinese
diagnostic string).

Usage:
    python3 report.py <Analysis_Summary.json path> [<output HTML path>]

Default output: Analysis_Report.html in the same directory as the input.
"""
import json
import re
import sys
import html


def _shorten_kernel(name: str) -> str:
    """Strip balanced <...> template parameters, trailing function signature (...),
    and CUDA ``void `` return types. Preserve namespaces (including
    ``(anonymous namespace)``) and the function name to avoid leaving only
    ``at::native::``.
    """
    if not name:
        return ""
    s = name.strip()
    # Strip balanced <...> template parameters
    while "<" in s:
        depth, start = 0, -1
        for i, ch in enumerate(s):
            if ch == "<":
                if depth == 0:
                    start = i
                depth += 1
            elif ch == ">":
                depth -= 1
                if depth == 0 and start >= 0:
                    s = s[:start] + s[i + 1:]
                    break
        else:
            break
    # Strip trailing function signature (balanced parens, scanning right to left)
    if s.endswith(")"):
        depth = 0
        for i in range(len(s) - 1, -1, -1):
            if s[i] == ")":
                depth += 1
            elif s[i] == "(":
                depth -= 1
                if depth == 0:
                    s = s[:i]
                    break
    # Strip C++ return-type prefixes (void / int / float / __global__, etc.)
    s = re.sub(r"^(?:__global__\s+|__device__\s+|void\s+|int\s+|float\s+|double\s+|bool\s+)+", "", s)
    return s.strip() or name[:60]
import datetime
import os


def us(v):
    if not isinstance(v, (int, float)):
        return str(v)
    if v >= 1_000_000:
        return f"{v/1_000_000:.2f} s"
    if v >= 1_000:
        return f"{v/1_000:.2f} ms"
    return f"{v:.1f} μs"


def pct(v):
    return f"{v:.1f}%" if isinstance(v, (int, float)) else str(v)


SEV = {
    "critical": ("#d93025", "Critical"),
    "warning":  ("#e37400", "Warning"),
    "good":     ("#1e8e3e", "Good"),
}


def classify(gpu_util, sm_util):
    if not isinstance(gpu_util, (int, float)):
        return "warning"
    if gpu_util < 50:
        return "critical"
    if gpu_util < 80:
        return "warning"
    if isinstance(sm_util, (int, float)) and sm_util < 80:
        return "warning"
    return "good"


def bar(label, value, total, color):
    w = (value / total * 100) if total else 0
    return f"""
    <div class="bar-row">
      <div class="bar-label">{html.escape(label)}</div>
      <div class="bar-track"><div class="bar-fill" style="width:{w:.1f}%;background:{color}"></div></div>
      <div class="bar-val">{us(value)} <span class="bar-pct">({w:.1f}%)</span></div>
    </div>"""


def _extract_pid(proc_key, proc):
    """proc_key looks like "1313787:[GPU0] Offline Profiling Task"; grab the numeric pid before the colon.
    Fallback: extract from AIProf_<pid>.json in traceUrl."""
    m = re.match(r"^\s*(\d+)", str(proc_key))
    if m:
        return m.group(1)
    url = proc.get("traceUrl", "") or ""
    m = re.search(r"_(\d+)\.json", url)
    return m.group(1) if m else ""


def _fmt_size_mb(v):
    if not isinstance(v, (int, float)):
        return str(v)
    if v >= 1024:
        return f"{v/1024:.2f} GB"
    return f"{v:.1f} MB"


def render_snapshot_panel(proc, analysis_id=""):
    """Memory-snapshot panel — data comes from fields written into proc by
    analysis.py::process_snapshot_file."""
    mm_url = proc.get("mmSnapUrl") or ""
    if not mm_url:
        return ""
    mem_summary = proc.get("memSummary") or ""
    oom = proc.get("memOom") or {}
    oom_items = oom.get("oomItem") or []
    spikes = proc.get("memspikes") or []

    badges = "".join(
        f'<span class="chip">{html.escape(label)} <b>{val}</b></span>'
        for label, val in [
            ("OOM 事件", len(oom_items)),
            ("显存尖峰", len(spikes)),
        ]
    )

    spike_rows = ""
    for i, sp in enumerate(spikes[:15], 1):
        stack = str(sp.get("stack") or "")
        stack_short = stack.split("\n")[0][:220] if stack else ""
        spike_rows += f"""<tr>
          <td class="rank">{i}</td>
          <td>{html.escape(str(sp.get('time', '')))}</td>
          <td>{_fmt_size_mb(sp.get('size'))}</td>
          <td class="kname"><code>{html.escape(str(sp.get('addr', '')))}</code></td>
          <td class="kname" title="{html.escape(stack)}">{html.escape(stack_short)}</td>
        </tr>"""
    spike_tbl = (
        f'<table class="tbl"><thead><tr>'
        f'<th>#</th><th>时间</th><th>大小</th><th>地址</th><th>调用栈（首帧）</th>'
        f'</tr></thead><tbody>{spike_rows}</tbody></table>'
        if spike_rows else "<p class='muted'>无显存尖峰事件</p>"
    )

    oom_note = ""
    if oom_items:
        oom_note = f'<p style="color:#d93025;margin:6px 0 12px">⚠️ 检测到 {len(oom_items)} 条 OOM 事件</p>'
    elif oom.get("summary"):
        oom_note = f'<p class="muted" style="margin:6px 0 12px">{html.escape(str(oom.get("summary")))}</p>'

    viz_btn = ""
    iframe_block = ""
    if analysis_id:
        pid = _extract_pid_for_snapshot(proc)
        pid_qs = f"&pid={html.escape(pid)}" if pid else ""
        embed_url = f"/ai_observable/result?analysisId={html.escape(analysis_id)}{pid_qs}&embed=memviz"
        full_url = f"/ai_observable/result?analysisId={html.escape(analysis_id)}{pid_qs}&embed=memviz"
        viz_btn = (
            f'<a class="perfetto-btn" style="background:#1e8e3e" target="_blank" rel="noreferrer" '
            f'href="{full_url}">🧠 在完整 MemoryViz 中打开</a>'
        )
        iframe_block = (
            f'<div style="margin-top:12px;border:1px solid #e8eaed;border-radius:8px;overflow:hidden">'
            f'<iframe src="{embed_url}" '
            f'style="width:100%;height:720px;border:0;display:block" '
            f'title="CUDA Memory Snapshot" loading="lazy"></iframe>'
            f'</div>'
        )
    dl_btn = (
        f'<a class="perfetto-btn" style="background:#5f6368" target="_blank" rel="noreferrer" '
        f'href="{html.escape(mm_url)}">⬇️ 下载 snapshot (pickle.gz)</a>'
    )

    return f"""
        <div class="panel">
          <h3>🧠 CUDA 显存快照分析</h3>
          <div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:12px">
            {badges}{viz_btn}{dl_btn}
          </div>
          {f'<p class="muted" style="margin:0 0 10px">{html.escape(str(mem_summary))}</p>' if mem_summary else ''}
          {oom_note}
          <h4 style="font-size:13px;color:#3c4043;margin:14px 0 8px">显存尖峰事件 <span class="muted">· 共 {len(spikes)} 条{'（截取前 15）' if len(spikes)>15 else ''}</span></h4>
          {spike_tbl}
          {iframe_block}
        </div>"""


def _extract_pid_for_snapshot(proc):
    """Pull pid out of proc — the AIProf_<pid>.pickle.gz suffix of mmSnapUrl is the most reliable."""
    url = proc.get("mmSnapUrl") or ""
    m = re.search(r"_(\d+)\.pickle", url)
    if m:
        return m.group(1)
    url = proc.get("traceUrl") or ""
    m = re.search(r"_(\d+)\.json", url)
    return m.group(1) if m else ""


def render_process_card(proc_key, proc, analysis_id=""):
    ov = proc.get("overview", {})
    s = ov.get("summary", {})
    det = ov.get("detail", {})
    gu = s.get("GPU_utilization", {}).get("GPU_total", {})
    gpu_util = gu.get("GPU_utilization", "N/A")
    sm_util = gu.get("SM_utilization", "N/A")
    sev = classify(gpu_util, sm_util)
    sev_color, sev_label = SEV[sev]
    conclusion = s.get("conclusion", "")
    dev = (s.get("device_info") or [{}])[0]
    ed = s.get("execution_delay", {})
    ed_total = ed.get("total", 0)

    delay_colors = {
        "cpu_op": "#4285f4", "cuda_runtime": "#a142f4", "kernel": "#1e8e3e",
        "overhead": "#9aa0a6", "gpu_memset": "#f4b400", "gpu_memcpy": "#e37400",
    }
    delay_items = sorted(
        [(k, v) for k, v in ed.items() if k != "total" and isinstance(v, (int, float))],
        key=lambda x: -x[1],
    )
    delay_bars = "".join(bar(k, v, ed_total, delay_colors.get(k, "#5f6368")) for k, v in delay_items)

    tc = det.get("tensorCores_usage", {}) or {}
    tc_svc = tc.get("service_time", 0) if isinstance(tc.get("service_time"), (int, float)) else 0
    tc_unused = tc.get("unused_time", 0) if isinstance(tc.get("unused_time"), (int, float)) else 0
    tc_total = tc_svc + tc_unused
    if tc_total:
        tc_bars = (bar("有效计算 (service)", tc_svc, tc_total, "#1e8e3e")
                   + bar("空转 (unused)", tc_unused, tc_total, "#d93025"))
    else:
        tc_bars = "<p class='muted'>无 TensorCore 数据</p>"

    ks = det.get("kernel_stistics", {}) or {}
    ks_num = {k: v for k, v in ks.items() if isinstance(v, (int, float))}
    ks_total = sum(ks_num.values()) or 1
    ks_colors = {"Computation": "#1e8e3e", "Memory": "#4285f4", "Communication": "#a142f4"}
    ks_bars = "".join(bar(k, v, ks_total, ks_colors.get(k, "#5f6368")) for k, v in ks_num.items())

    kd = det.get("kernel_details", {}) or {}
    kd_list = list(kd.values()) if isinstance(kd, dict) else []
    klist = sorted(kd_list, key=lambda x: -(x.get("total_delay_us", 0) or 0))[:10]
    krows = ""
    for i, k in enumerate(klist, 1):
        name = k.get("kernel_name", "") or ""
        short = _shorten_kernel(name)
        smu = k.get("SM_utilization", "N/A")
        if isinstance(smu, (int, float)):
            smcolor = "#d93025" if smu < 50 else ("#e37400" if smu < 80 else "#1e8e3e")
        else:
            smcolor = "#5f6368"
        krows += f"""<tr>
          <td class="rank">{i}</td>
          <td class="kname" title="{html.escape(name)}">{html.escape(short)}</td>
          <td>{k.get('run_times', '')}</td>
          <td>{us(k.get('total_delay_us', 0))}</td>
          <td>{us(k.get('avg_delay_us', 0))}</td>
          <td style="color:{smcolor};font-weight:600">{pct(smu)}</td>
          <td>{k.get('use_tensorCore', '')}</td>
          <td>{k.get('block', '')} / {k.get('grid', '')}</td>
        </tr>"""

    gpu_rows = ""
    for gid, g in s.get("GPU_utilization", {}).items():
        gpu_rows += f"""<tr>
          <td>{html.escape(str(gid))}</td>
          <td>{pct(g.get('GPU_utilization'))}</td>
          <td>{pct(g.get('SM_utilization'))}</td>
          <td>{g.get('active_blocks_per_SM', '')}</td>
          <td>{g.get('active_warps_per_SM', '')}</td>
        </tr>"""

    trace_mb = proc.get("traceFileSize", 0)
    trace_disp = f"{trace_mb:.1f} MB" if isinstance(trace_mb, (int, float)) else str(trace_mb)

    pid = _extract_pid(proc_key, proc)
    perfetto_btn = ""
    if analysis_id and pid:
        q = f"analysisId={html.escape(analysis_id)}&pid={html.escape(pid)}"
        perfetto_btn = (
            f'<a class="perfetto-btn" target="_blank" rel="noreferrer" '
            f'href="/aiprof/perfetto.html?{q}">🔬 在 Perfetto 中打开 Trace</a>'
        )

    return f"""
      <section class="proc-card">
        <div class="proc-head">
          <h2>{html.escape(proc_key)}</h2>
          {perfetto_btn}
          <span class="badge" style="background:{sev_color}">{sev_label}</span>
        </div>
        <div class="meta-grid">
          <div><span class="mk">设备</span>{html.escape(str(dev.get('device_name', 'Unknown')))}</div>
          <div><span class="mk">显存</span>{dev.get('memory_used', '?')} / {dev.get('memory_size', '?')} MB</div>
          <div><span class="mk">Trace 大小</span>{trace_disp}</div>
          <div><span class="mk">GPU 利用率</span><b style="color:{sev_color}">{pct(gpu_util)}</b></div>
          <div><span class="mk">SM 利用率</span><b>{pct(sm_util)}</b></div>
        </div>

        <div class="conclusion" style="border-left-color:{sev_color}">
          <div class="conclusion-title">📋 诊断结论</div>
          <p>{html.escape(str(conclusion))}</p>
        </div>

        <div class="two-col">
          <div class="panel">
            <h3>⏱ 执行延迟分布</h3>
            {delay_bars or "<p class='muted'>无延迟数据</p>"}
          </div>
          <div class="panel">
            <h3>🎯 TensorCore 使用率</h3>
            {tc_bars}
            <h3 style="margin-top:18px">🧮 Kernel 类别耗时</h3>
            {ks_bars or "<p class='muted'>无 kernel 分类数据</p>"}
          </div>
        </div>

        <div class="panel">
          <h3>🔥 GPU 利用率明细</h3>
          <table class="tbl"><thead><tr>
            <th>GPU</th><th>GPU 利用率</th><th>SM 利用率</th><th>Blocks/SM</th><th>Warps/SM</th>
          </tr></thead><tbody>{gpu_rows}</tbody></table>
        </div>

        <div class="panel">
          <h3>⚙️ Top 10 Kernel（按总耗时）<span class="muted">· 共 {len(kd_list)} 个 kernel</span></h3>
          <table class="tbl kernel-tbl"><thead><tr>
            <th>#</th><th>Kernel</th><th>调用次数</th><th>总耗时</th><th>平均</th><th>SM%</th><th>TC</th><th>block/grid</th>
          </tr></thead><tbody>{krows or '<tr><td colspan=8 class=muted>无 kernel 数据</td></tr>'}</tbody></table>
        </div>
        {render_snapshot_panel(proc, analysis_id)}
      </section>"""


CSS = """
  * { box-sizing: border-box; }
  body { font-family:-apple-system,BlinkMacSystemFont,"PingFang SC","Microsoft YaHei",sans-serif;
    margin:0; background:#f1f3f4; color:#202124; line-height:1.5; }
  .topbar { background:linear-gradient(135deg,#1a73e8,#174ea6); color:#fff; padding:28px 40px; }
  .topbar h1 { margin:0; font-size:24px; font-weight:600; }
  .topbar .sub { opacity:.85; font-size:13px; margin-top:6px; }
  .wrap { max-width:1140px; margin:24px auto; padding:0 20px; }
  .proc-card { background:#fff; border-radius:12px; padding:24px 28px; margin-bottom:24px;
    box-shadow:0 1px 3px rgba(0,0,0,.08); }
  .proc-head { display:flex; align-items:center; gap:12px; border-bottom:1px solid #e8eaed; padding-bottom:14px; }
  .proc-head h2 { font-size:16px; margin:0; flex:1; word-break:break-all; }
  .badge { color:#fff; padding:4px 12px; border-radius:20px; font-size:12px; font-weight:600; white-space:nowrap; }
  .perfetto-btn { display:inline-flex; align-items:center; gap:6px; background:#1a73e8; color:#fff;
    text-decoration:none; padding:6px 14px; border-radius:6px; font-size:13px; font-weight:500;
    white-space:nowrap; transition:background .2s; }
  .perfetto-btn:hover { background:#174ea6; }
  .meta-grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr)); gap:14px; margin:18px 0; }
  .meta-grid .mk { display:block; font-size:11px; color:#5f6368; text-transform:uppercase; letter-spacing:.5px; margin-bottom:2px; }
  .meta-grid div { font-size:15px; }
  .conclusion { background:#f8f9fa; border-left:4px solid #1a73e8; padding:14px 18px; border-radius:6px; margin:16px 0; }
  .conclusion-title { font-weight:600; font-size:14px; margin-bottom:6px; }
  .conclusion p { margin:0; font-size:14px; }
  .two-col { display:grid; grid-template-columns:1fr 1fr; gap:20px; }
  @media(max-width:800px){ .two-col{grid-template-columns:1fr} }
  .panel { margin-top:20px; }
  .panel h3 { font-size:14px; margin:0 0 12px; color:#3c4043; }
  .bar-row { display:flex; align-items:center; gap:10px; margin:7px 0; font-size:13px; }
  .bar-label { width:130px; text-align:right; color:#5f6368; flex-shrink:0; }
  .bar-track { flex:1; background:#e8eaed; border-radius:4px; height:16px; overflow:hidden; }
  .bar-fill { height:100%; border-radius:4px; transition:width .3s; }
  .bar-val { width:150px; font-variant-numeric:tabular-nums; color:#202124; }
  .bar-pct { color:#80868b; font-size:12px; }
  .tbl { width:100%; border-collapse:collapse; font-size:13px; }
  .tbl th { text-align:left; padding:9px 10px; background:#f8f9fa; color:#5f6368; font-weight:600;
    border-bottom:2px solid #e8eaed; font-size:12px; white-space:nowrap; }
  .tbl td { padding:9px 10px; border-bottom:1px solid #f1f3f4; }
  .tbl tbody tr:hover { background:#f8f9fa; }
  .rank { color:#80868b; font-weight:600; }
  .kname { font-family:"SF Mono",Consolas,monospace; font-size:12px; max-width:560px;
    word-break:break-all; white-space:normal; line-height:1.35; }
  .muted { color:#80868b; font-weight:400; font-size:12px; }
  .chip { display:inline-flex; align-items:center; gap:4px; background:#f1f3f4; color:#3c4043;
    padding:4px 10px; border-radius:16px; font-size:12px; }
  .chip b { color:#1a73e8; font-variant-numeric:tabular-nums; }
  code { background:#f1f3f4; padding:1px 5px; border-radius:3px; font-size:11px; }
  .footer { text-align:center; color:#80868b; font-size:12px; padding:20px; }
"""


def build_html(summary_data, generated_at=None, analysis_id=""):
    """summary_data is the dict after unwrapping the outer wrapper (keys = process name or aggregationUrl)."""
    procs = {k: v for k, v in summary_data.items()
             if k != "aggregationUrl" and isinstance(v, dict)}
    cards = "".join(render_process_card(k, v, analysis_id) for k, v in procs.items())
    ts = generated_at or datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S")

    return f"""<!DOCTYPE html><html lang="zh-CN"><head><meta charset="utf-8">
<title>AIProf 性能分析报告</title>
<style>{CSS}</style></head><body>
<div class="topbar">
  <h1>AIProf GPU 性能分析报告</h1>
  <div class="sub">规则式聚合分析 · 生成于 {ts} · 数据源：Analysis_Summary.json</div>
</div>
<div class="wrap">
  {cards or '<div class="proc-card"><p class="muted">未发现进程数据</p></div>'}
  <div class="footer">AIProf · 本报告由规则式分析引擎自动生成，无 LLM 参与 · 深度智能诊断请使用 AI Agent</div>
</div>
</body></html>"""


def render(summary_path, out_path=None):
    with open(summary_path, "r", encoding="utf-8") as f:
        raw = json.load(f)
    # Supports two shapes: an outer {code, message, data:"<json string>"} wrapper,
    # or the raw dict written directly by analysis.py.
    if isinstance(raw, dict) and "data" in raw and isinstance(raw["data"], str):
        data = json.loads(raw["data"])
    else:
        data = raw
    # analysisId = directory name that contains summary (dashboardServer uses
    # <RESULT_DIR>/<uuid>/ layout). Only inject the Perfetto button when the
    # directory name is a UUID, so arbitrary paths cannot be spliced into the link.
    parent = os.path.basename(os.path.dirname(os.path.abspath(summary_path)))
    uuid_re = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
    analysis_id = parent if uuid_re.match(parent) else ""
    doc = build_html(data, analysis_id=analysis_id)
    if out_path is None:
        out_path = os.path.join(os.path.dirname(summary_path) or ".", "Analysis_Report.html")
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(doc)
    return out_path


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: report.py <Analysis_Summary.json> [output.html]", file=sys.stderr)
        sys.exit(1)
    out = render(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else None)
    print(out)
