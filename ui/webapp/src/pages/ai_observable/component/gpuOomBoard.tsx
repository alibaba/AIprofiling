import React, { useMemo } from "react";
import type { OomItem } from './metaData';
interface OomProps {
  data: OomItem[];
  /** 绘图区域宽度（内部），最长框基本占满这个宽度 */
  width?: number;
  /** 绘图区域可视高度 */
  viewHeight?: number;
  /** 标题，由父组件传入 */
  title?: string;
}
const HEIGHT_SCALE = 2;     // size -> 基础高度（像素）
const MIN_BAR_HEIGHT = 4;   // 最小条高度
const MIN_BAR_WIDTH = 4;    // 最小条宽度
const SizeFlameGraphNoScroll: React.FC<OomProps> = ({
  data,
  width = 600,
  viewHeight = 300,
  title = "帧分布可视化",
}) => {
  const { blocks, maxSize } = useMemo(() => {
    if (!data.length) {
      return {
        blocks: [] as {
          item: OomItem;
          baseWidth: number;
          baseHeight: number;
        }[],
        maxSize: 0,
      };
    }
    // 按 start_time 排序：越早的排前面，之后会越靠下
    const sorted = [...data].sort((a, b) => a.start_time - b.start_time);
    const sizes = sorted.map((d) => d.size);
    const maxSize = Math.max(...sizes);
    const SIZE_SCALE_X = maxSize === 0 ? 0 : width / maxSize;
    // 先计算“基础宽度/高度”（未压缩的）
    const baseBlocks = sorted.map((item) => {
      const baseWidth = Math.max(item.size * SIZE_SCALE_X, MIN_BAR_WIDTH);
      const baseHeight = Math.max(item.size * HEIGHT_SCALE, MIN_BAR_HEIGHT);
      return { item, baseWidth, baseHeight };
    });
    return {
      blocks: baseBlocks,
      maxSize,
    };
  }, [data, width]);
  if (!blocks.length) {
    // 即使没有数据，也显示标题容器
    return (
      <div
        style={{
          width,
          border: "1px solid #ddd",
          boxSizing: "border-box",
          fontFamily: "sans-serif",
          background: "#fff",
        }}
      >
        {/* 标题（可换行） */}
        <div
          style={{
            padding: "6px 8px",
            borderBottom: "1px solid #eee",
            fontSize: 14,
            fontWeight: 600,
            backgroundColor: "#f0f0f0",
            color: "#222",
            whiteSpace: "normal",
            wordBreak: "break-all",
          }}
        >
          {title}
        </div>
        <div
          style={{
            height: viewHeight,
            background: "#fafafa",
          }}
        />
      </div>
    );
  }
  // 计算总的“基础高度”
  const totalBaseHeight = blocks.reduce((sum, b) => sum + b.baseHeight, 0);
  // 如果总高度大于 viewHeight，则做垂直压缩
  const compressRatio =
    totalBaseHeight > viewHeight ? viewHeight / totalBaseHeight : 1;
  // 重新计算每个条的 top/height（压缩后），并保证 start_time 越早越靠下
  let acc = 0;
  const layoutBlocks = [...blocks]
    .slice()
    .reverse()
    .map((b) => {
      const height = Math.max(b.baseHeight * compressRatio, MIN_BAR_HEIGHT);
      const top = viewHeight - acc - height;
      acc += height;
      const barWidth = b.baseWidth; // 宽度不随垂直压缩变化
      const left = 0;
      return {
        item: b.item,
        width: barWidth,
        height,
        left,
        top,
      };
    });
  return (
    <div
      style={{
        width,
        border: "1px solid #ddd",
        boxSizing: "border-box",
        fontFamily: "sans-serif",
        background: "#fff",
      }}
    >
      {/* 标题区域：宽度自适应，长文本自动换行 */}
      <div
        style={{
          padding: "6px 8px",
          borderBottom: "1px solid #eee",
          fontSize: 14,
          fontWeight: 600,
          backgroundColor: "#f0f0f0",
          color: "#222",
          whiteSpace: "normal",     // 允许换行
          wordBreak: "break-all",   // 单词/长串打断换行
          boxSizing: "border-box",
        }}
      >
        {title}
      </div>
      {/* 固定高度的绘图区（无滚动） */}
      <div
        style={{
          position: "relative",
          width,
          height: viewHeight,
          overflow: "hidden",
          background: "#fafafa",
        }}
      >
        {layoutBlocks.map((b, index) => {
          const hueBase = 20;
          const hue = (hueBase + (index * 7) % 30) % 360;
          const color = `hsl(${hue}, 80%, 55%)`;
          const { item, width: barWidth, height, left, top } = b;
          return (
            <div
              key={`${item.start_time}-${item.end_time}-${index}`}
              style={{
                position: "absolute",
                left,
                top,
                width: barWidth,
                height,
                backgroundColor: color,
                boxSizing: "border-box",
                border: "1px solid rgba(0,0,0,0.15)",
                borderRadius: 2,
                overflow: "hidden",
                whiteSpace: "nowrap",
                textOverflow: "ellipsis",
                fontSize: 11,
                color: "#fff",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                cursor: "default",
              }}
              title={`start_time=${item.start_time}, size=${item.size}`}
            >
              {item.frames}
            </div>
          );
        })}
        {/* 可选：size 刻度，仅作参考 */}
        {maxSize > 0 && (
          <div
            style={{
              position: "absolute",
              left: 0,
              bottom: 0,
              width: "100%",
              height: 16,
              fontSize: 10,
              color: "#666",
            }}
          >
            {[0, 0.25, 0.5, 0.75, 1].map((ratio, i) => (
              <div
                key={i}
                style={{
                  position: "absolute",
                  left: width * ratio,
                  bottom: 0,
                  transform: "translateX(-50%)",
                  textAlign: "center",
                }}
              >
                <div
                  style={{
                    width: 1,
                    height: 4,
                    background: "#999",
                    margin: "0 auto 1px",
                  }}
                />
                <span>{(maxSize * ratio).toFixed(0)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
};
export default SizeFlameGraphNoScroll;