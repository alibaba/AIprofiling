import os
import sys
import glob
import json
import time
import queue
import bisect
import gzip
import threading
import traceback
import ijson
import pickle
import multiprocessing
import logging
import argparse
import concurrent.futures
from concurrent.futures import ProcessPoolExecutor
from collections import defaultdict
from gmem import MemoryAnalyzer
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - [%(threadName)s] - %(message)s')
logger = logging.getLogger("OfflineAnalyzer")
GPU_UTILIZATION_THRESHOLD = 80
SM_UTILIZATION_THRESHOLD  = 80
# Bucket width for [memory] instant events, in microseconds. 15ms was too
# coarse for the short (few-second) captures the injector produces — it left
# only a handful of buckets to draw a curve from.
MEMORY_INTERVAL           = 5000
# Upper bound on points sent to the UI; longer traces are strided down.
MEMORY_MAX_POINTS         = 600
# ---------------------------------------------------------------------------
# DCGM hardware metrics
# Counter events are written to traceEvents as
#   {"name":"counter","pid":<pid>,"ph":"C","ts":<us>,"dur":100,
#    "args":{"DCGM_FI_DEV_GPU_TEMP_gpu0":61}}
# Host-level samples carry a _gpu<N> suffix; per-process samples use the bare field name.
# The open-source CollectionFramework no longer ships a built-in DCGM collector;
# the parsing code below is kept to remain compatible with chrome-trace reports
# produced by external tooling (e.g. Alibaba Cloud OS console) that include DCGM
# metrics.
# ---------------------------------------------------------------------------
DCGM_TEMP_THRESHOLD_C      = 85
DCGM_POWER_RATIO_THRESHOLD = 0.95
DCGM_CLOCK_DROP_THRESHOLD  = 0.15
# Monotonically increasing counters (ENERGY / VIOLATION / PCIE_REPLAY / ECC)
# use interval delta when summarised; other fields are treated as gauges
# reporting avg/max/min — see _summarize_hw_metrics.
NVML_THROTTLE_REASONS = (
    (0x0001, "GpuIdle"), (0x0002, "ApplicationsClocksSetting"),
    (0x0004, "SwPowerCap"), (0x0008, "HwSlowdown"), (0x0010, "SyncBoost"),
    (0x0020, "SwThermalSlowdown"), (0x0040, "HwThermalSlowdown"),
    (0x0080, "HwPowerBrakeSlowdown"), (0x0100, "DisplayClockSetting"),
)
# Only these reasons represent real performance loss; GpuIdle/ApplicationsClocksSetting are normal states
NVML_THROTTLE_HARMFUL = {
    "SwPowerCap", "HwSlowdown", "SwThermalSlowdown",
    "HwThermalSlowdown", "HwPowerBrakeSlowdown",
}
cpu_count = max(1, multiprocessing.cpu_count() - 2)
ProcessPool = None
def fetch_meta_from_file(filepath):
    deviceProperties, stepEndTime, indicators, statStep = None, [], [], []
    try:
        with open(filepath, 'rb') as f:
            for key, value in ijson.kvitems(f, '', use_float=True):
                if key == 'deviceProperties': deviceProperties = value
                elif key == 'stepEndTime': stepEndTime = value
                elif key == 'indicators': indicators = value
                elif key == 'statStep': statStep = value
                
                if deviceProperties is not None and stepEndTime is not None and statStep is not None:
                    break
        return deviceProperties, stepEndTime, indicators, statStep
    except Exception as e:
        logger.error(f"Failed to extract meta from {filepath}: {e}")
        return None, [], [], []
def fetch_trace_events_from_file(filepath):
    try:
        with open(filepath, 'rb') as f:
            for item in ijson.items(f, 'traceEvents.item', use_float=True):
                yield item
    except Exception as e:
        logger.error(f"Failed to parse traceEvents from {filepath}: {e}")
def _assign_event_to_period(t, stepEndTime, dur):
    if not stepEndTime: return -1
    idx = bisect.bisect_right(stepEndTime, t)
    if idx == 0 or idx == len(stepEndTime): return -1
    if t + dur > stepEndTime[idx]: return -1
    return idx - 1
def _cuda_stream_type(d):
    cat, name = d.get("cat", ""), d.get("name", "")
    if cat in ("gpu_memset", "gpu_memcpy"): return "Memory"
    if "nccl" in name or "cross_device" in name: return "Communication"
    return "Computation"
def __get_op_type(kernelName):
    optable = {
        "axpy": "向量乘法加法", "dot": "向量点积", "nrm2": "向量2范数", 
        "scal": "向量缩放", "trsm": "三角矩阵求解", "sgemm": "单精度矩阵乘法",
        "dgemm": "双精度矩阵乘法", "cgemm": "单精度复数矩阵乘法", 
        "zgemm": "双精度复数矩阵乘法", "gemm": "通用矩阵乘法"
    }
    for k, v in optable.items():
        if k in kernelName: return v
    return "N/A"
