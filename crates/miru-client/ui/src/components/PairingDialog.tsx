import { useEffect, useState } from "react";

interface Props {
  /** 8-byte hex fingerprint of the remote host (e.g. "AB:CD:12:34:56:78:9A:BC"). */
  fingerprint: string;
  /**
   * Called on confirm. `pin` is the entered 6-digit PIN when `requirePin` is
   * true, or `undefined` when this dialog is used purely as a fingerprint
   * confirmation (TOFU) gate with no PIN step.
   */
  onConfirm: (pin?: string) => void;
  onCancel: () => void;
  /**
   * Show the PIN entry field and require it before enabling confirm.
   * Defaults to true (original pairing-PIN flow). Set false for a
   * fingerprint-only confirmation, e.g. the first-connection TOFU gate,
   * where no PIN is being verified and showing the field would misleadingly
   * imply otherwise.
   */
  requirePin?: boolean;
}

export function PairingDialog({ fingerprint, onConfirm, onCancel, requirePin = true }: Props) {
  const [pin, setPin] = useState("");
  const [autoFocus] = useState(true);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const valid = !requirePin || /^[0-9]{6}$/.test(pin);

  return (
    <div className="dialog-backdrop">
      <div className="dialog">
        <h2>新しいデバイスに接続</h2>
        <p className="dialog-subtitle">
          {requirePin
            ? "相手のデバイスに表示されている指紋とPINを照合してください。"
            : "初めて接続するホストです。相手のデバイスに表示されている指紋と一致するか確認してください。"}
        </p>

        <div className="fpr-display">
          <span className="fpr-label">指紋</span>
          <code className="fpr-value">{fingerprint}</code>
        </div>

        {requirePin && (
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
        )}

        <div className="dialog-warning">
          指紋が一致しない場合は接続を拒否してください。中間者攻撃の可能性があります。
        </div>

        <div className="dialog-actions">
          <button className="button-ghost" onClick={onCancel}>キャンセル</button>
          <button
            className="button-primary"
            disabled={!valid}
            onClick={() => onConfirm(requirePin ? pin : undefined)}
          >
            ペアリング
          </button>
        </div>
      </div>
    </div>
  );
}
