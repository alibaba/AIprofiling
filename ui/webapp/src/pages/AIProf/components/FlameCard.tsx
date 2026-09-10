// SPDX-License-Identifier: Apache-2.0
import React, { useMemo, useState } from 'react';
import { Card, Input, Space, Button, Tooltip } from 'antd';
import {
  SearchOutlined,
  FullscreenOutlined,
  RobotOutlined,
  ReloadOutlined,
} from '@ant-design/icons';
import { FlamegraphRenderer } from '@pyroscope/flamegraph';

interface Props {
  folded: string;
  onOpenReport?: () => void;
}

function foldedToPyroscope(folded: string) {
  const names = ['total'];
  const levels: number[][] = [[]];
  const nameIndex = new Map<string, number>();
  nameIndex.set('total', 0);

  type Node = { name: string; total: number; self: number; children: Map<string, Node> };
  const root: Node = { name: 'total', total: 0, self: 0, children: new Map() };

  for (const line of folded.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const lastSpace = trimmed.lastIndexOf(' ');
    if (lastSpace <= 0) continue;
    const stack = trimmed.slice(0, lastSpace);
    const count = parseInt(trimmed.slice(lastSpace + 1), 10);
    if (!count || Number.isNaN(count)) continue;

    root.total += count;
    let cur = root;
    const frames = stack.split(';').filter(Boolean);
    for (const f of frames) {
      let child = cur.children.get(f);
      if (!child) {
        child = { name: f, total: 0, self: 0, children: new Map() };
        cur.children.set(f, child);
      }
      child.total += count;
      cur = child;
    }
    cur.self += count;
  }

  function idOf(name: string) {
    if (!nameIndex.has(name)) {
      nameIndex.set(name, names.length);
      names.push(name);
    }
    return nameIndex.get(name)!;
  }

  function walk(node: Node, depth: number, offset: number) {
    while (levels.length <= depth) levels.push([]);
    levels[depth].push(offset, node.total, node.self, idOf(node.name));
    let childOffset = offset;
    for (const child of node.children.values()) {
      walk(child, depth + 1, childOffset);
      childOffset += child.total;
    }
  }
  walk(root, 0, 0);

  return {
    version: 1,
    flamebearer: {
      names,
      levels,
      numTicks: root.total,
      maxSelf: Math.max(root.self, ...names.map((_, i) =>
        levels.flat().filter((_, idx, arr) => idx % 4 === 2)[i] ?? 0)),
    },
    metadata: { format: 'single', sampleRate: 100, spyName: 'unknown', units: 'samples' },
  } as any;
}

export const FlameCard: React.FC<Props> = ({ folded, onOpenReport }) => {
  const [keyword, setKeyword] = useState('');
  const [seq, setSeq] = useState(0);
  const flamebearer = useMemo(() => foldedToPyroscope(folded), [folded, seq]);

  const requestFullscreen = () => {
    const el = document.getElementById('aiprof-flame-wrap');
    if (el?.requestFullscreen) el.requestFullscreen();
  };

  return (
    <Card
      title="Flamegraph"
      extra={
        <Space size="small">
          <Input
            allowClear
            size="small"
            prefix={<SearchOutlined />}
            placeholder="Search frame"
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
            style={{ width: 200 }}
          />
          <Tooltip title="Reset">
            <Button size="small" icon={<ReloadOutlined />} onClick={() => setSeq((s) => s + 1)} />
          </Tooltip>
          <Tooltip title="Fullscreen">
            <Button size="small" icon={<FullscreenOutlined />} onClick={requestFullscreen} />
          </Tooltip>
          <Button size="small" type="primary" icon={<RobotOutlined />} onClick={onOpenReport}>
            AI Analysis
          </Button>
        </Space>
      }
      styles={{ body: { padding: 8 } }}
    >
      <div id="aiprof-flame-wrap" style={{ background: '#fff', minHeight: 380 }}>
        {folded.trim() ? (
          <FlamegraphRenderer
            key={seq}
            profile={flamebearer}
            colorMode="light"
            showToolbar={false}
            panesOrientation="horizontal"
            sharedQuery={{ searchQuery: keyword, onQueryChange: setKeyword } as any}
          />
        ) : (
          <div style={{ padding: 40, textAlign: 'center', color: '#9ca3af' }}>
            No flamegraph data
          </div>
        )}
      </div>
    </Card>
  );
};

export default FlameCard;
