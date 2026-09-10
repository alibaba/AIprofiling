# SPDX-License-Identifier: Apache-2.0
"""Unit tests for report._top_stacks.

The function takes a Brendan-Gregg folded flamegraph (each line is
"frame1;frame2;...;leaf <count>") and returns the N heaviest stacks in
the same format, sorted by count descending.

The report layer feeds this string into the LLM prompt, so getting the
format right matters — earlier a bug flipped "<count> <stack>" and the
offline heuristic couldn't parse it back.
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from server.services.report.app.main import _top_stacks  # noqa: E402


def test_top_stacks_sorts_descending_and_keeps_folded_format():
    folded = "\n".join([
        "main;a 5",
        "main;b 20",
        "main;c 1",
        "main;d 12",
    ])
    got = _top_stacks(folded, n=2).splitlines()
    assert got == ["main;b 20", "main;d 12"]


def test_top_stacks_ignores_blank_and_malformed_lines():
    folded = "\n".join([
        "",
        "main;a 3",
        "this line has no count",
        "  ",
        "main;b 7",
    ])
    got = _top_stacks(folded, n=10).splitlines()
    assert got == ["main;b 7", "main;a 3"]


def test_top_stacks_limits_to_n():
    folded = "\n".join(f"stk{i} {i}" for i in range(1, 21))
    got = _top_stacks(folded, n=3).splitlines()
    # Highest three counts (20, 19, 18) in descending order.
    assert got == ["stk20 20", "stk19 19", "stk18 18"]
