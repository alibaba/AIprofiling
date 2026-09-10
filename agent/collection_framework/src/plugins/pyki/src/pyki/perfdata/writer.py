from pyki.perfdata.common import (PerfdataHeader, BufferPrologueV2, EntryHeader, MAGIC,
                                  get_perfdata_path, DataType, vVariable, uNone, str_type, byteorder_to_struct_endian)
import logging
import os
import mmap
import sys
import time
import atexit
import threading
import traceback
from typing import Dict

_logger = logging.getLogger(__name__)
_PERFDATA_MEMORY_SIZE = 32 * 1024  # 32kB
DEFAULT_STR_LENGTH = 256


class Metric:
    def __init__(self, writer, data_type: DataType, name: str, data_var=vVariable, data_unit=uNone,
                 length=1):
        self.writer = writer
        self.data_type = data_type
        self.name = name
        self.length = length
        name_bytes = name.encode('utf-8') + b'\x00'
        name_length = len(name_bytes)
        data_length = self._get_data_length()
        self.entry_header = EntryHeader(entry_length=EntryHeader.SIZE + name_length + data_length,
                                        name_offset=EntryHeader.SIZE,
                                        vector_length=0 if length == 1 else length,
                                        data_type=int.from_bytes(data_type.perfdata_code, writer.byteorder),
                                        flags=1,  # this field is no use
                                        data_units=data_unit,
                                        data_var=data_var,
                                        data_offset=EntryHeader.SIZE + name_length)
        # will be initialized when registering
        self.global_entry_offset = 0

    def _get_data_length(self):
        return self.data_type.length * self.length

    # caller should ensure thread safety
    def update_value(self, new_value, index=0):
        if not self.writer.enabled:
            return
        if isinstance(new_value, list) or self.data_type == str_type:
            assert self.data_type == str_type or (self.length != 1 and len(new_value) == self.length)
            data = self.data_type.to_bytes(new_value, self.writer.byteorder, length=self.length)
            offset = self.global_entry_offset + self.entry_header.data_offset
        else:
            assert index < self.length
            data = self.data_type.to_bytes(new_value, self.writer.byteorder)
            offset = self.global_entry_offset + self.entry_header.data_offset + index * self.data_type.length
        with self.writer.lock:
            self.writer.update_bytes(data, offset)
            self.writer.mark_updated()

    def to_bytearray(self, data, byteorder):
        buffer = bytearray(self.entry_header.entry_length)
        buffer[0:EntryHeader.SIZE] = self.entry_header.to_bytes(byteorder_to_struct_endian(byteorder))
        name_bytes = self.name.encode('utf-8') + b'\x00'
        name_length = len(name_bytes)
        data_length = self._get_data_length()
        buffer[self.entry_header.name_offset:self.entry_header.name_offset + name_length] = name_bytes
        buffer[self.entry_header.data_offset:self.entry_header.data_offset + data_length] = (
            self.data_type.to_bytes(data, byteorder, self.length))
        return buffer


class _PerfdataWriter:
    def __init__(self):
        # self.enabled = os.environ.get("PYKI_PERFDATA_DISABLE", None) is None
        self.enabled = False
        self.byteorder = sys.byteorder
        byteorder_byte = 1 if sys.byteorder == 'little' else 0
        self._initial_time = time.perf_counter_ns()
        self._header: PerfdataHeader = PerfdataHeader(MAGIC, byteorder_byte, 2, 0)
        self._prologue: BufferPrologueV2 = BufferPrologueV2(1, 32, 0, 0, 32, 0)
        self._metrics: Dict[str, Metric] = {}
        self._file = None
        self._file_length: int = 0
        self._mm = None
        self.lock: threading.Lock = threading.Lock()
        self.shutdown = False

        self.create_mmap_file()

    def create_mmap_file(self):
        try:
            if not self.enabled:
                return
            with self.lock:
                path = get_perfdata_path(os.getpid())
                if os.path.exists(path):
                    os.remove(path)
                self._file = open(path, "w+b")
                self._file.truncate(_PERFDATA_MEMORY_SIZE)
                self._file_length = _PERFDATA_MEMORY_SIZE
                self._mm = mmap.mmap(fileno=self._file.fileno(),
                                     length=_PERFDATA_MEMORY_SIZE,
                                     flags=mmap.MAP_SHARED,
                                     access=mmap.PROT_WRITE)
                self.update_bytes(self._header.to_bytes(), 0)
                self.update_bytes(self._prologue.to_bytes(byteorder_to_struct_endian(self.byteorder)),
                                  self._header.SIZE)
                self.mark_updated()
        except Exception as e:
            _logger.error(f"Failed to create perfdata file: {traceback.format_exc()}")
            self.enabled = False
            self.clean_up()

    def mark_updated(self):
        assert self.enabled and self.lock.locked()
        elapsed = time.perf_counter_ns() - self._initial_time
        self._prologue.mod_timestamp = elapsed
        self.update_bytes(int.to_bytes(elapsed, 8, sys.byteorder), BufferPrologueV2.MOD_TIMESTAMP_OFFSET)

    def _guarantee_file_capacity(self):
        assert self.enabled and self.lock.locked()
        if self._prologue.used > self._file_length:
            new_length = self._prologue.used + _PERFDATA_MEMORY_SIZE
            assert self._file is not None
            assert self._mm is not None
            self._file.truncate(new_length)
            self._file_length = new_length
            self._mm.resize(new_length)

    def register_metric(self, data_type: DataType, name: str, data_var=vVariable, data_unit=uNone, initial_value=None,
                        length: int = 1) -> Metric:
        assert initial_value is None or length == 1 or len(initial_value) == length
        if length == 1 and data_type == str_type:
            length = DEFAULT_STR_LENGTH

        with self.lock:
            if name in self._metrics:
                raise ValueError(f"Metric {name} already registered")

            metric = Metric(self, data_type, name, data_var, data_unit, length)
            self._metrics[name] = metric
            if not self.enabled:
                return metric

            # update prolog
            metric.global_entry_offset = self._prologue.used
            self._prologue.used += metric.entry_header.entry_length
            self._guarantee_file_capacity()
            self._prologue.num_entries += 1
            self.update_bytes(self._prologue.to_bytes(byteorder_to_struct_endian(self.byteorder)),
                              self._header.SIZE)

            if initial_value is None:
                if length == 1:
                    initial_value = data_type.default_value
                else:
                    initial_value = [data_type.default_value] * length
            # write entry
            metric_byte_array = metric.to_bytearray(initial_value, self.byteorder)
            self.update_bytes(metric_byte_array, metric.global_entry_offset)

            self.mark_updated()
            return metric

    def update_bytes(self, data: bytes, offset: int):
        assert self.enabled and self.lock.locked()
        assert self._mm is not None
        self._mm.seek(offset)
        self._mm.write(data)

    def clean_up(self):
        with self.lock:
            if self._file is not None and not self._file.closed:
                self._file.close()
                self._file = None
            if self._mm is not None and not self._mm.closed:
                self._mm.close()
                self._mm = None
            path = get_perfdata_path(os.getpid())
            if os.path.exists(path):
                os.remove(path)
            self.enabled = False


def register_metric(data_type: DataType, name: str, data_var=vVariable, data_unit=uNone, initial_value=None,
                    length: int = 1) -> Metric:
    return perfdata_writer.register_metric(data_type, name, data_var, data_unit, initial_value, length)


perfdata_writer = _PerfdataWriter()
atexit.register(perfdata_writer.clean_up)
