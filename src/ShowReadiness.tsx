import { buildReadinessReport, type ReadinessCheck } from "./readinessChecks";
import { useGate } from "./state";

const ICON: Record<ReadinessCheck["level"], string> = {
  pass: "✓",
  warn: "!",
  fail: "×",
  info: "·",
};

function actionLabel(action: NonNullable<ReadinessCheck["action"]>): string {
  if (action === "control") return "Open Control";
  if (action === "firewall") return "Authorize firewall";
  if (action === "updates") return "Open Updates";
  return "Open Settings";
}

export default function ShowReadiness() {
  const { client, config, status, connected } = useGate();

  if (!config || !status) {
    return (
      <div className="readiness-page">
        <section className="readiness-hero blocked">
          <div>
            <p className="readiness-kicker">Live preflight</p>
            <h2>Waiting for Gate status</h2>
            <p>No readiness claim can be made until configuration and live telemetry arrive.</p>
          </div>
        </section>
      </div>
    );
  }

  const report = buildReadinessReport(config, status, connected);
  const headline = report.state === "ready"
    ? "Ready for show"
    : report.state === "standby"
      ? "Ready for a dry run"
      : report.state === "blocked"
        ? "Show start blocked"
        : "Review before show";
  const subhead = report.state === "standby"
    ? "Core checks pass. sACN is off, so packet delivery is not yet proven."
    : report.state === "ready"
      ? "Live output and core runtime checks currently pass."
      : `${report.failures} failed · ${report.warnings} warning${report.warnings === 1 ? "" : "s"}`;

  const runAction = (action: NonNullable<ReadinessCheck["action"]>) => {
    if (action === "firewall") {
      client.authorizeFirewall();
      return;
    }
    location.hash = action === "control" ? "control" : "settings";
  };

  return (
    <div className="readiness-page">
      <section className={`readiness-hero ${report.state}`}>
        <div>
          <p className="readiness-kicker">Live preflight</p>
          <h2>{headline}</h2>
          <p>{subhead}</p>
        </div>
        <div className="readiness-counts" aria-label={`${report.failures} failures and ${report.warnings} warnings`}>
          <strong>{report.checks.filter((check) => check.level === "pass").length}</strong>
          <span>passing</span>
        </div>
      </section>

      <p className="readiness-caveat">
        This panel reflects live Gate telemetry and saved configuration. It cannot verify receiver power,
        physical pixels, audio cabling, or the lighting network beyond packets leaving this machine.
      </p>

      <div className="readiness-grid">
        {report.checks.map((check) => (
          <article key={check.id} className={`readiness-check ${check.level}`}>
            <span className="readiness-icon" aria-hidden="true">{ICON[check.level]}</span>
            <div className="readiness-copy">
              <div className="readiness-check-head">
                <h3>{check.label}</h3>
                <span className={`readiness-badge ${check.level}`}>{check.level}</span>
              </div>
              <p>{check.summary}</p>
              {check.detail && <small>{check.detail}</small>}
              {check.action && (
                <button className="ghost readiness-action" onClick={() => runAction(check.action!)}>
                  {actionLabel(check.action)} →
                </button>
              )}
            </div>
          </article>
        ))}
      </div>
      <p className="readiness-live-note">Updates automatically as Gate status changes.</p>
    </div>
  );
}
