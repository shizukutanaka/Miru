import { useEffect, useRef, useState } from "react";
import { api, RecordingSummary } from "../lib/tauri";

interface Props {
  onClose: () => void;
}

export function TimelineScrubber({ onClose }: Props) {
  const [recordings, setRecordings] = useState<RecordingSummary[]>([]);
  const [selected, setSelected] = useState<RecordingSummary | null>(null);
  const [playbackPos, setPlaybackPos] = useState(0); // 0 - 1 normalized
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [bookmarks, setBookmarks] = useState<number[]>([]);
  const [frameB64, setFrameB64] = useState<string | null>(null);
  const fetchingRef = useRef(false);

  useEffect(() => {
    api.listRecordings().then(setRecordings).catch(() => {});
  }, []);

  // Fetch frame when playback position changes
  useEffect(() => {
    if (!selected || fetchingRef.current) return;
    const frameIdx = Math.floor(playbackPos * Math.max(1, selected.frame_count - 1));
    fetchingRef.current = true;
    api.getRecordingFrame(selected.path, frameIdx)
      .then(setFrameB64)
      .catch(() => setFrameB64(null))
      .finally(() => { fetchingRef.current = false; });
  }, [selected, Math.floor(playbackPos * (selected?.frame_count ?? 1))]);

  // Playback advance loop
  useEffect(() => {
    if (!playing || !selected) return;
    const totalMs = selected.duration_ms || 60_000;
    const step = (100 * speed) / totalMs;
    const interval = setInterval(() => {
      setPlaybackPos((p) => {
        const next = p + step;
        if (next >= 1) { setPlaying(false); return 1; }
        return next;
      });
    }, 100);
    return () => clearInterval(interval);
  }, [playing, speed, selected]);

  const addBookmark = () => {
    if (!bookmarks.includes(playbackPos)) {
      setBookmarks([...bookmarks, playbackPos].sort());
    }
  };

  const jumpTo = (frac: number) => setPlaybackPos(Math.max(0, Math.min(1, frac)));

  return (
    <div className="timeline-view">
      <div className="audit-header">
        <div>
          <h2>セッション履歴 (Time Travel)</h2>
          <p className="audit-subtitle">過去のセッションを巻き戻し / 早送りで再生</p>
        </div>
        <button className="button-ghost" onClick={onClose}>閉じる</button>
      </div>

      {recordings.length === 0 ? (
        <div className="empty-state">
          まだ録画がありません。セッション中に「録画」ボタンで作成。
        </div>
      ) : (
        <div className="timeline-layout">
          <div className="recording-list">
            {recordings.map((r) => (
              <button
                key={r.path}
                className={`recording-item ${selected?.path === r.path ? "selected" : ""}`}
                onClick={() => {
                  setSelected(r);
                  setPlaybackPos(0);
                  setPlaying(false);
                  setBookmarks([]);
                  setFrameB64(null);
                }}
              >
                <div className="recording-name">{new Date(r.start_ts_ms).toLocaleString()}</div>
                <div className="recording-meta">
                  {(r.size_bytes / 1024 / 1024).toFixed(1)} MB
                  {r.frame_count > 0 && ` · ${r.frame_count.toLocaleString()} フレーム`}
                </div>
              </button>
            ))}
          </div>

          <div className="timeline-main">
            {selected ? (
              <>
                <div className="preview-area">
                  {frameB64 ? (
                    <img
                      src={`data:image/jpeg;base64,${frameB64}`}
                      alt="録画フレーム"
                      style={{ width: "100%", height: "100%", objectFit: "contain" }}
                    />
                  ) : (
                    <div className="preview-placeholder">
                      <div className="preview-text">
                        フレーム {Math.floor(playbackPos * (selected.frame_count || 1))}
                        <br />
                        <span className="dim">
                          @ {formatTime(playbackPos * (selected.duration_ms || 60000))}
                        </span>
                      </div>
                    </div>
                  )}
                </div>

                <div className="scrubber">
                  <div
                    className="scrubber-track"
                    onClick={(e) => {
                      const rect = (e.target as HTMLElement).getBoundingClientRect();
                      jumpTo((e.clientX - rect.left) / rect.width);
                    }}
                  >
                    {bookmarks.map((b, i) => (
                      <div key={i} className="bookmark" style={{ left: `${b * 100}%` }} />
                    ))}
                    <div className="scrubber-fill" style={{ width: `${playbackPos * 100}%` }} />
                    <div className="scrubber-thumb" style={{ left: `${playbackPos * 100}%` }} />
                  </div>
                  <div className="scrubber-times">
                    <span>{formatTime(playbackPos * (selected.duration_ms || 60000))}</span>
                    <span>{formatTime(selected.duration_ms || 60000)}</span>
                  </div>
                </div>

                <div className="timeline-controls">
                  <button
                    className="button-ghost"
                    onClick={() => jumpTo(Math.max(0, playbackPos - 0.05))}
                    title="-5%"
                  >
                    ◁
                  </button>
                  <button className="button-primary" onClick={() => setPlaying(!playing)}>
                    {playing ? "一時停止" : "再生"}
                  </button>
                  <button
                    className="button-ghost"
                    onClick={() => jumpTo(Math.min(1, playbackPos + 0.05))}
                    title="+5%"
                  >
                    ▷
                  </button>

                  <div className="speed-selector">
                    {[0.25, 0.5, 1, 2, 4, 8].map((s) => (
                      <button
                        key={s}
                        className={speed === s ? "active" : ""}
                        onClick={() => setSpeed(s)}
                      >
                        {s}x
                      </button>
                    ))}
                  </div>

                  <button className="button-ghost" onClick={addBookmark}>ブックマーク</button>
                </div>
              </>
            ) : (
              <div className="empty-state">左から録画を選択</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function formatTime(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  const hh = String(Math.floor(totalSec / 3600)).padStart(2, "0");
  const mm = String(Math.floor((totalSec % 3600) / 60)).padStart(2, "0");
  const ss = String(totalSec % 60).padStart(2, "0");
  return totalSec >= 3600 ? `${hh}:${mm}:${ss}` : `${mm}:${ss}`;
}
