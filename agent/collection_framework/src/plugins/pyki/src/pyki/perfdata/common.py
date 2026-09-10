import struct
import os
import tempfile
from abc import ABC

MAGIC = 0xcafec0c0

_perfdata_code_to_type = {}


def find_type_by_code(code):
    if code not in _perfdata_code_to_type:
        raise ValueError(f"Unknown perfdata code: {code}")
    return _perfdata_code_to_type[code]


def byteorder_to_struct_endian(byteorder: object) -> str:
    return '>' if byteorder == 'big' else '<'


def struct_endian_to_byteorder(byteorder: object) -> str:
    return 'big' if byteorder == '>' else 'little'


class DataType(ABC):
    def __init__(self, length: int, perfdata_code: bytes, python_type: type, default_value):
        """
        :param length: length of the single element in bytes
        :param perfdata_code: perfdata code
        :param python_type: python type
        :param default_value: default value
        """
        self.length = length
        self.perfdata_code = perfdata_code
        self.python_type = python_type
        self.default_value = default_value
        _perfdata_code_to_type[perfdata_code] = self

    def from_bytes(self, data: bytes, byteorder, length=1):
        """
        :param data: bytes to decode
        :param byteorder: byteorder, "big" or "little"
        :param length: length of the elements
        """
        if length == 1:
            return self._element_from_bytes(data, byteorder)
        else:
            return [self._element_from_bytes(data[i * self.length:(i + 1) * self.length], byteorder) for i in
                    range(length)]

    def to_bytes(self, data, byteorder, length=1):
        """
        :param data: elements to encode
        :param byteorder: byteorder, "big" or "little"
        :param length: length of the elements
        """
        assert length == 1 or len(data) == length
        if length == 1:
            return self._element_to_bytes(data, byteorder)
        else:
            return b''.join([self._element_to_bytes(data[i], byteorder) for i in range(len(data))])

    def _element_from_bytes(self, data: bytes, byteorder):
        raise NotImplementedError()

    def _element_to_bytes(self, data, byteorder):
        raise NotImplementedError


def get_datatype_from_value(value):
    if isinstance(value, str):
        return str_type
    elif isinstance(value, int):
        return long_type
    elif isinstance(value, float):
        return float_type
    else:
        raise ValueError()


class StrType(DataType):
    def __init__(self):
        super().__init__(1, b"B", bytes, b"")

    def from_bytes(self, data, byteorder, length=1):
        data = data.split(b'\x00')[0]
        return data.decode("utf-8")

    def to_bytes(self, data, byteorder, length=1):
        encoded = data.encode("utf-8")
        if len(encoded) > length - 1:
            encoded = encoded[:length - 1]
        encoded = encoded + b'\x00'
        return encoded


class LongType(DataType):
    def __init__(self):
        super().__init__(8, b"J", int, 0)

    def _element_from_bytes(self, data, byteorder):
        return int.from_bytes(data, byteorder=byteorder, signed=True)

    def _element_to_bytes(self, data, byteorder):
        return data.to_bytes(8, byteorder=byteorder, signed=True)


class FloatType(DataType):
    def __init__(self):
        super().__init__(8, b"D", float, 0.0)

    def _element_from_bytes(self, data, byteorder):
        return struct.unpack(byteorder_to_struct_endian(byteorder) + 'd', data)[0]

    def _element_to_bytes(self, data, byteorder):
        return struct.pack(byteorder_to_struct_endian(byteorder) + 'd', data)


str_type = StrType()
long_type = LongType()
int_type = long_type
float_type = FloatType()

# Constants for variability attribute
vInvalid = 0
vConstant = 1
vMonotonic = 2
vVariable = 3

# Constants for unit of measure attribute
uInvalid = 0
uNone = 1
uBytes = 2
uTicks = 3
uEvents = 4
uString = 5
uHertz = 6


class PerfdataHeader:
    FORMAT = '>IBBB'
    SIZE = struct.calcsize(FORMAT)
    # offset to begin of file
    MAGIC_OFFSET = 0
    BYTE_ORDER_OFFSET = 4
    MAJOR_OFFSET = 5
    MINOR_OFFSET = 6

    def __init__(self, magic, byte_order, major, minor):
        self.magic = magic
        self.byte_order = byte_order
        self.major = major
        self.minor = minor

    @classmethod
    def from_bytes(cls, data):
        return cls(*struct.unpack(cls.FORMAT, data))

    def to_bytes(self):
        return struct.pack(PerfdataHeader.FORMAT, self.magic, self.byte_order, self.major, self.minor)

    def __str__(self):
        return f"PerfdataHeader(magic={self.magic}, byte_order={self.byte_order}, major={self.major}, minor={self.minor})"


class BufferPrologueV2:
    FORMAT = 'Biiqii'
    SIZE = struct.calcsize('>' + FORMAT)
    # offset to begin of file
    ACCESSIBLE_OFFSET = 7
    USED_OFFSET_OFFSET = 8
    OVERFLOW_OFFSET = 12
    MOD_TIMESTAMP_OFFSET = 16
    ENTRY_OFFSET_OFFSET = 20
    NUM_ENTRIES_OFFSET = 24

    def __init__(self, accessible, used, overflow, mod_timestamp, entry_offset, num_entries):
        self.accessible = accessible
        self.used = used
        self.overflow = overflow
        self.mod_timestamp = mod_timestamp
        self.entry_offset = entry_offset
        self.num_entries = num_entries

    @classmethod
    def from_bytes(cls, data, endian):
        return cls(*struct.unpack(endian + cls.FORMAT, data))

    def to_bytes(self, endian):
        return struct.pack(endian + self.FORMAT, self.accessible, self.used, self.overflow, self.mod_timestamp,
                           self.entry_offset, self.num_entries)

    def __str__(self):
        return f"BufferPrologueV2(accessible={self.accessible}, used={self.used}, overflow={self.overflow}, mod_timestamp={self.mod_timestamp}, entry_offset={self.entry_offset}, num_entries={self.num_entries})"


class EntryHeader:
    FORMAT = 'iiiBBBBi'
    SIZE = struct.calcsize(FORMAT)
    # offset to begin of entry
    ENTRY_LENGTH_OFFSET = 0
    NAME_OFFSET_OFFSET = 4
    VECTOR_LENGTH_OFFSET = 8
    DATA_TYPE_OFFSET = 12
    FLAGS_OFFSET = 13
    DATA_UNITS_OFFSET = 14
    DATA_VAR_OFFSET = 15
    DATA_OFFSET_OFFSET = 16

    def __init__(self, entry_length, name_offset, vector_length, data_type, flags, data_units, data_var, data_offset):
        self.entry_length = entry_length
        self.name_offset = name_offset
        self.vector_length = vector_length
        self.data_type = data_type
        self.flags = flags
        self.data_units = data_units
        self.data_var = data_var
        self.data_offset = data_offset

    @classmethod
    def from_bytes(cls, data, endian):
        return cls(*struct.unpack(endian + cls.FORMAT, data))

    def to_bytes(self, endian):
        return struct.pack(endian + self.FORMAT, self.entry_length, self.name_offset, self.vector_length,
                           self.data_type, self.flags, self.data_units, self.data_var, self.data_offset)

    def __str__(self):
        return f"EntryHeader(entry_length={self.entry_length}, name_offset={self.name_offset}, vector_length={self.vector_length}, data_type={self.data_type}, flags={self.flags}, data_units={self.data_units}, data_var={self.data_var}, data_offset={self.data_offset})"


def get_perfdata_path(pid):
    return os.path.join(tempfile.gettempdir(), f".pyki_perfdata_{pid}")