def _static_gpu_util(d, statKernel, statPerGpu, statTensor):
    name = d["name"]
    if name not in statKernel:
        statKernel[name] = {
            "kernel_name": name, "type_of_operation": __get_op_type(name),
            "run_times": 0, "use_tensorCore": "是" if "tensor" in name else "否",
            "total_delay_us": 0, "max_delay_us": 0, "avg_delay_us": 0, "min_delay_us": sys.maxsize
        }
    
    kf = statKernel[name]
    kf["run_times"] += 1
    kf["total_delay_us"] = round(kf["total_delay_us"] + d["dur"], 3)
    kf["avg_delay_us"] = round(kf["total_delay_us"] / kf["run_times"], 3)
    kf["max_delay_us"] = max(kf["max_delay_us"], d["dur"])
    kf["min_delay_us"] = min(kf["min_delay_us"], d["dur"])
    if kf["use_tensorCore"] == "是":
        statTensor["service_time"] = round(statTensor["service_time"] + d["dur"], 3)
    else:
        statTensor["unused_time"] = round(statTensor["unused_time"] + d["dur"], 3)
    if "args" in d and "device" in d["args"]:
        gpuId = d["args"]["device"]
        if gpuId not in statPerGpu:
            statPerGpu[gpuId] = {
                "SM_utilization": 0, "active_blocks_per_SM": 0,
                "active_warps_per_SM": 0, "total_delay_us": 0, "run_times": 0
            }
        
        perGpu = statPerGpu[gpuId]
        perGpu["run_times"] += 1
        perGpu["total_delay_us"] = round(perGpu["total_delay_us"] + d["dur"], 3)
        for ele in ["block", "grid"]:
            if ele not in kf or not kf[ele]:
                kf[ele] = str(d["args"].get(ele, []))
        for oriK, newK in {
            "est. achieved occupancy %": "SM_utilization",
            "blocks per SM": "active_blocks_per_SM",
            "warps per SM": "active_warps_per_SM",
            "shared memory": "shared_memory_size",
            "registers per thread": "registers_per_thread"
        }.items():
            if oriK not in d["args"]:
                if newK in perGpu: perGpu[newK] = "N/A"
                kf[newK] = "N/A"
                continue
            
            kf.setdefault(newK, 0)
            kf[newK] = round((kf[newK] * (kf["run_times"] - 1) + d["args"][oriK]) / kf["run_times"], 3)
            if newK in perGpu:
                perGpu[newK] = round((perGpu[newK] * (perGpu["run_times"] - 1) + d["args"][oriK]) / perGpu["run_times"], 3)
def _split_dcgm_key(key):
    """Split into (field_name, device_key). Host-level samples look like DCGM_FI_DEV_GPU_TEMP_gpu0; per-process samples have no suffix."""
    idx = key.rfind("_gpu")
    if idx > 0 and key[idx + 4:].isdigit():
        return key[:idx], "gpu" + key[idx + 4:]
    return key, "process"
def _stat_counter_event(d, statCounter):
    """Accumulate ph=="C" counter events (DCGM / RDMA). These are instantaneous
    samples rather than intervals, so they must never fall into perCatDelay —
    they carry no cat, and previously would all pile up under statPerCat[""]."""
    args = d.get("args") or {}
    ts = d.get("ts", 0)
    for rawKey, value in args.items():
        if rawKey == "name" or isinstance(value, bool) or not isinstance(value, (int, float)):
            continue
        field, dev = _split_dcgm_key(rawKey)
        bucket = statCounter.setdefault(dev, {})
        s = bucket.get(field)
        if s is None:
            bucket[field] = {
                "count": 1, "sum": value, "max": value, "min": value,
                "first": value, "last": value, "firstTs": ts, "lastTs": ts,
                "bits": int(value) if field.endswith("EVENT_REASONS") else 0,
            }
            continue
        s["count"] += 1
        s["sum"] += value
        s["max"] = max(s["max"], value)
        s["min"] = min(s["min"], value)
        if ts >= s["lastTs"]: s["last"], s["lastTs"] = value, ts
        if ts <= s["firstTs"]: s["first"], s["firstTs"] = value, ts
        if field.endswith("EVENT_REASONS"): s["bits"] |= int(value)
def _dcgm_gauge(stat, field, scale=1.0, ndigits=2):
    s = stat.get(field)
    if not s or not s.get("count"): return None
    return {
        "avg": round(s["sum"] / s["count"] * scale, ndigits),
        "max": round(s["max"] * scale, ndigits),
        "min": round(s["min"] * scale, ndigits),
    }
def _dcgm_delta(stat, field, scale=1.0, ndigits=2):
    """Take the interval delta for a monotonic counter; if wrap-around or a
    collector restart yields a negative value, fall back to the last value."""
    s = stat.get(field)
    if not s: return None
    delta = s["last"] - s["first"]
    if delta < 0: delta = s["last"]
    return round(delta * scale, ndigits)
def _dcgm_last(stat, field):
    s = stat.get(field)
    return None if not s else s["last"]
def _decode_throttle_reasons(bits):
    return [name for mask, name in NVML_THROTTLE_REASONS if bits & mask]
