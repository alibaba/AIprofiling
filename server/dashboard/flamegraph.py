# -*- coding: utf-8 -*-
"""FlameGraphConverter — folded-stack text → Pyroscope differential flamegraph JSON.

Pure algorithm, no external dependencies. AIProf's diff_analysis uses
convert_to_pyroscope_diff to produce the "double" (differential) flamegraph
data required by the frontend @pyroscope/flamegraph component.

Input folded-stack line format: ``symbol;path;goes;here 1234``
(semicolon-separated call stack, sample count after the space).
"""

from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple, Any


@dataclass
class Node:
    """A single node in the flamegraph call-stack tree."""
    sample: int = 0  # total samples for this node and its subtree
    children: Dict[str, 'Node'] = field(default_factory=dict)  # symbol -> Node


@dataclass
class ProfilingData:
    """Parsed result container for a single profiling run."""
    tree: Node
    units: str = "samples"
    samples: Optional[List[Tuple[float, int]]] = None  # timeline: (timestamp, value)
    instance: Optional[str] = None
    pid: Optional[int] = None
    table: Optional[str] = None


class FlameGraphConverter:
    def __init__(self):
        pass

    def _parse_lines_to_rows(self, lines_str: str) -> Dict[str, int]:
        """Folded-stack text → {symbol_path: total_sample}."""
        rows: Dict[str, int] = {}
        for line in lines_str.split('\n'):
            line = line.strip()
            if not line:
                continue
            parts = line.rsplit(' ', 1)
            if len(parts) != 2:
                continue
            symbol_path, sample_str = parts
            try:
                sample = float(sample_str)
            except ValueError:
                continue
            rows[symbol_path] = rows.get(symbol_path, 0) + sample
        return rows

    def _add_line_to_tree(self, root: Node, symbol_path: str, sample: int):
        """Accumulate one call-stack path into the tree; every node along the path adds the sample."""
        current = root
        current.sample += sample
        for sym in symbol_path.split(';'):
            if sym not in current.children:
                current.children[sym] = Node()
            current = current.children[sym]
            current.sample += sample

    def _build_tree_from_rows(self, rows: Dict[str, int]) -> Node:
        root = Node()
        for sym_path, sample in rows.items():
            self._add_line_to_tree(root, sym_path, sample)
        return root

    def _build_profiling_data(self, raw_text: str, units: str = "samples",
                              samples_timeline: Optional[List[Tuple[float, int]]] = None,
                              instance: Optional[str] = None, pid: Optional[int] = None,
                              table: Optional[str] = None) -> ProfilingData:
        rows = self._parse_lines_to_rows(raw_text)
        tree = self._build_tree_from_rows(rows)
        return ProfilingData(tree=tree, units=units, samples=samples_timeline,
                             instance=instance, pid=pid, table=table)

    # --- Pyroscope conversion helpers ---

    def _calc_sum_children(self, node: Optional[Node]) -> int:
        if not node:
            return 0
        return sum(child.sample for child in node.children.values())

    def _calc_self_sample(self, node: Optional[Node]) -> int:
        if not node:
            return 0
        return node.sample - self._calc_sum_children(node)

    def _time_line(self, samples_timeline: Optional[List[Tuple[float, int]]]) -> Dict[str, Any]:
        res: Dict[str, Any] = {"durationDelta": 10, "samples": []}
        if samples_timeline:
            res["startTime"] = samples_timeline[0][0]
            res["samples"] = [s[1] for s in samples_timeline]
        else:
            res["startTime"] = 0
        return res

    # --- Pyroscope differential flamegraph conversion ---

    def _add_pyro_cell_diff(self, flamebearer_data: Dict[str, Any],
                            left_node: Optional[Node], right_node: Optional[Node],
                            tag_index: int, level: int,
                            current_node_abs_pos_left: int,
                            current_node_abs_pos_right: int):
        """Append one differential cell (7-tuple) to flamebearer levels:
        [offset_left, w_left, s_left, offset_right, w_right, s_right, name_idx].
        The offset is delta-encoded relative to the absolute end position of the previous bar on the same level.
        """
        levels = flamebearer_data['levels']
        if level - 1 >= len(levels):
            levels.extend([[] for _ in range(level - len(levels))])
        level_now = levels[level - 1]

        sample_left = left_node.sample if left_node else 0
        self_left_now = self._calc_self_sample(left_node)
        sample_right = right_node.sample if right_node else 0
        self_right_now = self._calc_self_sample(right_node)

        prev_x_left = flamebearer_data['pos_tracking_diff_left'].get(level, 0)
        prev_x_right = flamebearer_data['pos_tracking_diff_right'].get(level, 0)

        offset_left = current_node_abs_pos_left - prev_x_left
        offset_right = current_node_abs_pos_right - prev_x_right

        level_now.extend([
            offset_left, sample_left, self_left_now,
            offset_right, sample_right, self_right_now,
            tag_index,
        ])

        flamebearer_data['pos_tracking_diff_left'][level] = current_node_abs_pos_left + sample_left
        flamebearer_data['pos_tracking_diff_right'][level] = current_node_abs_pos_right + sample_right
        flamebearer_data['maxSelf'] = max(flamebearer_data['maxSelf'], max(self_left_now, self_right_now))

    def _rec_pyro_diff(self, flamebearer_data: Dict[str, Any],
                       left_node: Optional[Node], right_node: Optional[Node],
                       tag: str, level: int,
                       current_node_abs_pos_left: int,
                       current_node_abs_pos_right: int):
        names = flamebearer_data['names']
        tag_to_index = flamebearer_data['tag_to_index']
        if tag not in tag_to_index:
            tag_to_index[tag] = len(names)
            names.append(tag)
        current_tag_index = tag_to_index[tag]

        self._add_pyro_cell_diff(flamebearer_data, left_node, right_node,
                                 current_tag_index, level,
                                 current_node_abs_pos_left, current_node_abs_pos_right)

        next_sibling_abs_pos_left = current_node_abs_pos_left
        next_sibling_abs_pos_right = current_node_abs_pos_right

        all_child_keys = set()
        if left_node and left_node.children:
            all_child_keys.update(left_node.children.keys())
        if right_node and right_node.children:
            all_child_keys.update(right_node.children.keys())

        for sym_key in sorted(all_child_keys):
            left_child = left_node.children.get(sym_key) if left_node else None
            right_child = right_node.children.get(sym_key) if right_node else None
            self._rec_pyro_diff(flamebearer_data, left_child, right_child, sym_key,
                                level + 1, next_sibling_abs_pos_left, next_sibling_abs_pos_right)
            if left_child:
                next_sibling_abs_pos_left += left_child.sample
            if right_child:
                next_sibling_abs_pos_right += right_child.sample

    def convert_to_pyroscope_diff(self, left_raw_text: str, right_raw_text: str,
                                  left_metadata: Dict[str, Any] = None,
                                  right_metadata: Dict[str, Any] = None) -> Dict[str, Any]:
        """Two folded-stack texts → Pyroscope differential (double) JSON."""
        left_metadata = left_metadata or {}
        right_metadata = right_metadata or {}

        left_data = self._build_profiling_data(
            left_raw_text, units=left_metadata.get('units', 'samples'),
            samples_timeline=left_metadata.get('samples_timeline'),
            instance=left_metadata.get('instance'), pid=left_metadata.get('pid'),
            table=left_metadata.get('table'))
        right_data = self._build_profiling_data(
            right_raw_text, units=right_metadata.get('units', 'samples'),
            samples_timeline=right_metadata.get('samples_timeline'),
            instance=right_metadata.get('instance'), pid=right_metadata.get('pid'),
            table=right_metadata.get('table'))

        flamebearer_data_temp: Dict[str, Any] = {
            'names': [], 'tag_to_index': {}, 'levels': [], 'maxSelf': 0,
            'pos_tracking_diff_left': {}, 'pos_tracking_diff_right': {},
        }

        self._rec_pyro_diff(flamebearer_data_temp, left_data.tree, right_data.tree,
                            "total", 1, 0, 0)

        pyroscope_json_output = {
            "version": 1,
            "flamebearer": {
                "names": flamebearer_data_temp['names'],
                "levels": flamebearer_data_temp['levels'],
                "numTicks": left_data.tree.sample + right_data.tree.sample,
                "maxSelf": flamebearer_data_temp['maxSelf'],
            },
            "metadata": {
                "appName": "liveTrace.profiling",
                "name": "liveTrace",
                "sampleRate": 100,
                "spyName": "cprof",
                "units": left_data.units,
                "format": "double",
                "leftTicks": left_data.tree.sample,
                "rightTicks": right_data.tree.sample,
            },
            "other": {
                "leftInstance": left_data.instance,
                "rightInstance": right_data.instance,
                "leftPid": left_data.pid,
                "rightPid": right_data.pid,
                "table": left_data.table,
            },
        }

        if left_data.samples or right_data.samples:
            pyroscope_json_output["timeLine"] = {
                "left": self._time_line(left_data.samples),
                "right": self._time_line(right_data.samples),
            }

        return pyroscope_json_output
