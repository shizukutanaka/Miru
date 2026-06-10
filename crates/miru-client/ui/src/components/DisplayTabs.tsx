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
    <div className="display-tabs">
      {displays.map((d) => {
        const ratio = d.width / d.height;
        const w = 32;
        const h = Math.round(w / ratio);
        const isSelected = d.index === selected;

        return (
          <button
            key={d.index}
            className={`display-tab ${isSelected ? "selected" : ""}`}
            onClick={() => onSelect(d.index)}
            title={`${d.name} • ${d.width}×${d.height} @ ${d.refresh_hz}Hz`}
          >
            <div
              className="display-thumb"
              style={{ width: `${w}px`, height: `${h}px` }}
            />
            <span className="display-label">
              {d.primary ? "メイン" : `画面 ${d.index + 1}`}
            </span>
          </button>
        );
      })}
    </div>
  );
}
