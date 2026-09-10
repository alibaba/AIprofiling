import argparse
import os
import traceback
from pyki.attach import attach
import pyki.constant
import shutil
import sys

from pyki.process_util import get_nspid, is_process_exist, get_process_commandline
from pyki.response_formatter import ResponseFormatter, CheckImportsResponseFormatter
from typing import Dict, Type


_fmt_cls_dict: Dict[str, Type[ResponseFormatter]] = {
    "check_imports": CheckImportsResponseFormatter
}


def _run_ps(args):
    import tempfile
    import os
    mypid = os.getpid()
    # todo: we don't support list process in container now, though we can send command
    # and read perfdata there
    for file in os.listdir(tempfile.gettempdir()):
        if file.startswith(".pyki_pid_"):
            try:
                pid = int(file[10:])
                if pid == mypid or not is_process_exist(pid):
                    continue
                line = str(pid)
                if args.v:
                    line += f"\t{get_process_commandline(pid)}"
                print(line)
            except:
                pass


def _define_ps(subparsers, funcs):
    list_parser = subparsers.add_parser(
        "ps", help="list all processes known by pyki")
    list_parser.add_argument(
        "-v", help="increase show process detail", action="store_true")

    funcs["ps"] = _run_ps


def _get_additional_arguments(pid):
    nspid = get_nspid(pid)
    in_container = (pid != nspid)
    result = {
        "working_dir": os.getcwd(),
        "in_container": in_container,
        "host_pid": pid
    }
    return result


def _move_file_from_container(args):
    shutil.move(args["src"], args["dest"])


def _execute_client_command(commands):
    command_dict = {
        "move_file_from_container": _move_file_from_container
    }
    for command in commands:
        if command is not None:
            command_dict[command["name"]](command["args"])


def _run_cmd(args):
    from pyki.ipc import async_send_command
    command_args = {}
    for argument in args.argument:
        if "=" in argument:
            key, value = argument.split("=", 1)
            command_args[key] = value
        else:
            command_args[argument] = True

    pids = list(set(args.pid))
    results = {}

    import asyncio

    async def _async_run():
        tasks = []
        for i, pid in enumerate(pids):
            args_for_process = command_args.copy()
            args_for_process[pyki.constant.ADDITIONAL_ARGS] = _get_additional_arguments(
                pid)
            # asyncio.create_task is introduced in Python 3.7, fallback to ensure_future for Python 3.6
            _create_task = getattr(asyncio, "create_task", asyncio.ensure_future)
            task = _create_task(async_send_command(
                pid, args.command, args_for_process, args.verbose))
            tasks.append(task)

        for i, task in enumerate(tasks):
            await task
            results[pids[i]] = task.result()

    if hasattr(asyncio, "run"):
        asyncio.run(_async_run())
    else:
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        try:
            loop.run_until_complete(_async_run())
        finally:
            loop.close()
            asyncio.set_event_loop(None)

    print_header = len(pids) > 1
    all_succeeded = True

    for i, pid in enumerate(pids):
        succeeded, response, commands, log = results[pid]
        try:
            _execute_client_command(commands)
        except Exception:
            results[pid] = (False, traceback.format_exc(), [], "")

    if args.command in _fmt_cls_dict:
        formatter_cls: Type[ResponseFormatter] = _fmt_cls_dict[args.command]
        formatter = formatter_cls()
        for i, pid in enumerate(pids):
            succeeded, response, commands, log = results[pid]
            formatter.add_response(pid, response)
            if not succeeded:
                all_succeeded = False

        print(formatter.format())

    else:
        for i, pid in enumerate(pids):
            succeeded, response, commands, log = results[pid]
            if i > 0:
                print()
            if print_header:
                print(f"[PID: {pid}]")
            print(response)
            if log is not None and log != "":
                print()
                print("pyki log on server:")
                print(log)
            if not succeeded:
                all_succeeded = False
    if not all_succeeded:
        exit(1)


def _define_cmd(subparsers, funcs):
    cmd_parser = subparsers.add_parser(
        "cmd", help="send command to target processes")
    group = cmd_parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "-p", "--pid", help="specify pids of target processes", type=int, action="append")

    cmd_parser.add_argument("-v", "--verbose",
                            help="increase output verbosity", action="store_true")

    cmd_parser.add_argument("command", help="command name")
    cmd_parser.add_argument("argument", nargs="*",
                            help="command arguments, using <key>=<value> or <key> format")

    funcs["cmd"] = _run_cmd


def _run_stat(args):
    from pyki.perfdata.visualizer import PerfdataConsoleLogger
    PerfdataConsoleLogger(args.pid, args.interval,
                          args.count, args.prefix).start()


def _define_stat(subparsers, funcs):
    cmd_parser = subparsers.add_parser(
        "stat", help="print stats of target processes")
    cmd_parser.add_argument(
        "-p", "--pid", help="specify pid of target process", type=int)
    cmd_parser.add_argument(
        "-c", "--count", help="specify how many times to print stats", type=int, default=1)
    cmd_parser.add_argument(
        "-i", "--interval", help="specify interval to print stats", type=float, default=1.0)
    cmd_parser.add_argument(
        "--prefix", help="specify prefix to print stats", type=str, default="")

    funcs["stat"] = _run_stat


def _run_attach(args):
    try:
        attach(args.pid)
    except Exception as e:
        print(f"{e}")
        sys.exit(-1)


def _define_attach(subparsers, funcs):
    attach_parser = subparsers.add_parser(
        "attach", help="attach to target process")
    attach_parser.add_argument(
        "pid", help="specify pid of target process", type=int)

    funcs["attach"] = _run_attach


_DESC = """pyki: a tool to help send command to target processes.
Run "pyki -p [pid] list_commands" to check all commands pyki supports
"""


def main():
    argparse.SUPPRESS
    parser = argparse.ArgumentParser(prog="pyki", description=_DESC)

    subparsers = parser.add_subparsers(
        dest="subcommand", title="supported subcommands")
    subparsers.required = True

    subparsers.default = "ps"

    funcs = {}
    _define_ps(subparsers, funcs)
    _define_cmd(subparsers, funcs)
    _define_stat(subparsers, funcs)
    _define_attach(subparsers, funcs)

    args = parser.parse_args()
    funcs[args.subcommand](args)


if __name__ == "__main__":
    main()
