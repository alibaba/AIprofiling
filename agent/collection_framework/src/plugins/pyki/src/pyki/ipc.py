import asyncio
import atexit
import json
import logging
import os
import platform
import pyki.attach as attach
import pyki.command as command
import pyki.native.pyki_extension as pyki_extension
import socket
import struct
import threading
import traceback


from contextlib import suppress
from pyki.error import PyKiError, raise_error
from pyki.module_util import update_checker
from pyki.process_util import is_process_exist
from threading import Thread
from typing import Callable, Optional

_logger = logging.getLogger(__name__)

# Payload Format:
#   1 byte:  type
#
# type 1 - command:
#   2 bytes: content length
#   n bytes: json of CommandPayload
_COMMAND = 1

_system = platform.system()


class ConnectionClosedError(PyKiError):
    def __init__(self):
        super().__init__("Connection closed", True)


class _Payload():
    __payload_type__ = _COMMAND


class _CommandPayload(dict, _Payload):

    def to_json(self):
        return json.dumps(self)


class _ClientRequest(_CommandPayload):
    def __init__(self, command: str, arguments: dict) -> None:
        dict.__init__(self, command=command, arguments=arguments)

    @classmethod
    def from_json(cls, json_str: str):
        d = json.loads(json_str)
        return cls(d["command"], d["arguments"])

    @property
    def command(self) -> str:
        return self["command"]

    @property
    def arguments(self) -> dict:
        return self["arguments"]


class _ServerResponse(_CommandPayload):
    """
    format: {
        success:True,
        content:"hello"
        commands:[{
            name: "copy"
            args: {src:"/tmp/file1",dest:"/target/file2"}
        }]
    }
    """

    def __init__(self, success: bool, content: str, commands: list, log: str) -> None:
        dict.__init__(self, success=success, content=content, commands=commands, log=log)

    @classmethod
    def from_json(cls, json_str: str):
        d = json.loads(json_str)
        return cls(d["success"], d["content"], d["commands"], d["log"])

    @property
    def success(self) -> bool:
        return self["success"]

    @property
    def content(self) -> str:
        return self["content"]

    @property
    def commands(self) -> list:
        return self["commands"]

    @property
    def log(self) -> str:
        return self["log"]


def _read(sock, length) -> bytes:
    result = b""
    while len(result) < length:
        data = sock.recv(length - len(result))
        if len(data) == 0:
            raise ConnectionClosedError()
        result += data
    return result


def _read_payload(is_server, sock: socket.socket) -> _Payload:
    data = _read(sock, 1)
    payload_type = data[0]
    if payload_type == _COMMAND:
        data = sock.recv(2)
        length = struct.unpack("@H", data)[0]
        if length == 0:
            raise_error("Empty command payload")
        json_str = _read(sock, length).decode('utf-8')
        cls = _ClientRequest if is_server else _ServerResponse
        return cls.from_json(json_str)
    else:
        raise_error(f"Unsupported payload type: {payload_type}")


def _send_payload(sock: socket.socket, payload):
    payload_type = payload.__payload_type__
    if payload_type == _COMMAND:
        assert isinstance(payload, _CommandPayload)
        bs = payload.to_json().encode('utf-8')
        l = len(bs)
        if l == 0:
            raise_error("Empty command payload")
        if l > 65535:
            raise_error("Command payload too long")
        data = b'\x01' + struct.pack("@H", l) + bs
        sock.sendall(data)
    else:
        raise_error(f"Unsupported payload type: {payload_type}")


def _process_command_request(request: _ClientRequest) -> _ServerResponse:
    payload_type = request.__payload_type__
    success, content, commands, log = True, "", [], ""
    try:
        if payload_type == _COMMAND:
            result = command.run_command(request.command, **request.arguments)
            if isinstance(result, str):
                content = result
            else:
                content, commands = result["message"], result["commands"]
        else:
            raise_error(f"Unsupported payload type: {payload_type}")
    except Exception as e:
        content = f"Error in executing command: {e}"
        _logger.error(content, exc_info=True)
    return _ServerResponse(success, content, commands, log)


def _process_payload_from_client(sock):
    try:
        with suppress(ConnectionClosedError):
            payload = _read_payload(True, sock)
            payload_type = payload.__payload_type__
            if payload_type == _COMMAND:
                assert isinstance(payload, _ClientRequest)
                resp = _process_command_request(payload)
                with suppress(BrokenPipeError):
                    _send_payload(sock, resp)
                    sock.shutdown(socket.SHUT_RDWR)
            else:
                raise_error(f"Unsupported payload type: {payload_type}")
    except Exception as e:
        _logger.error(f"Failed to process payload from client: {e}", exc_info=True)
    finally:
        sock.close()


