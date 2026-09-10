from abc import ABC, abstractmethod


class ResponseFormatter:
    """
    A class to format the response of a command.
    """

    def __init__(self):
        self.responses = None
        pass

    def add_response(self, pid, response):
        if self.responses is None:
            self.responses = {}
        self.responses[pid] = response

    @abstractmethod
    def format(self):
        pass


class CheckImportsResponseFormatter(ResponseFormatter):
    def __init__(self):
        super().__init__()
        self.pids = []

    def add_response(self, pid, response):
        if response == "true":
            self.pids.append(pid)

    def format(self):
        return ",".join(map(str, self.pids))
