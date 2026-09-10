from abc import ABC, abstractmethod
from pyki.error import raise_error, raise_error_from
from enum import Enum
import logging
import threading

_logger = logging.getLogger(__name__)


class ProfileStatus(Enum):
    NOT_STARTED = 0
    PROFILING = 1


class BaseProfiler(ABC):
    """
    Base class for profiler
    """

    def __init__(self, name):
        self.name = name
        self.profile_status = ProfileStatus.NOT_STARTED
        self.profile_config = None
        self.lock = threading.Lock()

    def start_profile(self, config=None):
        with self.lock:
            if self.is_profiling():
                raise_error(f"{self.name} is already running")
            self.profile_config = config
            try:
                self._start_profile(config)
                self.profile_status = ProfileStatus.PROFILING
                _logger.info(f"{self.name} started")
            except Exception as e:
                raise_error_from(f"Fail to start {self.name}", e)

        self._post_start_profile()

    @abstractmethod
    def _start_profile(self, config):
        pass

    def _post_start_profile(self):
        pass

    @abstractmethod
    def _stop_profile(self):
        pass

    def stop_profile(self):
        with self.lock:
            if not self.is_profiling():
                raise_error(f"{self.name} is not running")
            try:
                self._stop_profile()
                _logger.info(f"{self.name} stopped")
            except Exception as e:
                raise_error_from(f"Fail to stop {self.name}", e)
            finally:
                self.profile_status = ProfileStatus.NOT_STARTED

    def finalize(self):
        pass

    def is_profiling(self):
        return self.profile_status == ProfileStatus.PROFILING