def _summarize_hw_metrics(statCounter):
    """Collapse DCGM counter samples into per-card hardware metrics plus
    throttle/fault findings. Returns (hardware_metrics, hardware_findings)."""
    metrics, findings = {}, []
    for dev, stat in sorted(statCounter.items()):
        if not any(k.startswith("DCGM_FI_") for k in stat): continue
        m = {"samples": max(s.get("count", 0) for s in stat.values())}
        for key, field in (
            ("utilization_pct",      "DCGM_FI_DEV_GPU_UTIL"),
            ("mem_copy_util_pct",    "DCGM_FI_DEV_MEM_COPY_UTIL"),
            ("fb_used_mib",          "DCGM_FI_DEV_FB_USED"),
            ("fb_free_mib",          "DCGM_FI_DEV_FB_FREE"),
            ("temperature_c",        "DCGM_FI_DEV_GPU_TEMP"),
            ("memory_temperature_c", "DCGM_FI_DEV_MEMORY_TEMP"),
            ("power_draw_w",         "DCGM_FI_DEV_POWER_USAGE"),
            ("sm_clock_mhz",         "DCGM_FI_DEV_SM_CLOCK"),
            ("mem_clock_mhz",        "DCGM_FI_DEV_MEM_CLOCK"),
            ("sm_active_ratio",      "DCGM_FI_PROF_SM_ACTIVE"),
            ("sm_occupancy_ratio",   "DCGM_FI_PROF_SM_OCCUPANCY"),
        ):
            g = _dcgm_gauge(stat, field)
            if g is not None: m[key] = g
        for key, field in (("fb_total_mib",  "DCGM_FI_DEV_FB_TOTAL"),
                           ("power_limit_w", "DCGM_FI_DEV_ENFORCED_POWER_LIMIT")):
            v = _dcgm_last(stat, field)
            if v is not None: m[key] = v
        energy = _dcgm_delta(stat, "DCGM_FI_DEV_TOTAL_ENERGY_CONSUMPTION", 0.001, 3)
        if energy is not None: m["energy_consumed_j"] = energy
        if m.get("fb_total_mib") and m.get("fb_used_mib"):
            m["memory_utilization_pct"] = round(m["fb_used_mib"]["max"] / m["fb_total_mib"] * 100, 2)
        reasonBits = (stat.get("DCGM_FI_DEV_CLOCKS_EVENT_REASONS") or {}).get("bits", 0)
        reasons = _decode_throttle_reasons(reasonBits)
        throttle = {
            "reasons": reasons,
            "harmful_reasons": [r for r in reasons if r in NVML_THROTTLE_HARMFUL],
            "power_violation_us": _dcgm_delta(stat, "DCGM_FI_DEV_POWER_VIOLATION"),
            "thermal_violation_us": _dcgm_delta(stat, "DCGM_FI_DEV_THERMAL_VIOLATION"),
        }
        if reasons or throttle["power_violation_us"] or throttle["thermal_violation_us"]:
            m["throttle"] = throttle
        errors = {
            "xid_last": _dcgm_last(stat, "DCGM_FI_DEV_XID_ERRORS"),
            "ecc_sbe_delta": _dcgm_delta(stat, "DCGM_FI_DEV_ECC_SBE_VOL_TOTAL"),
            "ecc_dbe_delta": _dcgm_delta(stat, "DCGM_FI_DEV_ECC_DBE_VOL_TOTAL"),
            "pcie_replay_delta": _dcgm_delta(stat, "DCGM_FI_DEV_PCIE_REPLAY_COUNTER"),
        }
        if any(v for v in errors.values()): m["errors"] = errors
        metrics[dev] = m
        def add(level, kind, msg):
            findings.append({"level": level, "device": dev, "type": kind, "message": msg})
        if throttle["harmful_reasons"]:
            add("warning", "throttle",
                f"{dev} 期间发生降频，原因位：{'/'.join(throttle['harmful_reasons'])}，"
                f"该时段的 kernel 耗时不能直接与未降频时段横向对比")
        if throttle["thermal_violation_us"]:
            add("warning", "thermal_throttle",
                f"{dev} 因温度触发降频累计 {throttle['thermal_violation_us']} us，请检查散热与机位风道")
        if throttle["power_violation_us"]:
            add("warning", "power_throttle",
                f"{dev} 因功耗墙触发降频累计 {throttle['power_violation_us']} us，可评估提高功耗上限")
        if errors["ecc_dbe_delta"]:
            add("critical", "ecc",
                f"{dev} 出现 {errors['ecc_dbe_delta']} 个双比特 ECC 错误，属不可纠正故障，建议立刻隔离该卡")
        elif errors["ecc_sbe_delta"]:
            add("warning", "ecc",
                f"{dev} 出现 {errors['ecc_sbe_delta']} 个单比特 ECC 错误，可纠正但需持续观察")
        if errors["xid_last"]:
            add("critical", "xid",
                f"{dev} 上报 XID {int(errors['xid_last'])}，通常对应驱动/硬件异常，请结合 dmesg 定位")
        if errors["pcie_replay_delta"]:
            add("warning", "pcie",
                f"{dev} PCIe 重传 {errors['pcie_replay_delta']} 次，链路质量下降会拖慢 H2D/D2H 拷贝")
        temp = m.get("temperature_c")
        if temp and temp["max"] >= DCGM_TEMP_THRESHOLD_C:
            add("warning", "temperature",
                f"{dev} 峰值温度 {temp['max']} C 已达 {DCGM_TEMP_THRESHOLD_C} C 降频阈值附近")
        power, limit = m.get("power_draw_w"), m.get("power_limit_w")
        if power and limit and power["max"] >= limit * DCGM_POWER_RATIO_THRESHOLD:
            add("warning", "power",
                f"{dev} 峰值功耗 {power['max']} W 触及 {limit} W 上限（均值 {power['avg']} W），"
                f"算力可能被功耗墙压制")
        clock = m.get("sm_clock_mhz")
        if clock and clock["max"] and clock["min"] < clock["max"] * (1 - DCGM_CLOCK_DROP_THRESHOLD):
            add("info", "clock",
                f"{dev} SM 频率在 {clock['min']}~{clock['max']} MHz 间波动，"
                f"跌幅超过 {int(DCGM_CLOCK_DROP_THRESHOLD * 100)}%，请结合降频原因位判断是否为空闲导致")
        occ = m.get("sm_occupancy_ratio")
        if occ and occ["avg"] < SM_UTILIZATION_THRESHOLD / 100.0 / 2:
            add("info", "occupancy",
                f"{dev} 硬件实测 SM occupancy 均值仅 {round(occ['avg'] * 100, 2)}%，"
                f"可与 CUPTI 估算的 est. achieved occupancy 交叉校验 grid/block 配置")
    return metrics, findings
def _merge_counter_stat(old, new):
    """Merge raw counter statistics across files so multi-process / multi-card
    results can be aggregated again downstream."""
    merged = {dev: {f: dict(s) for f, s in stat.items()} for dev, stat in (old or {}).items()}
    for dev, stat in (new or {}).items():
        bucket = merged.setdefault(dev, {})
        for field, s in stat.items():
            o = bucket.get(field)
            if o is None:
                bucket[field] = dict(s)
                continue
            o["count"] += s["count"]
            o["sum"] += s["sum"]
            o["max"] = max(o["max"], s["max"])
            o["min"] = min(o["min"], s["min"])
            o["bits"] = o.get("bits", 0) | s.get("bits", 0)
            if s["lastTs"] >= o["lastTs"]: o["last"], o["lastTs"] = s["last"], s["lastTs"]
            if s["firstTs"] <= o["firstTs"]: o["first"], o["firstTs"] = s["first"], s["firstTs"]
    return merged
