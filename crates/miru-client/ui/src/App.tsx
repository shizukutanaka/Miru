import { useEffect, useState } from "react";
import { ConnectScreen } from "./components/ConnectScreen";
import { SessionScreen } from "./components/SessionScreen";
import { AgentTokenIssue } from "./components/AgentTokenIssue";
import { AuditLogViewer } from "./components/AuditLogViewer";
import { ConstellationMap } from "./components/ConstellationMap";
import { TimelineScrubber } from "./components/TimelineScrubber";
import { OnboardingWizard, isOnboarded } from "./components/OnboardingWizard";
import { api } from "./lib/tauri";
import { initLocale, t } from "./lib/i18n";

type View =
  | { kind: "onboarding" }
  | { kind: "connect" }
  | { kind: "session" }
  | { kind: "agent_issue" }
  | { kind: "audit" }
  | { kind: "constellation" }
  | { kind: "timeline" };

export default function App() {
  const [view, setView] = useState<View>(
    isOnboarded() ? { kind: "connect" } : { kind: "onboarding" }
  );
  const [fingerprint, setFingerprint] = useState("");

  useEffect(() => {
    initLocale();
    api.fingerprint().then(setFingerprint).catch(() => {});
  }, []);

  // Onboarding stays separate from the chrome
  if (view.kind === "onboarding") {
    return (
      <div className="app">
        <OnboardingWizard
          fingerprint={fingerprint}
          onComplete={() => setView({ kind: "connect" })}
        />
      </div>
    );
  }

  return (
    <div className="app">
      <div className="titlebar">
        <div className="logo">
          <div className="logo-mark" />
          <span>Miru</span>
        </div>

        <nav className="titlebar-nav">
          <button
            className={view.kind === "connect" || view.kind === "session" ? "active" : ""}
            onClick={() => setView({ kind: "connect" })}
          >
            {t("nav.connect")}
          </button>
          <button
            className={view.kind === "constellation" ? "active" : ""}
            onClick={() => setView({ kind: "constellation" })}
          >
            {t("nav.constellation")}
          </button>
          <button
            className={view.kind === "agent_issue" ? "active" : ""}
            onClick={() => setView({ kind: "agent_issue" })}
          >
            {t("nav.ai_authorize")}
          </button>
          <button
            className={view.kind === "audit" ? "active" : ""}
            onClick={() => setView({ kind: "audit" })}
          >
            {t("nav.audit")}
          </button>
          <button
            className={view.kind === "timeline" ? "active" : ""}
            onClick={() => setView({ kind: "timeline" })}
          >
            {t("nav.timeline")}
          </button>
        </nav>

        <div className="fpr" title={t("titlebar.fingerprint_tooltip")}>
          {fingerprint}
        </div>
      </div>

      {view.kind === "connect" && (
        <ConnectScreen onConnect={() => setView({ kind: "session" })} />
      )}
      {view.kind === "session" && (
        <SessionScreen onDisconnect={() => setView({ kind: "connect" })} />
      )}
      {view.kind === "agent_issue" && (
        <AgentTokenIssue
          onIssued={() => setView({ kind: "audit" })}
          onCancel={() => setView({ kind: "connect" })}
        />
      )}
      {view.kind === "audit" && (
        <AuditLogViewer onClose={() => setView({ kind: "connect" })} />
      )}
      {view.kind === "constellation" && (
        <ConstellationMap
          onConnect={() => setView({ kind: "connect" })}
          onClose={() => setView({ kind: "connect" })}
        />
      )}
      {view.kind === "timeline" && (
        <TimelineScrubber onClose={() => setView({ kind: "connect" })} />
      )}
    </div>
  );
}
