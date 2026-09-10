from typing import NoReturn

class PyKiError(Exception):
    """
    Error class for PyKi.
    """

    def __init__(self, message, minor: bool = False):
        super().__init__(message)
        self.error_message = message
        self.minor = minor


def raise_error(message: str, minor: bool = False) -> NoReturn:
    raise PyKiError(message, minor)


def raise_error_from(message: str, error, minor: bool = False) -> NoReturn:
    raise PyKiError(message, minor) from error
