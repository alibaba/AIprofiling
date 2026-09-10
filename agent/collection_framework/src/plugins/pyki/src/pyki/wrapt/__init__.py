# type: ignore

from .__wrapt__ import (ObjectProxy, CallableObjectProxy, FunctionWrapper,
        BoundFunctionWrapper, PartialCallableObjectProxy)

from .patches import (resolve_path, apply_patch, wrap_object, wrap_object_attribute,
        function_wrapper, wrap_function_wrapper, patch_function_wrapper,
        transient_function_wrapper)