import os.path

from pyki.perfdata.common import PerfdataHeader, MAGIC, BufferPrologueV2, EntryHeader, find_type_by_code, \
    get_perfdata_path
from pyki.process_util import get_nspid


def read_perfdata_from_pid(pid: int):
    nspid = get_nspid(pid)
    if nspid == pid:
        filepath = get_perfdata_path(pid)
    else:
        # in container
        filepath = f"/proc/{pid}/root/{get_perfdata_path(nspid)}"
    return read_perfdata(filepath)


def read_perfdata(filepath):
    if not os.path.exists(filepath):
        raise FileNotFoundError(f"Perfdata file at {filepath} not found")
    with open(filepath, 'rb') as f:
        buffer = f.read()

    header = PerfdataHeader.from_bytes(buffer[:PerfdataHeader.SIZE])
    if header.magic != MAGIC:
        raise ValueError(f"Illegal magic {header.magic}")
    if not (header.major == 2 and header.minor == 0):
        raise ValueError(f"Unsupported version {header.major}.{header.minor}")

    endian = '>' if header.byte_order == 0 else '<'
    byteorder = 'big' if header.byte_order == 0 else 'little'

    prologue = BufferPrologueV2.from_bytes(buffer[PerfdataHeader.SIZE:PerfdataHeader.SIZE + BufferPrologueV2.SIZE],
                                           endian)
    if prologue.accessible == 0:
        raise ValueError(f"Not accessible {prologue.accessible}")

    result = {}
    start_offset = prologue.entry_offset

    for _ in range(prologue.num_entries):
        entry = EntryHeader.from_bytes(buffer[start_offset:start_offset + 20], endian)
        name_start = start_offset + entry.name_offset
        name_end = buffer.find(b'\x00', name_start)
        if name_end < 0:
            raise ValueError("Invalid binary")
        name = buffer[name_start:name_end].decode('utf-8')
        data_type = find_type_by_code(entry.data_type.to_bytes(1, byteorder=byteorder))
        data_start = start_offset + entry.data_offset
        data = data_type.from_bytes(buffer[data_start:start_offset + entry.entry_length],
                                    byteorder,
                                    1 if entry.vector_length == 0 else entry.vector_length)
        result[name] = data

        start_offset += entry.entry_length

    return result
