import { useEffect, useState } from "react";

interface Props {
  /** 8-byte hex fingerprint of the remote host (e.g. "AB:CD:12:34:56:78:9A:BC"). */
  fingerprint: string;
  /** Called with the entered 6-digit PIN. */
  onConfirm: (pin: string) => void;
  onCancel: () => void;
}

export function PairingDialog({ fingerprint, onConfirm, onCancel }: Props) {
  const [pin, setPin] = useState("");
  const [autoFocus] = useState(true);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const valid = /^[0-9]{6}$/.test(pin);

  return (
    <div className="dialog-backdrop">
      <div className="dialog">
        <h2>新しいデバイスに接続</h2>
        <p className="dialog-subtitle">
          相手のデバイスに表示されている指紋とPINを照合してください。
        </p>

        <div className="fpr-display">
          <span className="fpr-label">指紋</span>
          <code className="fpr-value">{fingerprint}</code>
        </div>

        <div className="pin-input">
          <label>PIN（6桁）</label>
          <input
            type="text"
            inputMode="numeric"
            pattern="[0-9]*"
            maxLength={6}
            placeholder="······"
            value={pin}
            onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))}
            autoFocus={autoFocus}
          />
        </div>

        <div className="dialog-warning">
          指紋が一致しない場合は接続を拒否してください。中間者攻撃の可能性があります。
        </div>

        <div className="dialog-actions">
          <button className="button-ghost" onClick={onCancel}>キャンセル</button>
          <button
            className="button-primary"
            disabled={!valid}
            onClick={() => onConfirm(pin)}
          >
            ペアリング
          </button>
        </div>
      </div>
    </div>
  );
}