def _statics_cpu_gpu_util(filepath, stepEndTime, statStep):
    statGPUUtil = {
        "kernel_details": {}, "perGpu": {},
        "tensorCores_usage": {"service_time": 0, "unused_time": 0},
        "kernel_stistics": {"Computation": 0, "Memory": 0, "Communication": 0},
        "memory_stistics": {"Allocated": [], "Reserved": [], "Time": []}
    }
    statPerCat = {}
    statCounter = {}
    endTs, startTs = 0, sys.maxsize
    # Bucket [memory] events by absolute ts // MEMORY_INTERVAL so the pass is
    # order-independent (merged traces are not guaranteed monotonic) and the
    # working set is bounded by bucket count rather than event count.
    memBuckets = {}
    for d in fetch_trace_events_from_file(filepath):
        startTs = min(startTs, d.get("ts", startTs))
        endTs = max(endTs, d.get("ts", endTs))

        cat = d.get("cat", "")
        if cat in {"python", "cpu", "syscall", "cuda", "Trace"} or "annotation" in cat:
            continue

        if cat == "cpu_instant_event" and d.get("name") == "[memory]" and d.get("args", {}).get("Device Type") == 1:
            args = d.get("args", {})
            reserved = args.get("Total Reserved")
            allocated = args.get("Total Allocated")
            if reserved is None or allocated is None:
                continue
            slot = int(d["ts"]) // MEMORY_INTERVAL
            cur = memBuckets.get(slot)
            if cur is None:
                memBuckets[slot] = [allocated, reserved]
            else:
                if allocated > cur[0]: cur[0] = allocated
                if reserved > cur[1]: cur[1] = reserved
            continue
        # Counter events (DCGM/RDMA samples) carry dur=100 but no cat. If they
        # slip through they end up accumulated into statPerCat[""], fabricating
        # a non-existent duration category.
        if d.get("ph") == "C":
            _stat_counter_event(d, statCounter)
            continue
        if "dur" not in d: continue
        if cat in ["kernel", "gpu_memcpy", "gpu_memset"]:
            period_idx = _assign_event_to_period(d["ts"], stepEndTime, d["dur"])
            kernelType = _cuda_stream_type(d)
            if period_idx >= 0 and statStep and period_idx < len(statStep):
                key = f"{kernelType}:{d.get('pid', '0')}"
                statStep[period_idx][key] = round(statStep[period_idx].get(key, 0) + d["dur"], 2)
            statGPUUtil["kernel_stistics"][kernelType] += round(d["dur"], 2)
        statPerCat[cat] = round(statPerCat.get(cat, 0) + d["dur"], 3)
        if cat.lower() == "kernel":
            _static_gpu_util(d, statGPUUtil["kernel_details"], statGPUUtil["perGpu"], statGPUUtil["tensorCores_usage"])
    statPerCat["total"] = round(endTs - startTs, 3) if startTs and endTs else 0
    _flush_memory_buckets(memBuckets, statGPUUtil["memory_stistics"])
    return {"statGPUUtil": statGPUUtil, "perCatDelay": statPerCat,
            "stepInfo": statStep, "counterStat": statCounter}
