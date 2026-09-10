import pickle
from collections import defaultdict
import pandas as pd
import json
from datetime import datetime
from typing import List
resOom = {'summary':'No oom events','oomItem': []}
def trans_frames_to_str(frames):
    stack = ""
    for frame in reversed(frames[-10:]):
        line = frame["name"]+":"+frame["filename"]+":"+str(frame["line"])
        stack = stack+" | "+line
    if len(stack) < 1:
        return "no frames record ... Potential causes may be: This block was allocated before _record_memory_history was enabled."
    return stack
def format_date_time(timestamp_us):
    timestamp_sec = timestamp_us / 1_000_000
    #dt = datetime.fromtimestamp(timestamp_sec, tz=timezone.utc)
    dt = datetime.fromtimestamp(timestamp_sec)
    formatted_time = dt.strftime("%Y:%m:%dT%H:%M:%S.%f")
    return formatted_time
    
class _Frame():
    def __init__(self, filename: str, line: int, name: str):
        self.filename = filename
        self.line = line
        self.name = name
class _Block():
    def __init__(self, size: int, requested_size: int, address: int, state: str, frames: List["_Frame"]):
        self.size = size
        self.requested_size = requested_size
        self.address = address
        self.state = state
        self.frames = frames
class _Segment():
    def __init__(self, device: int,address: int, total_size: int, stream: int, segment_type: str, allocated_size: int, active_size: int, blocks: List["_Block"]):
        self.device = device,
        self.address = address
        self.total_size = total_size
        self.stream = stream
        self.segment_type = segment_type
        self.allocated_size = allocated_size
        self.active_size = active_size
        self.blocks = blocks
class _TraceEntry():
    def __init__(self, action: str, addr: int, frames: List["_Frame"], size: int, stream: int, time_us: int, device_free: int = 0):
        self.action = action
        self.addr = addr
        self.frames = frames
        self.size = size
        self.stream = stream
        self.time_us = time_us
        self.device_free = device_free
class _Snapshot():
    #def __init__(self, segments: list[_Segment], device_traces: list[list[_TraceEntry]]):
    segments: List["_Segment"]
    device_traces: List[List["_TraceEntry"]]