class _Server:
    _lock = threading.Lock()
    _singleton = None
    _cleaner: Optional[Callable] = None

    def __init__(self) -> None:
        self.address: Optional[str] = None
        self.socket: Optional[socket.socket] = None

    def start(self) -> None:
        with _Server._lock:
            if _Server._singleton is not None:
                return

            self.address = attach.get_self_address()

            # remove the socket file if it already exists
            if os.path.exists(self.address):
                try:
                    os.unlink(self.address)
                except OSError:
                    if os.path.exists(self.address):
                        raise_error(
                            "Failed to remove existing socket file")
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            s.bind(self.address)
            s.listen(5)
            self.socket = s

            if _Server._cleaner is not None:
                atexit.unregister(_Server._cleaner)
            _Server._cleaner = atexit.register(self.cleanup)

            thread = Thread(target=self.loop, daemon=True)
            thread.start()

            _Server._singleton = self


    def cleanup(self) -> None:
        try:
            if self.address is not None and os.path.exists(self.address):
                os.unlink(self.address)
            if self.socket is not None:
                self.socket.close()
        except:
            ...

    def loop(self) -> None:
        try:
            assert self.socket is not None
            while True:
                self.socket.settimeout(10)
                exit_loop = False
                while True:
                    try:
                        sock, addr = self.socket.accept()
                        self.socket.settimeout(None)
                        break
                    except socket.timeout:
                        if update_checker.updated():
                            self.socket.close()
                            exit_loop = True
                            break
                if exit_loop:
                    return

                cred = struct.unpack(
                    "@III", sock.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                uid, gid = cred[1], cred[2]

                if not self.check_permission(uid, gid):
                    sock.close()
                    continue

                threading.Thread(target=_process_payload_from_client, args=(sock,),
                                name="Pyki Process Command").start()
        finally:
            with _Server._lock:
                self.socket = None
                _Server._singleton = None

    def check_permission(self, uid, gid) -> bool:
        return uid == 0 or (uid == os.geteuid() and gid == os.getegid())


class _Client:
    def __init__(self, pid: int) -> None:
        self.address = attach.attach(pid)
        self.pid = pid
        self.socket: Optional[socket.socket] = None

    async def async_request(self, command: str, arguments: dict):
        reader, writer = await asyncio.open_unix_connection(self.address)

        payload = _ClientRequest(command, arguments)
        payload_type = payload.__payload_type__
        if payload_type == _COMMAND:
            assert isinstance(payload, _CommandPayload)
            bs = payload.to_json().encode('utf-8')
            l = len(bs)
            if l == 0:
                raise_error("Empty command payload")
            if l > 65535:
                raise_error("Command payload too long")
            data = b'\x01' + struct.pack("@H", l) + bs
            writer.write(data)
            await writer.drain()
        else:
            raise_error(f"Unsupported payload type: {payload_type}")

        data = await self.read(reader, 1)
        payload_type = data[0]
        if payload_type == _COMMAND:
            data = await self.read(reader, 2)
            length = struct.unpack("@H", data)[0]
            if length == 0:
                raise_error("Empty command payload")
            data = await self.read(reader, length)
            json_str = data.decode('utf-8')
            resp = _ServerResponse.from_json(json_str)
            return True, resp.content, resp.commands, resp.log
        else:
            raise_error(f"Unsupported payload type: {payload_type}")

    async def read(self, reader, bytes):
        data = await reader.read(bytes)
        if len(data) == 0:
            if not is_process_exist(self.pid):
                raise_error(f"Process {self.pid} exited before command completes")
            else:
                raise_error(f"Fail to read payload from process {self.pid}")
        return data


def on_attach():
    # register_samplers()
    update_checker.record_pyki_version()
    init_attach_mechanism()


def init_attach_mechanism():
    _Server().start()


async def async_send_command(pid: int, command: str, arguments: dict, verbose=False):
    try:
        client = _Client(pid)
        return await client.async_request(command, arguments)
    except Exception as e:
        if isinstance(e, PyKiError) and e.minor:
            return False, f"{e}", [], ""
        else:
            return False, traceback.format_exc(), [], ""