def _flush_memory_buckets(memBuckets, memStat, maxPoints=MEMORY_MAX_POINTS):
    """Turn ts-keyed buckets into the Allocated/Reserved/Time arrays the UI reads.

    Time is emitted in milliseconds relative to the first [memory] event. The
    previous implementation emitted a bucket counter (+= 10 per bucket) that the
    UI then labelled as seconds, so a 5s trace rendered as "3120s". It also
    dropped any bucket whose max matched the previous one, collapsing steady
    state to a handful of points and hiding the shape of the curve entirely.
    """
    if not memBuckets:
        return
    slots = sorted(memBuckets)
    # Downsample by taking the max within each stride so a long trace still
    # yields a bounded payload without losing peaks.
    stride = max(1, (len(slots) + maxPoints - 1) // maxPoints)
    base = slots[0]
    for i in range(0, len(slots), stride):
        window = slots[i:i + stride]
        allocated = max(memBuckets[s][0] for s in window)
        reserved = max(memBuckets[s][1] for s in window)
        # Bucket start, converted from microseconds to milliseconds.
        memStat["Allocated"].append(allocated)
        memStat["Reserved"].append(reserved)
        memStat["Time"].append(round((window[0] - base) * MEMORY_INTERVAL / 1000.0, 3))
def _get_summary(dataDeviceProperties, execDelay, perGpuUtil):
    processGpuUtil = {}
    totalDelay = execDelay.get("total", 0)
    totalGpuUtil = 0
    if totalDelay:
        kernel_time = execDelay.get("kernel", execDelay.get("Kernel", 0))
        totalGpuUtil = round(kernel_time / totalDelay * 100, 3)
    totalSMUtil = {"SM_utilization": 0, "active_blocks_per_SM": 0, "active_warps_per_SM": 0}
    for gpuId, gpuInfo in perGpuUtil.items():
        gpuUtil = round(gpuInfo["total_delay_us"] / totalDelay * 100, 3) if totalDelay else 0
        processGpuUtil[f"GPU{gpuId}"] = {
            "GPU_utilization": gpuUtil, "SM_utilization": gpuInfo["SM_utilization"],
            "active_blocks_per_SM": gpuInfo["active_blocks_per_SM"], "active_warps_per_SM": gpuInfo["active_warps_per_SM"]
        }
        for k in totalSMUtil:
            if gpuInfo[k] != "N/A" and totalSMUtil[k] != "N/A": totalSMUtil[k] += gpuInfo[k]
            else: totalSMUtil[k] = "N/A"
    if len(perGpuUtil) > 0:
        for k in totalSMUtil:
            if totalSMUtil[k] != "N/A": totalSMUtil[k] = round(totalSMUtil[k] / len(perGpuUtil), 3)
    processGpuUtil["GPU_total"] = {
        "GPU_utilization": totalGpuUtil, "SM_utilization": totalSMUtil["SM_utilization"],
        "active_blocks_per_SM": totalSMUtil["active_blocks_per_SM"], "active_warps_per_SM": totalSMUtil["active_warps_per_SM"]
    }
    device = []
    for gpuId in perGpuUtil.keys():
        for dev in (dataDeviceProperties or []):
            if dev.get("id") == gpuId:
                device.append({
                    "device_name": dev.get("name", "Unknown"),
                    "memory_size": dev.get("totalGlobalMem", 0),
                    "memory_used": dev.get("memoryUsed", 0),
                })
    conclusion = "本作业进程GPU总体利用率位于良好水位，如想进一步提升性能，可参考'GPU-Kernel执行分析'分布对SM利用率低于80%的kernel函数优化"
    if totalGpuUtil < GPU_UTILIZATION_THRESHOLD:
        maxDelayCat = max([k for k in execDelay if k.lower() not in {"kernel", "total"}], key=lambda k: execDelay[k], default="")
        if maxDelayCat:
            conclusion = f"本作业进程GPU总体利用率低，最高时间消耗在{maxDelayCat}部分，详细可参考'执行延迟'分布以及'CPU/GPU-Kernel Tracing分析'进行分析"
    elif totalSMUtil.get("SM_utilization", 0) != "N/A" and totalSMUtil["SM_utilization"] < SM_UTILIZATION_THRESHOLD:
        conclusion = "本作业进程GPU总体SM利用率低，详细可参考'GPU-Kernel执行分析'分布确认SM利用率低于50%的kernel函数，优化grid和block大小，提升并行度"
    return {"conclusion": conclusion, "device_info": device, "GPU_utilization": processGpuUtil, "execution_delay": execDelay}
def fill_memSnap_summary(summary, devs):
    if not devs: return "暂无任何显存设备信息"
    dev = devs[0]
    raw_mem = dev.get("memory_size", 0)
    
    total_mb = 0.0
    if isinstance(raw_mem, str):
        raw_mem = raw_mem.strip()
        if 'GiB' in raw_mem: total_mb = float(raw_mem.replace('GiB', '')) * 1024
        elif 'MiB' in raw_mem: total_mb = float(raw_mem.replace('MiB', ''))
        elif 'B' in raw_mem and not 'iB' in raw_mem: total_mb = float(raw_mem.replace('B', '')) / (1024**2)
    elif isinstance(raw_mem, (int, float)):
        if raw_mem > 10000000: total_mb = float(raw_mem) / (1024**2)
        else: total_mb = float(raw_mem)
    if total_mb == 0: return "设备显存信息提取异常，无法计算百分比"
    resTop = summary.get("top", {"size":0})
    resTop["util"] = resTop["size"] / total_mb
    risk1 = "显存压力较大,有OOM风险" if resTop["util"] > 0.94 else "显存压力不大,暂无OOM风险"
    line = f'torch最高使用了{resTop["size"]}MB 显存, torch显存使用率{resTop["util"]:.2%} {risk1}'
    resCurr = summary.get("curr", {"size":0})
    resCurr["util"] = resCurr["size"] / total_mb
    risk2 = "显存压力较大,有风险" if resCurr["util"] > 0.94 else "显存压力不大"
    line += f'; torch当前使用了{resCurr["size"]}MB 显存, torch显存使用率{resCurr["util"]:.2%} {risk2}'
    
    return line
def convert_to_resource_path(filepath):
    """Convert an absolute path into a /resource/-prefixed relative path.

    BASE_PATH lets the whole app be reverse-proxied under a path prefix
    (e.g. nginx `/aiprof-open/`). When set, the emitted URL is prefixed so
    the browser fetches through the same proxy path.
    """
    base = (os.environ.get('BASE_PATH') or '').rstrip('/')
    # Locate the position of result/ in the path
    if '/result/' in filepath:
        # Extract the portion after result/
        parts = filepath.split('/result/')
        if len(parts) > 1:
            return f"{base}/resource/app_observable/ai_observable/result/{parts[1]}"
    return filepath

def aggregate_python_stacks(filepath, max_nodes=4000, top_hot=15):
    """Rebuild python_function events from the trace into a tree the frontend
    flame graph can consume directly (``{name, value, children}``), using the
    (tid, ts, ts+dur) time-nesting relationship.

    Preferred correlation is via ``args.{Python id, Python parent id}``; if the
    trace does not carry those fields (older pyki / trimmed trace), fall back
    to "parent-child by full time containment within the same tid".

    - value unit: event ``dur`` (microseconds) summed along the stack; a parent
      node's value equals the sum of its children plus self.
    - Only the top ``max_nodes`` subtrees are kept, so hundreds of thousands of
      frames do not crush the browser.
    - Also returns the top ``top_hot`` hot functions (by cumulative dur) for
      the Profiling Agent.

    Returns ``(tree_or_none, hot_list)``; ``(None, [])`` when no python_function events exist.
    """
    events = []
    try:
        for e in fetch_trace_events_from_file(filepath):
            if not isinstance(e, dict) or e.get("cat") != "python_function":
                continue
            ts = e.get("ts")
            dur = e.get("dur")
            if ts is None or dur is None:
                continue
            events.append({
                "name": e.get("name") or "<?>",
                "tid": e.get("tid"),
                "ts": float(ts),
                "dur": float(dur),
                "args": e.get("args") or {},
            })
    except Exception as e:
        logger.error(f"Failed to parse python_function events from {filepath}: {e}")
        return None, []

    if not events:
        return None, []

    # Aggregate the hot-function table (across tids and stacks)
    hot_acc = defaultdict(float)
    for ev in events:
        hot_acc[ev["name"]] += ev["dur"]
    hot_sorted = sorted(hot_acc.items(), key=lambda kv: -kv[1])[:top_hot]
    hot_list = [{"name": n, "self_us": round(v, 2)} for n, v in hot_sorted]

    root = {"name": "all", "value": 0.0, "children": {}, "self": 0.0}

    def _insert_path(path_names, self_us):
        node = root
        for name in path_names:
            child = node["children"].get(name)
            if child is None:
                child = {"name": name, "value": 0.0, "children": {}, "self": 0.0}
                node["children"][name] = child
            node = child
        node["self"] += self_us

    # If every event carries a Python id, build via the id chain (most precise)
    has_id = all("Python id" in ev["args"] for ev in events[:200])
    if has_id:
        by_id = {ev["args"].get("Python id"): ev for ev in events if "Python id" in ev["args"]}
        for ev in events:
            args = ev["args"]
            pid_ = args.get("Python id")
            if pid_ is None:
                continue
            chain, cur, guard = [], ev, 0
            while cur is not None and guard < 1024:
                chain.append(cur["name"])
                pa = cur.get("args", {}).get("Python parent id")
                cur = by_id.get(pa) if pa is not None else None
                guard += 1
            chain.reverse()
            _insert_path(chain, ev["dur"])
    else:
        # Fallback: group by tid, sort each group by (ts asc, dur desc), and
        # rebuild the parent-child chain via a stack that tracks containment.
        by_tid = defaultdict(list)
        for ev in events:
            by_tid[ev["tid"]].append(ev)
        for tid, group in by_tid.items():
            group.sort(key=lambda e: (e["ts"], -e["dur"]))
            stack = []  # elements: (end_ts, name)
            for ev in group:
                end = ev["ts"] + ev["dur"]
                # Pop all ancestors that have already ended
                while stack and stack[-1][0] <= ev["ts"]:
                    stack.pop()
                path = [n for _, n in stack] + [ev["name"]]
                # Only attribute the leaf event's dur to the path so we don't double-count parent+child
                _insert_path(path, ev["dur"])
                stack.append((end, ev["name"]))

    def _finalize(node):
        total = node["self"]
        kids = []
        for c in node["children"].values():
            _finalize(c)
            total += c["value"]
            kids.append(c)
        node["value"] = total
        node["children"] = sorted(kids, key=lambda x: -x["value"])

    _finalize(root)

    if not root["children"] or root["value"] <= 0:
        return None, hot_list

    kept = 0
    def _trim(node):
        nonlocal kept
        kept += 1
        if kept >= max_nodes:
            node["children"] = []
            return
        for c in node["children"]:
            _trim(c)
    _trim(root)

    return root, hot_list


def process_profiling_file(pid, filepath):
    retData = {}
    deviceProperties, stepEndTime, _, statStep = fetch_meta_from_file(filepath)
    if not deviceProperties:
        deviceProperties = [{"name": "Unknown", "totalGlobalMem": 0, "memoryUsed": 0, "id": 0}]
    cpuGpuUtil = _statics_cpu_gpu_util(filepath, stepEndTime, statStep)
    if not cpuGpuUtil["perCatDelay"].get("total"):
        return retData
    util = cpuGpuUtil.pop("statGPUUtil")
    perGpu = util.pop("perGpu")
    perCatDelay = cpuGpuUtil.pop("perCatDelay")
    counterStat = cpuGpuUtil.pop("counterStat", {})
    summary = _get_summary(deviceProperties, perCatDelay, perGpu)
    hwMetrics, hwFindings = _summarize_hw_metrics(counterStat)
    summary["hardware_metrics"] = hwMetrics
    summary["hardware_findings"] = hwFindings
    stepInfo = cpuGpuUtil.pop("stepInfo")
    file_size_mb = os.path.getsize(filepath) / (1024 * 1024)
    gpu_id = list(perGpu.keys())[0] if perGpu else "?"

    py_tree, py_hot = aggregate_python_stacks(filepath)

    return {
        "traceUrl": convert_to_resource_path(filepath),
        "traceFileSize": file_size_mb,
        "overview": {"detail": util, "summary": summary},
        "stepInfo": stepInfo if stepInfo else [],
        "scenario": "",
        "device_info_for_snap": summary["device_info"],
        "detected_gpu_id": str(gpu_id),
        "raw_exec_delay": perCatDelay,
        "raw_per_gpu": perGpu,
        "raw_counter_stat": counterStat,
        "raw_device_props": deviceProperties,
        "stackInfo": py_tree or {},
        "pythonHotFrames": py_hot,
    }
def process_snapshot_file(pid, filepath):
    retData = {}
    try:
        class CustomUnpickler(pickle.Unpickler):
            def find_class(self, module, name):
                if name in ['_Snapshot', '_Segment', '_Block', '_Frame', '_TraceEntry']:
                    return getattr(sys.modules['gemm'], name)
                return super().find_class(module, name)
        
        # Supports both .pickle.gz and .pickle formats
        if filepath.endswith('.gz'):
            with gzip.open(filepath, 'rb') as f:
                data = CustomUnpickler(f).load()
        else:
            with open(filepath, 'rb') as f:
                data = CustomUnpickler(f).load()
            
        analyzer = MemoryAnalyzer(data)
        analyzer.parseSnap()
        retData["mmSnapUrl"] = convert_to_resource_path(filepath)
        retData["memspikes"] = analyzer.analyze_memory_spikes()
        retData["memOom"] = analyzer.analyze_oom_risk()
        retData["summary"] = analyzer.analyze_memory_usage()
    except Exception as e:
        logger.error(f"Failed to parse snapshot {filepath}: {e}")
    return retData
class AiEvent:
    def __init__(self, folder_path):
        self.folder_path = folder_path
        self.accumulate_result = {}
        self.device_info_map = {} 
        self.pid_gpu_mapping = {} 
    
    def extract_pid(self, filename):
        base = os.path.basename(filename)
        return base.split('_')[-1].split('.')[0]
    def format_key(self, pid):
        gpu_id = self.pid_gpu_mapping.get(pid, "?")
        if gpu_id == "?" and self.pid_gpu_mapping:
            gpu_id = list(self.pid_gpu_mapping.values())[0]
        return f"{pid}:[GPU{gpu_id}] Offline Profiling Task"
    def run(self):
        logger.info(f"Starting to analyze event directory: {self.folder_path}")

        # Gather all JSON and pickle files
        all_json_files = glob.glob(os.path.join(self.folder_path, "*.json"))
        all_pickle_files = glob.glob(os.path.join(self.folder_path, "*.pickle"))
        # Also support .pickle.gz
        all_pickle_files.extend(glob.glob(os.path.join(self.folder_path, "*.pickle.gz")))

        # Check whether aggregation-kernel.json exists
        aggregation_file = None
        for f in all_json_files:
            if f.endswith('aggregation-kernel.json'):
                aggregation_file = f
                break

        # Filter out files that don't need analysis (aggregation-kernel.json is handled separately)
        json_files = [f for f in all_json_files
                      if not (f.endswith('aggregation-kernel.json') or f.endswith('OFFSET.json'))]
        pickle_files = all_pickle_files

        logger.info(f"Found {len(json_files)} JSON files and {len(pickle_files)} pickle files to analyze")

        # If an aggregation-kernel.json exists, set aggregationUrl
        if aggregation_file:
            self.accumulate_result["aggregationUrl"] = convert_to_resource_path(aggregation_file)
            logger.info(f"Aggregation file located: {aggregation_file}")

        all_tasks = []
        for f in json_files:
            original_pid = self.extract_pid(f)
            future = ProcessPool.submit(process_profiling_file, original_pid, f)
            # Store as tuple for later dispatch
            all_tasks.append((original_pid, future, 'profiling'))
            
        for f in pickle_files:
            original_pid = self.extract_pid(f)
            future = ProcessPool.submit(process_snapshot_file, original_pid, f)
            all_tasks.append((original_pid, future, 'snapshot'))

        temp_results = {}
        
        for original_pid, future, task_type in all_tasks:
            try:
                res = future.result(timeout=1200)
                if not res: continue
                
                if task_type == 'profiling':
                    # 1. Update GPU mapping first (used to build the key)
                    self.pid_gpu_mapping[original_pid] = res.get("detected_gpu_id", "?")

                    # 2. Build the final key (e.g. "600201:[GPU0] ...")
                    final_key = self.format_key(original_pid)

                    # 3. Decide whether to merge or insert
                    if final_key not in temp_results:
                        # First occurrence of this key — assign directly
                        temp_results[final_key] = res
                    else:
                        # Key already exists — merge (string concat, numeric accumulate)
                        temp_results[final_key] = self._merge_results(temp_results[final_key], res)

                    # Update the device-info map (used when correlating pickles)
                    if original_pid not in self.device_info_map:
                        self.device_info_map[original_pid] = res.get("device_info_for_snap", [])

                elif task_type == 'snapshot':
                    # Snapshot handling
                    # Note: the snapshot also needs to find its corresponding key.
                    # If the snapshot's pid has no profiling result yet, create an empty placeholder.
                    snap_key = self.format_key(original_pid)
                    if snap_key not in temp_results:
                        temp_results[snap_key] = {
                            "traceUrl": "", "traceFileSize": 0, "overview": {},
                            "stackInfo": {}, "stepInfo": [], "scenario": "",
                            "mmSnapUrl": "", "memOom": {}, "memspikes": [], "memSummary": "暂无任何信息"
                        }

                    temp_results[snap_key]["mmSnapUrl"] = res.get("mmSnapUrl", "")
                    temp_results[snap_key]["memOom"] = res.get("memOom", {})
                    temp_results[snap_key]["memspikes"] = res.get("memspikes", [])
                    
                    raw_summary = res.get("summary", {})
                    device_info = self.device_info_map.get(original_pid, [])
                    temp_results[snap_key]["memSummary"] = fill_memSnap_summary(raw_summary, device_info)

            except concurrent.futures.TimeoutError:
                logger.error(f"Task timeout (type: {task_type})")
            except Exception as e:
                logger.error(f"Task failed (type: {task_type}) - {e}\n{traceback.format_exc()}")

        # Move temp_results contents into accumulate_result.
        # temp_results is shaped like { "PID:[GPU]...": {...data...} }.
        for key, data in temp_results.items():
            self.accumulate_result[key] = data

        return self.save_result()
    def save_result(self):
        def safe_json_default(obj):
            if hasattr(obj, 'item'): return obj.item()
            return str(obj)
        try:
            analysis_result_str = json.dumps(self.accumulate_result, ensure_ascii=False, default=safe_json_default)
        except Exception as e:
            logger.error(f"Failed to serialise analysisResult: {e}")
            analysis_result_str = ""
        detail_output_data = {
            "code": "Success",
            "message": "",
            "data": analysis_result_str,
            "request_id": ""
        }
        
        out_file = os.path.join(self.folder_path, "Analysis_Summary.json")
        try:
            with open(out_file, 'w', encoding='utf-8') as f:
                json.dump(detail_output_data, f, ensure_ascii=False, indent=4)
            logger.info(f"Wrote condensed summary: {out_file}")
        except Exception as e:
            logger.error(f"Failed to save JSON {out_file}: {e}")
        actual_pids = ",".join(self.pid_gpu_mapping.keys()) if self.pid_gpu_mapping else "未知"
        
        meta_output_data = {
            "uid": "1808078950770264",
            "created_by": "alibuc:442438",
            "analysisId": os.path.basename(os.path.normpath(self.folder_path)),
            "analysisTime": time.strftime("%Y-%m-%d %H:%M:%S"),
            "status": "分析成功" if self.accumulate_result else "未匹配到有效源文件或解析失败",
            "arguments": {
                "uid": "1808078950770264",
                "pids": actual_pids,
                "region": "cn-wulanchabu",
                "channel": "ecs",
                "timeout": 2000,
                "instance": "offline-task-node",
                "created_by": "alibuc:442438",
                "iteration_mod": "",
                "iteration_func": "",
                "analysis_params": ["adapt", "kernel", "pytorch", "python", "snapshot", "rdma", "dcgm", "nvtx"]
            },
            "analysisResult": "",
            "region": "cn-wulanchabu",
            "failedLog": "" if self.accumulate_result else "本离线目录文件缺失或解析异常",
            "id": int(time.time() * 1000) % 100000 
        }
        
        return meta_output_data
    def _merge_results(self, existing, new_res):
        # 1. traceUrl: string concatenation (semicolon-separated)
        if existing.get("traceUrl") and new_res.get("traceUrl"):
            existing["traceUrl"] = existing["traceUrl"] + ";" + new_res["traceUrl"]

        # 2. traceFileSize: numeric accumulation
        existing["traceFileSize"] = existing.get("traceFileSize", 0) + new_res.get("traceFileSize", 0)

        # 3. stepInfo: list append
        if new_res.get("stepInfo"):
            existing["stepInfo"].extend(new_res["stepInfo"])

        # 4. device_info: list append
        if new_res.get("device_info_for_snap"):
            existing["device_info_for_snap"].extend(new_res.get("device_info_for_snap", []))

        # 5. Merge core data (recompute Summary)
        old_delay = existing.get("raw_exec_delay", {})
        new_delay = new_res.get("raw_exec_delay", {})
        old_gpu = existing.get("raw_per_gpu", {})
        new_gpu = new_res.get("raw_per_gpu", {})
        old_dev_props = existing.get("raw_device_props", [])
        new_dev_props = new_res.get("raw_device_props", [])

        # Merge execDelay (values summed)
        merged_delay = {}
        for k in set(list(old_delay.keys()) + list(new_delay.keys())):
            merged_delay[k] = (old_delay.get(k, 0) + new_delay.get(k, 0))

        # Merge perGpu (values summed; utilisation weighted-averaged)
        merged_gpu = {}
        for gid in set(list(old_gpu.keys()) + list(new_gpu.keys())):
            o = old_gpu.get(gid, {})
            n = new_gpu.get(gid, {})

            total_delay = o.get("total_delay_us", 0) + n.get("total_delay_us", 0)
            run_times = o.get("run_times", 0) + n.get("run_times", 0)

            # Weighted-average SM utilisation
            sm_util = 0
            if run_times > 0:
                sm_util = (o.get("SM_utilization", 0) * o.get("run_times", 0) + 
                           n.get("SM_utilization", 0) * n.get("run_times", 0)) / run_times
            
            merged_gpu[gid] = {
                "SM_utilization": round(sm_util, 3),
                "active_blocks_per_SM": (o.get("active_blocks_per_SM", 0) + n.get("active_blocks_per_SM", 0)) / 2 if (o and n) else (o.get("active_blocks_per_SM") or n.get("active_blocks_per_SM")),
                "active_warps_per_SM": (o.get("active_warps_per_SM", 0) + n.get("active_warps_per_SM", 0)) / 2 if (o and n) else (o.get("active_warps_per_SM") or n.get("active_warps_per_SM")),
                "total_delay_us": total_delay,
                "run_times": run_times
            }

        # Refresh cached raw data on existing
        existing["raw_exec_delay"] = merged_delay
        existing["raw_per_gpu"] = merged_gpu
        existing["raw_device_props"] = old_dev_props + new_dev_props

        # Rebuild Summary
        new_summary = _get_summary(old_dev_props + new_dev_props, merged_delay, merged_gpu)
        merged_counter = _merge_counter_stat(existing.get("raw_counter_stat", {}),
                                             new_res.get("raw_counter_stat", {}))
        existing["raw_counter_stat"] = merged_counter
        hwMetrics, hwFindings = _summarize_hw_metrics(merged_counter)
        new_summary["hardware_metrics"] = hwMetrics
        new_summary["hardware_findings"] = hwFindings
        existing["overview"]["summary"] = new_summary

        # Refresh perGpu inside detail
        if existing["overview"].get("detail"):
             existing["overview"]["detail"]["perGpu"] = merged_gpu
        
        return existing

class EventQueue:
    def __init__(self):
        self.queue = queue.Queue()
        self.all_records_memory = []
        self.lock = threading.Lock()
    
    def add_event(self, folder_path):
        self.queue.put(AiEvent(folder_path))
    def worker(self):
        while True:
            try:
                event = self.queue.get(block=False)
                meta_info = event.run()
                
                if meta_info:
                    with self.lock:
                        self.all_records_memory.append(meta_info)
                        
                self.queue.task_done()
            except queue.Empty:
                break
            except Exception as e:
                logger.error(f"Worker exited with exception: {e}")
    def start_workers(self, num_workers=2):
        logger.info(f"Starting {num_workers} directory consumer threads...")
        threads = []
        for _ in range(num_workers):
            t = threading.Thread(target=self.worker)
            t.start()
            threads.append(t)
            
        for t in threads:
            t.join()
if __name__ == "__main__":
    multiprocessing.freeze_support()
    
    # Parse command-line arguments
    parser = argparse.ArgumentParser(description='AI Profiling Data Analysis Tool')
    parser.add_argument('-d', '--directory', type=str, required=True,
                        help='Input data directory path; files in this directory are analysed directly')
    args = parser.parse_args()

    ProcessPool = ProcessPoolExecutor(max_workers=cpu_count)
    DATA_DIR = os.path.abspath(args.directory)

    if not os.path.exists(DATA_DIR):
        logger.error(f"Data directory not found '{DATA_DIR}'. Please provide a valid directory path.")
        sys.exit(1)

    if not os.path.isdir(DATA_DIR):
        logger.error(f"'{DATA_DIR}' is not a directory.")
        sys.exit(1)

    # Analyse the given directory directly, without recursing into subdirectories
    logger.info(f"Starting analysis of directory: {DATA_DIR}")

    start_t = time.time()
    event = AiEvent(DATA_DIR)
    meta_info = event.run()

    ProcessPool.shutdown(wait=True)

    if meta_info:
        logger.info(f"Status: {meta_info.get('status', 'unknown')}")
    else:
        logger.warning("Analysis produced no valid results")

    logger.info(f"Total elapsed: {round(time.time() - start_t, 2)} seconds.")