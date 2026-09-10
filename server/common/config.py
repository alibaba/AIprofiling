# SPDX-License-Identifier: Apache-2.0
"""Shared config for AIProf server micro-services.

Reads settings from environment variables so the same image works in
docker-compose, in Kubernetes, and locally. Defaults are tuned for a
single-host quickstart.
"""

from __future__ import annotations

import os
from dataclasses import dataclass


_DATA_DIR = os.getenv("AIPROF_DATA_DIR", "./data")


@dataclass
class Settings:
    db_url: str = os.getenv(
        "AIPROF_DB_URL", f"sqlite:///{_DATA_DIR}/aiprof.db"
    )
    blob_dir: str = os.getenv("AIPROF_BLOB_DIR", f"{_DATA_DIR}/blobs")
    ai_gateway_url: str = os.getenv(
        "AIPROF_AI_URL", "http://localhost:8642/v1/chat/completions"
    )
    ai_api_key: str = os.getenv("AIPROF_AI_KEY", "")
    upload_token: str = os.getenv("AIPROF_UPLOAD_TOKEN", "")  # "" == disabled


settings = Settings()
