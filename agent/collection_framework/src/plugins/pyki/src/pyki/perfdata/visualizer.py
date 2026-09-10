import time

from pyki.perfdata.reader import read_perfdata_from_pid


class PerfdataConsoleLogger:

    def __init__(self, pid, interval=1, count=1, prefix="") -> None:
        self.pid = pid
        self.interval = interval
        self.count = count
        self.prefix = prefix

    def start(self):
        for i in range(self.count):
            now = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime())
            print(f"Stats of process {self.pid} at {now}:")
            data = read_perfdata_from_pid(self.pid)
            assert data is not None
            for k, v in data.items():
                if k.startswith(self.prefix):
                    print(f"{k}: {v}")
            if i < self.count - 1:
                time.sleep(self.interval)
                print()
