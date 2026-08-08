import { useEffect, useState } from "react";

export interface DisplayInfo {
  index: number;
  width: number;
  height: number;
  refresh_hz: number;
  name: string;
  primary: boolean;
}

interface Props {
  displays: DisplayInfo[];
  selected: number;
  onSelect: (index: number) => void;
}

/**
 * Compact tab bar shown when host has 2+ displays.
 * Renders a miniature aspect-ratio thumbnail for each.
 */
export function DisplayTabs({ displays, selected, onSelect }: Props) {
  if (displays.length <= 1) return null;

  return (
    <div className="display-tabs" role="tablist" aria-label="表示するディスプレイ">
      {displays.map((d) => {
        const ratio = d.width / d.height;
        const w = 32;
        const h = Math.round(w / ratio);
        const isSelected = d.index === selected;
        const label = d.primary ? "メイン" : `画面 ${d.index + 1}`;

        return (
          <button
            key={d.index}
            role="tab"
            aria-selected={isSelected}
            aria-label={`${label}: ${d.name} ${d.width}×${d.height} @ ${d.refresh_hz}Hz`}
            className={`display-tab ${isSelected ? "selected" : ""}`}
            onClick={() => onSelect(d.index)}
            title={`${d.name} • ${d.width}×${d.height} @ ${d.refresh_hz}Hz`}
          >
            <div
              className="display-thumb"
              aria-hidden="true"
              style={{ width: `${w}px`, height: `${h}px` }}
            />
            <span className="display-label">{label}</span>
          </button>
        );
      })}
    </div>
  );
}
