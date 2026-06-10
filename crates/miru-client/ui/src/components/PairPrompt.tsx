/**
 * Pair prompt — shown when an unknown peer requests connection.
 * Displays the peer's fingerprint and asks the host user to verify the PIN.
 */
import { useState } from "react";

interface Props {
  deviceId: string;
  fingerprint: string;
  onAccept: (permission: "view_only" | "control" | "full", remember: boolean) => void;
  onReject: () => void;
}

export function PairPrompt({ deviceId, fingerprint, onAccept, onReject }: Props) {
  const [permission, setPermission] = useState<"view_only" | "control" | "full">("control");
  const [remember, setRemember] = useState(true);

  return (
    <div className="modal-overlay">
      <div className="modal-card">
        <h2>接続要求</h2>
        <p className="subtitle">未確認のデバイスから接続要求があります</p>

        <div className="pair-info">
          <div className="pair-row">
            <span className="label">デバイス ID</span>
            <span className="value mono">{deviceId}</span>
          </div>
          <div className="pair-row">
            <span className="label">指紋</span>
            <span className="value mono small">{fingerprint}</span>
          </div>
        </div>

        <div className="warning-box">
          指紋が相手の画面に表示されているものと一致するか確認してください。
          一致しない場合は「拒否」を選んでください。
        </div>

        <div className="field">
          <label>権限</label>
          <div className="radio-group">
            <label className="radio-row">
              <input
                type="radio"
                checked={permission === "view_only"}
                onChange={() => setPermission("view_only")}
              />
              <div>
                <div className="radio-title">閲覧のみ</div>
                <div className="radio-desc">画面を見るだけ。操作不可</div>
              </div>
            </label>
            <label className="radio-row">
              <input
                type="radio"
                checked={permission === "control"}
                onChange={() => setPermission("control")}
              />
              <div>
                <div className="radio-title">操作可能</div>
                <div className="radio-desc">マウス・キーボード操作を許可</div>
              </div>
            </label>
            <label className="radio-row">
              <input
                type="radio"
                checked={permission === "full"}
                onChange={() => setPermission("full")}
              />
              <div>
                <div className="radio-title">フルアクセス</div>
                <div className="radio-desc">操作 + クリップボード共有 + ファイル転送</div>
              </div>
            </label>
          </div>
        </div>

        <label className="checkbox-row">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
          />
          <span>このデバイスを記憶する（次回から自動承認）</span>
        </label>

        <div className="modal-actions">
          <button className="button-ghost" onClick={onReject}>
            拒否
          </button>
          <button
            className="button-primary"
            onClick={() => onAccept(permission, remember)}
          >
            許可
          </button>
        </div>
      </div>
    </div>
  );
}
