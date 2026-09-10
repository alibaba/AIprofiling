import os
import site
import sys
from typing import List


def _prefix_regex() -> List[str]:
    raw_paths = (
        site.getsitepackages()
        + sys.path
        + [site.getuserbase()]
        + [site.getusersitepackages()]

    )
    try:
        import torch
        raw_paths.extend([os.path.dirname(os.path.dirname(torch.__file__))])
    except ImportError:
        pass

    path_prefixes = sorted({os.path.abspath(i) for i in raw_paths}, reverse=True)
    assert all(isinstance(i, str) for i in path_prefixes)
    return [i + os.sep for i in path_prefixes]