class MemoryAnalyzer:
    def __init__(self, snapshot_data: _Snapshot):
        self.snapshot = snapshot_data
        self.memory_usage_over_time = []
        self.seg_usage_over_time = []
        self.allocation_events = []
        self.deallocation_events = []
        self.seg_allocation_events = []
        self.oom_events = []
        self.allocated_memory_map = {} # address -> (size, time_us, requested_size, frames)
        self.allocated_seg_map = {} # address -> (size, time_us)
        self.snap_seg_map = {}      # address -> (size, time_us, end_time, frames)
        self.seg_size_map = {}           # size -> coount
        self.allocated_block_map = {} # address -> (size, time_us)
        self.snap_block_map = {}      # address -> (size, time_us)
        self.processed_traces = False
        self.start_time = 0
        self.end_time = 0
        self.total_seg = {}
    def add_snap_size_to_seg_over_time(self,snap_seg):
        if len(self.seg_usage_over_time) > 0:
            firstItem = self.seg_usage_over_time[0]
            if firstItem["time_us"] > self.start_time:
                first = {"time_us":self.start_time,"usage":0}
                self.seg_usage_over_time.insert(0,first)
        else:
            firstItem = {"time_us":self.start_time,"usage":0}
            lastItem = {"time_us":self.end_time,"usage":0}
            self.seg_usage_over_time.append(firstItem)
            self.seg_usage_over_time.append(lastItem)
        for usage in self.seg_usage_over_time:
            if usage["time_us"] >= snap_seg[1]:
                usage["usage"] += snap_seg[0]
    
    def seg_alloc_deal(self,addr,size,tm,frames):
        if addr not in self.allocated_seg_map:
            self.allocated_seg_map[addr] = (size,tm,frames)
        elif size != self.allocated_seg_map[addr][0]:
            # reallocating the same address (size differs)
            self.allocated_seg_map[addr] = (size,tm,frames)
        else:
            # reallocating the same address (size equal)
            self.allocated_seg_map[addr] = (size,tm,frames)
        if addr in self.snap_seg_map:
            seg = self.snap_seg_map[addr]
            self.snap_seg_map[addr] = (seg[0], tm,0,frames)
    def parseSnap(self):
        self.total_seg["total_size"] = 0
        self.total_seg["active_size"] = 0
        self.total_seg["allocated_size"] = 0
        self.total_seg["total_seg"] = 0
        self.total_seg["small_pool"] = 0
        self.total_seg["large_pool"] = 0
        self.total_seg["other_pool"] = 0
        self.total_seg["smallSize"] = 0
        self.total_seg["largeSize"] = 0
        self.total_seg["otherSize"] = 0
        self.total_seg["active_awaiting_free"] = 0
        self.total_seg["nr_blocks"] = 0
 
        for seg in self.snapshot['segments']:
            #print(f"total_size={seg["total_size"]} allocated_size={seg["allocated_size"]} active_size={seg["active_size"]} dev={seg["device"]}")
            active_allocated = active_awaiting_free = inactive = other = 0
            #active_awaiting_free = 0
            nr_blocks = 0
            for block in seg["blocks"]:
                if block["state"] == "active_allocated":
                    active_allocated = block["size"] + active_allocated
                elif block["state"] == "active_awaiting_free":
                    active_awaiting_free = active_awaiting_free + block["size"]
                elif block["state"] == "inactive":
                    inactive = inactive + block["size"]
                else :
                    other = other + block["size"]
                nr_blocks += 1
            #print(f"active_allocated={active_allocated} active_awaiting_free={active_awaiting_free} inactive={inactive} other={other}")
            self.snap_seg_map[seg['address']] = (seg["total_size"],self.start_time,0,"")
            self.total_seg["total_size"] += seg["total_size"]
            self.total_seg["active_size"] += seg["active_size"]
            self.total_seg["allocated_size"] += seg["allocated_size"]
            self.total_seg["total_seg"] += 1
            if seg['segment_type'] == "small":
                self.total_seg["small_pool"] += 1
                self.total_seg["smallSize"] += seg["total_size"]
            elif seg['segment_type'] == "large":
                self.total_seg["large_pool"] += 1
                self.total_seg["largeSize"] += seg["total_size"]
            else:
                self.total_seg["other_pool"] += 1
                self.total_seg["otherSize"] += seg["total_size"]
            self.total_seg["active_awaiting_free"] = active_awaiting_free
            self.total_seg["nr_blocks"] += nr_blocks
        curr_allocte_size = 0
        nr_trace = 0
        dvid = 0
        for device_trace_list in self.snapshot["device_traces"]:
            if len(device_trace_list) < 1:
                dvid += 1
                continue
            dvid += 1
            curr_allocte_size = 0
            first_ent = device_trace_list[0]
            start_tm = first_ent["time_us"]
            last_ent = device_trace_list[-1]
            end_tm = last_ent["time_us"]
            if self.end_time < end_tm:
                self.end_time = end_tm
            if start_tm > end_tm:
                print(f"start_tm={start_tm} end_tm={end_tm} , entry over follow and not ordered!!")
            for trace_entry in device_trace_list:
                if self.start_time == 0:
                    #:what about traceEntry overflow?? that'ok, increase order by time_us in device_trace_list
                    self.start_time = trace_entry['time_us']
                    for seg in self.snapshot['segments']:
                        self.snap_seg_map[seg['address']] = (seg["total_size"],self.start_time,0,"")
                        self.seg_allocation_events.append({
                            "time_us": self.start_time,
                            "size": seg["total_size"],
                            "addr": seg['address'],
                            "frames": trans_frames_to_str(seg["frames"]),
                            "requested_size": seg["total_size"] # Placeholder
                        })
                is_seg_trace = 0
                if trace_entry["action"] in ("segment_alloc","segment_map"):
                    is_seg_trace = 1
                    curr_allocte_size += trace_entry["size"]
                    self.seg_alloc_deal(trace_entry['addr'],trace_entry["size"],trace_entry["time_us"],trace_entry['frames'])
                    self.seg_allocation_events.append({
                        "time_us": trace_entry["time_us"],
                        "size": trace_entry["size"],
                        "addr": trace_entry["addr"],
                        "frames": trans_frames_to_str(trace_entry["frames"]),
                        "requested_size": trace_entry["size"] # Placeholder
                    })
                elif trace_entry["action"] in ("segment_free","segment_unmap"):
                    is_seg_trace = 1
                    curr_allocte_size -= trace_entry["size"]
                    if trace_entry['addr'] not in self.allocated_seg_map:
                        if trace_entry['addr'] in self.snap_seg_map:
                            #self.add_snap_size_to_seg_over_time(self.snap_seg_map[trace_entry['addr']])
                            print(f"WARN: Free a segment {hex(trace_entry['addr'])} in snapshot")
                        else:
                            self.add_snap_size_to_seg_over_time((trace_entry["size"],self.start_time))
                            print(f"WARN: Free a segment {hex(trace_entry['addr'])} never recorded!!! May be Trace Entry overflow")
                    else:
                        del self.allocated_seg_map[trace_entry['addr']]
                elif trace_entry["action"] == "oom":
                    self.oom_events.append(trace_entry)
                if is_seg_trace == 1:
                    self.seg_usage_over_time.append({"time_us": trace_entry["time_us"], "usage": curr_allocte_size})
                nr_trace += 1
        print(f"nr_trace={nr_trace}")
        # Update seg_usage_over_time for segments still present in the snapshot
        for addr in self.snap_seg_map:
            snap = self.snap_seg_map[addr]
            self.snap_seg_map[addr] = (snap[0],snap[1],self.end_time,snap[3])
            snap = self.snap_seg_map[addr]
            #print(f"start_time={self.start_time}")
            #print(f"snap:segments:addr={addr},start_time={snap[1]},end_time={snap[2]}, size={snap[0]}")
            if snap[1] == self.start_time:
                self.add_snap_size_to_seg_over_time(snap)
        # Debug: segments not captured in the trace may have been allocated before tracing started
        # for addr in  self.snap_seg_map:
        #     if addr in self.allocated_seg_map:
        #         size,tm = self.allocated_seg_map[addr]
        #         print(f"addr={hex(addr)} was alloccted at {tm}")
        #     else:
        #         print(f"addr={hex(addr)} was alloccted before trace")
        # Debug: system-wide segment memory usage
        # for x in self.seg_usage_over_time:
        #     tm = x["time_us"]
        #     kb = x["usage"]/1024
        #     print(f"{tm}: {kb} KB")
    def get_seg_snaps(self,endtime):
        oomItems = []
        oomItem = {}
        i = 0
        #for seg in self.snapshot['segments']:
        for addr in self.snap_seg_map:
            seg = self.snap_seg_map[addr]
            oomItem["size"] = seg[0]
            oomItem["start_time"] = seg[1]
            oomItem["end_time"] = endtime
            oomItem["frames"] = "decorate_context:torch/utils/_contextlib.py:116" #trans_frames_to_str(seg[3])
            oomItems.append(oomItem)
            i += 1
            if i > 20:
                break
        return oomItems
    def moc_oom_datas(self):
        oomItems = [{
            "start_time": 0,
            "end_time": 0,
            "size": 0,
            "frames": "",
        }]
        return oomItems
    def analyze_oom_risk(self):
        #self._process_traces()
        ooms = []
        oom = {}
        res = {}
        if self.oom_events:
            for oom_event in self.oom_events:
                oom["time"] = oom_event['time_us']
                oom["free"] = oom_event['device_free']/1024**2
                oom["request"] = oom_event["size"]/1024**2
                oom["requested"] = self.total_seg["total_size"]/1024**2
                oom["alloced"] = self.total_seg["allocated_size"]/1024**2
                oom["acvite"] =  self.total_seg["active_size"]/1024**2
                oom["nrSeg"] = self.total_seg["total_seg"]
                oom["nrSmall"] = self.total_seg["small_pool"]
                oom["nrLarge"] = self.total_seg["large_pool"]
                oom["nrOther"] = self.total_seg["other_pool"]
                oom["smallSize"] = self.total_seg["smallSize"]/1024**2
                oom["largeSize"] = self.total_seg["largeSize"]/1024**2
                oom["otherSize"] = self.total_seg["otherSize"]/1024**2
                oom["waitingFree"] = self.total_seg["active_awaiting_free"]
                oom["nrBlocks"] = self.total_seg["nr_blocks"]
                summary ="在 %s 申请%dMB时 发生OOM,剩余显存 %dMB\n"%(oom["time"],oom["request"],oom["free"])
                summary = "%storch已经从显卡申请了%dMB显存(池),显存池已经分配出去%dMB,正在使用的有%dMB\n"%(summary,oom["requested"],oom["alloced"],oom["acvite"])
                summary = "%storch中小池%d个共%dMB,大池%d个共%dMB,显存池中目前有%d个对象"%(summary,oom["nrSmall"],oom["smallSize"],oom["nrLarge"],oom["largeSize"],oom["nrBlocks"])
                oom["summary"] = summary
                ooms.append(oom)
                res["summary"] = summary
                res["oomItem"] = self.get_seg_snaps(oom["time"])
                break
        else:
            res["summary"] = "未检测到 OOM 事件"
            res["oomItem"] = self.moc_oom_datas()
            res = resOom
        return res
        #oom["nr_pool"] = oom_event[""]
    def analyze_memory_usage(self):
        def fill_usage(tm,size,util,risk):
            usage = {}
            usage["time"] = tm
            usage["size"] = size
            usage["util"] = util
            usage["risk"] = risk
            return usage
        top5 = sorted(self.seg_usage_over_time, key=lambda x: x["usage"], reverse=True)[:1]
        res  = {}
        usage = {}
        res["top"] = fill_usage("",0,0,"risk")
        for item in top5:
            used = item['usage']
            usage = fill_usage(format_date_time(item["time_us"]),item['usage']/1024**2,0.00,0)
            res["top"] = usage
            break
            #res = "%s %d)time=%s usage=%dMB "%(res,i,tm,usage/1024**2)
        usage = fill_usage(format_date_time(self.end_time),self.total_seg["total_size"]/1024**2,0.00,0)
        res["curr"] = usage
        return res
    def analyze_memory_spikes(self):
        #self._process_traces()
        spikes_json = []
        if not self.seg_allocation_events:
            return spikes_json
        
        allocation_sizes = [event['size'] for event in self.seg_allocation_events]
        if not allocation_sizes:
            return spikes_json
        # Use standard deviation to identify outliers
        # mean_size = pd.Series(allocation_sizes).mean()
        # std_size = pd.Series(allocation_sizes).std()
        # max_size = pd.Series(allocation_sizes).max()
        # min_size = pd.Series(allocation_sizes).min()
        # print(f"mean_size={mean_size}, std_size={std_size}, max_size={max_size}")
        # spike_threshold = mean_size + 1.5 * std_size # arbitrary threshold, tunable
        spikes = []
        for event in sorted(self.seg_allocation_events, key=lambda x: x['size'],reverse=True)[0:5]:
            spikes.append(event)
        if spikes:
            for spike in spikes:
                tmp = {}
                formatted_time = format_date_time(spike["time_us"])
                spike["time_us"] = formatted_time
                tmp["time"] = formatted_time
                tmp["size"] = spike["size"]/1024**2
                tmp["stack"] = spike["frames"]
                tmp["addr"] = hex(spike["addr"])
                spikes_json.append(tmp)
                json_str = json.dumps(tmp, ensure_ascii=False, indent=2)
        return spikes_json
def load_memory_snapshot(filepath):
    try:
        with open(filepath, 'rb') as f:
            snapshot_dict: _Snapshot
            snapshot_dict = pickle.load(f)
            return snapshot_dict
    except Exception as e:
        print(f"Error loading snapshot file {filepath}: {e}")
        return None