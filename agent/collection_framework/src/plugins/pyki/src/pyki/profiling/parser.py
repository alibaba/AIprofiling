from datetime import datetime, timedelta
from os import SEEK_SET
from pyki.error import raise_error
from typing import Any


def _format_millisecond(ms):
    return datetime.fromtimestamp(ms / 1000).strftime('%Y-%m-%d %H:%M:%S.%f')[:-3]


def _to_unsigned_int(bytes):
    return int.from_bytes(bytes, byteorder='little', signed=False)


def _read_var_int(fd):
    d = fd.read(1)[0]
    r = d & 0x7F
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 7)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 14)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 21)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 28)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 35)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 42)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0x7f) << 49)
    if not d & 0x80:
        return r

    d = fd.read(1)[0]
    r += ((d & 0xff) << 56)
    return r


class _Header:
    def parse(self, f):
        expected_magic = b'PYKI-PROFILE'
        expected_versions = [1]

        magic = f.read(12)
        if magic != expected_magic:
            raise_error(f"Invalid magic: {magic}")

        version = _to_unsigned_int(f.read(4))
        if not version in expected_versions:
            raise_error(f"Invalid version: {version}")

        size = _to_unsigned_int(f.read(8))
        start_time = _to_unsigned_int(f.read(8))
        end_time = _to_unsigned_int(f.read(8))
        constant_pool_offset = _to_unsigned_int(f.read(8))

        self.magic = magic
        self.version = version
        self.size = size
        self.start_time = start_time
        self.end_time = end_time
        self.constant_pool_offset = constant_pool_offset

    def __repr__(self) -> str:
        s = "Profile Header:\n"
        s += f"  {'Magic:': <21} {self.magic}\n"
        s += f"  {'Version:': <21} {self.version}\n"
        s += f"  {'Size:': <21} {self.size}\n"
        s += f"  {'Start Time:': <21} {_format_millisecond(self.start_time)}\n"
        s += f"  {'End Time:': <21} {_format_millisecond(self.end_time)}\n"
        s += f"  {'Constant Pool Offset:': <21} {self.constant_pool_offset}\n"
        return s


class _Frame:
    def __init__(self, constant_pools, filename, name, lineno):
        self.constant_pools = constant_pools
        self.filename = filename
        self.name = name
        self.lineno = lineno

    def __repr__(self) -> str:
        filename = self.constant_pools.get_string(self.filename)
        name = self.constant_pools.get_string(self.name)
        return f"{name} ({filename}:{self.lineno})"


class _StackTrace:
    def __init__(self, frames):
        self.frames = frames

    def depth(self):
        return len(self.frames)

    def __repr__(self) -> str:
        s = ""
        for i, frame in enumerate(self.frames):
            s += "    " + str(frame) + "\n"
        return s


class _ConstantPools:
    def __init__(self):
        self.strings = {}
        self.stack_traces = {}

    def get_string(self, id):
        return self.strings[id]

    def get_stack_traces(self, id):
        return self.stack_traces[id]

    def parse(self, f):
        while True:
            b = f.read(1)
            if not b:
                break
            type = b[0]
            length = _read_var_int(f)
            index = 0
            if type == 1:
                for i in range(length):
                    id = _read_var_int(f)
                    kind = f.read(1)[0]
                    length = _read_var_int(f)
                    content = f.read(length).decode('utf-8')
                    self.strings[id] = content
                    index = index + 1
            elif type == 2:
                for i in range(length):
                    id = _read_var_int(f)
                    nframes = _read_var_int(f)
                    frames = []
                    for i in range(nframes):
                        filename = _read_var_int(f)
                        name = _read_var_int(f)
                        lineno = _read_var_int(f)
                        frames.append(_Frame(self, filename, name, lineno))
                    stack_traces = _StackTrace(frames)
                    self.stack_traces[id] = stack_traces
                    index = index + 1
            else:
                raise_error(f"Invalid constant pool type: {type}")


class CPUEvent:
    ID = 32

    def __init__(self, constant_pools, thread_id, stack_traces_id):
        self.constant_pools = constant_pools
        self.thread_id = thread_id
        self.stack_traces_id = stack_traces_id

    def __repr__(self) -> str:
        s = "CPU Event:\n"
        s += f"  {'Thread ID:': <10} {self.thread_id}\n"
        s += f"  {'Stack Trace:': <10}\n"
        s += str(self.constant_pools.get_stack_traces(self.stack_traces_id))
        return s


class AllocationEvent:
    ID = 33

    def __init__(self, constant_pools, thread_id, stack_traces_id, size):
        self.constant_pools = constant_pools
        self.thread_id = thread_id
        self.stack_traces_id = stack_traces_id
        self.size = size

    def __repr__(self) -> str:
        s = "Allocation Event:\n"
        s += f"  {'Thread ID:': <10} {self.thread_id}\n"
        s += f"  {'Size:': <10} {self.size}\n"
        s += f"  {'Stack Trace:': <10}\n"
        s += str(self.constant_pools.get_stack_traces(self.stack_traces_id))
        return s


class Parser:
    def __init__(self, data_path):
        self._data_path = data_path
        self._header = _Header()
        self._events = []
        self._allocation_events = []
        self._cpu_events = []

    def parse(self):
        with open(self._data_path, 'rb') as f:
            # Parse header
            self._header.parse(f)

            # Parse constant pools
            f.seek(self._header.constant_pool_offset, SEEK_SET)
            cps = _ConstantPools()
            cps.parse(f)

            # Parse events
            f.seek(48, SEEK_SET)
            while True:
                pos = f.tell()
                if pos == self._header.constant_pool_offset:
                    break
                type = f.read(1)[0]
                if type < 32:
                    # reach constant pool
                    return
                event: Any = None

                if type == CPUEvent.ID:
                    thread_id = _read_var_int(f)
                    stack_traces_id = _read_var_int(f)
                    event = CPUEvent(cps, thread_id, stack_traces_id)
                    self._cpu_events.append(event)
                    self._events.append(event)
                elif type == AllocationEvent.ID:
                    thread_id = _read_var_int(f)
                    stack_traces_id = _read_var_int(f)
                    size = _read_var_int(f)
                    event = AllocationEvent(
                        cps, thread_id, stack_traces_id, size)
                    self._allocation_events.append(event)
                    self._events.append(event)
                else:
                    raise_error(f"Invalid event type: {type}")

    def print_header(self):
        print(self._header)

    def print_events(self):
        for event in self._events:
            print(event)

    def print_summary(self):
        print(f"Total Events: {len(self._events)}")
        print(f"  Allocation Events: {len(self._allocation_events)}")
        print(f"  CPU Events: {len(self._cpu_events)}")


def get_start_end_time(data_path):
    with open(data_path, 'rb') as f:
        header = _Header()
        header.parse(f)
        return (header.start_time, header.end_time)
