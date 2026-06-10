import { useState } from "react";

interface Props {
  fingerprint: string;
  onComplete: () => void;
}

type Step = "welcome" | "fingerprint" | "trust_axes" | "ready";

const STORAGE_KEY = "miru.onboarded";

export function OnboardingWizard({ fingerprint, onComplete }: Props) {
  const [step, setStep] = useState<Step>("welcome");

  const finish = () => {
    localStorage.setItem(STORAGE_KEY, "1");
    onComplete();
  };

  return (
    <div className="onboarding">
      <div className="onboarding-card">
        <div className="onboarding-progress">
          {(["welcome", "fingerprint", "trust_axes", "ready"] as Step[]).map((s, i) => (
            <div
              key={s}
              className={`progress-dot ${
                step === s ? "active" : isStepDone(step, s) ? "done" : ""
              }`}
            />
          ))}
        </div>

        {step === "welcome" && (
          <>
            <div className="welcome-mark" />
            <h1>Miru へようこそ</h1>
            <p className="onboarding-lead">
              リモートデスクトップ + AI エージェント基盤 + 検証可能な E2E。
              <br />
              30秒でセットアップを完了します。
            </p>
            <button className="button-primary onboarding-btn" onClick={() => setStep("fingerprint")}>
              はじめる
            </button>
          </>
        )}

        {step === "fingerprint" && (
          <>
            <div className="step-icon">1</div>
            <h2>あなたのデバイス指紋</h2>
            <p className="onboarding-lead">
              この端末を一意に識別する暗号学的指紋です。<br />
              他デバイスから接続するとき、相手側でこれと同じ指紋が表示されます。
            </p>
            <div className="fingerprint-display">
              <code>{fingerprint || "計算中..."}</code>
            </div>
            <p className="hint">
              ※ ペアリング時に必ずこの指紋を視覚で照合してください。
              一致しない場合は中間者攻撃の可能性があります。
            </p>
            <div className="onboarding-actions">
              <button className="button-ghost" onClick={() => setStep("welcome")}>戻る</button>
              <button className="button-primary" onClick={() => setStep("trust_axes")}>次へ</button>
            </div>
          </>
        )}

        {step === "trust_axes" && (
          <>
            <div className="step-icon">2</div>
            <h2>4つの独自軸</h2>
            <p className="onboarding-lead">
              他のリモートデスクトップソフトにはない機能です。
            </p>
            <div className="axes-list">
              <div className="axis-row">
                <strong>Constellation</strong>
                <span>全デバイスをひとつの fabric として可視化</span>
              </div>
              <div className="axis-row">
                <strong>AI 許可</strong>
                <span>Claude などの AI に時限式で安全に PC 制御を許可</span>
              </div>
              <div className="axis-row">
                <strong>Audit ログ</strong>
                <span>SHA-256 chain で改ざん検知、すべての操作を記録</span>
              </div>
              <div className="axis-row">
                <strong>Time Travel</strong>
                <span>過去のセッションを巻き戻し / 早送り再生</span>
              </div>
            </div>
            <div className="onboarding-actions">
              <button className="button-ghost" onClick={() => setStep("fingerprint")}>戻る</button>
              <button className="button-primary" onClick={() => setStep("ready")}>次へ</button>
            </div>
          </>
        )}

        {step === "ready" && (
          <>
            <div className="step-icon ok">✓</div>
            <h2>準備完了</h2>
            <p className="onboarding-lead">
              さっそく接続してみましょう。
              上部メニューから「Constellation」「AI 許可」「Audit」「Timeline」も
              いつでも切り替えられます。
            </p>
            <div className="quickstart">
              <h3>次のステップ</h3>
              <ul>
                <li>別の PC で Miru ホストを起動 → デバイス ID を確認</li>
                <li>そのデバイス ID で接続 → 指紋照合 → ペアリング</li>
                <li>または「AI 許可」から Claude 用トークン発行</li>
              </ul>
            </div>
            <button className="button-primary onboarding-btn" onClick={finish}>
              はじめる
            </button>
          </>
        )}

        <button className="onboarding-skip" onClick={finish}>スキップ</button>
      </div>
    </div>
  );
}

function isStepDone(current: Step, target: Step): boolean {
  const order: Step[] = ["welcome", "fingerprint", "trust_axes", "ready"];
  return order.indexOf(current) > order.indexOf(target);
}

/** Returns whether onboarding has already been seen by this user. */
export function isOnboarded(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}
