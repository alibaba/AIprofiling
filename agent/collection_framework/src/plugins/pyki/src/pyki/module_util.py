import re
import os
import sys
import importlib
try:
    import importlib.metadata as _importlib_metadata
except ImportError:  # Python < 3.8
    import importlib_metadata as _importlib_metadata
import importlib.util
import subprocess


def is_package_installed(package_name: str, version_spec: str = "") -> bool:
    try:
        # todo: we can use "packaging" package to support better version check
        installed_version = _importlib_metadata.version(package_name)
        return True
    except _importlib_metadata.PackageNotFoundError:
        return False


def install_packages(*package_specs): # pragma: no cover
    command = ["pip", "install", *package_specs]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
    stdout, stderr = process.communicate()
    if process.returncode != 0:
        raise Exception(f"Error installing packages {package_specs}: \nstdout: \n{stdout}\nstderr:\n{stderr}")


def guarantee_packages_installed(*package_descs): # pragma: no cover
    regex = re.compile(r'([a-zA-Z0-9_-]+)(([><~=!]+.*)?)')
    packages_to_install = []
    for package_desc in package_descs:
        match = regex.match(package_desc)
        assert match is not None
        package_name = match.group(1)
        version_spec = match.group(2)
        if not is_package_installed(package_name, version_spec):
            packages_to_install.append(package_desc)
    if len(packages_to_install) > 0:
        install_packages(*packages_to_install)
        importlib.invalidate_caches()


def is_module_exist(module_name: str) -> bool: # pragma: no cover
    # 分割模块名以处理子模块
    parts = module_name.split('.')
    package_name = parts[0]
    sub_module_name = '.'.join(parts[1:]) if len(parts) > 1 else None

    # 查找父模块的规范
    package_spec = importlib.util.find_spec(package_name)
    if package_spec is None:
        return False

    # 如果没有子模块，直接返回父模块是否存在
    if sub_module_name is None:
        return True

    # 查找子模块的规范
    if package_spec.origin == 'namespace':
        # 处理命名空间包
        if package_spec.submodule_search_locations is not None:
            for path in package_spec.submodule_search_locations:
                sub_module_path = os.path.join(path, *sub_module_name.split('.')) + '.py'
                if os.path.exists(sub_module_path):
                    return True
        return False
    elif package_spec.origin is not None:
        # 处理常规包
        package_path = os.path.dirname(package_spec.origin)
        sub_module_path = os.path.join(package_path, *sub_module_name.split('.')) + '.py'
        return os.path.exists(sub_module_path)
    else:
        return False


def is_module_imported(module_name: str) -> bool:
    return module_name in sys.modules


def reimport_module(module_name: str): # pragma: no cover
    if is_module_imported(module_name):
        module = sys.modules[module_name]
        importlib.reload(module)


def get_package_version(package_name):
    try:
        return _importlib_metadata.version(package_name)
    except Exception:
        return None


def get_cuda_version():
    try:
        import torch
        return torch.version.cuda
    except Exception:
        return None


class UpdateChecker:
    def __init__(self):
        self.version = None

    def record_pyki_version(self):
        match = re.search(r'/pyki/v([^/]+)/module_util', __file__)
        if match:
            version_string = match.group(1)
            self.version = version_string.replace("_", ".")
        else:
            self.version = None

    def updated(self):
        if self.version is None:
            return False
        version = get_package_version("pyki")
        return version is not None and self.version != version


update_checker = UpdateChecker()
