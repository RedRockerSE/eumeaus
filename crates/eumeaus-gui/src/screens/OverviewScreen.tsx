import { useEffect, useState } from "react";
import type { AuditEvent, CaseInfo, CaseStats } from "../api";
import { auditList, caseStats } from "../api";

function fmtTime(ms: number): string {
  const d = new Date(ms);
  return d.toISOString().slice(0, 16).replace("T", " ");
}

export default function OverviewScreen({ current }: { current: CaseInfo }) {
  const [stats, setStats] = useState<CaseStats | null>(null);
  const [audit, setAudit] = useState<AuditEvent[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([caseStats(), auditList(20)])
      .then(([s, a]) => {
        setStats(s);
        setAudit(a);
      })
      .catch((e) => setError(String(e)));
  }, [current.id]);

  return (
    <div style={{ flex: 1, overflow: "auto", padding: "26px 30px" }}>
      <div style={{ maxWidth: 900 }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: 12, marginBottom: 4 }}>
          <h1 style={{ margin: 0, fontSize: 22, fontWeight: 600, letterSpacing: "-0.3px" }}>
            {current.name}
          </h1>
          <span className="mono" style={{ fontSize: 11, color: "var(--text-mono-muted)" }}>
            {current.id.slice(0, 8)}
          </span>
        </div>
        <div className="mono" style={{ fontSize: 11.5, color: "var(--text-fainter)", marginBottom: 24 }}>
          {current.path}
        </div>

        {error && <p className="error-text">{error}</p>}

        <div style={{ display: "grid", gridTemplateColumns: "repeat(4, 1fr)", gap: 10, marginBottom: 26 }}>
          <div className="stat-card">
            <div className="stat-value">{stats?.entity_count ?? "—"}</div>
            <div className="stat-label">Entities</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">{stats?.fact_count ?? "—"}</div>
            <div className="stat-label">Facts</div>
          </div>
          <div className="stat-card">
            <div className="stat-value">{stats?.relationship_count ?? "—"}</div>
            <div className="stat-label">Relationships</div>
          </div>
          <div className={"stat-card" + (stats && stats.conflicting_entity_count > 0 ? " warn" : "")}>
            <div className="stat-value">{stats?.conflicting_entity_count ?? "—"}</div>
            <div className="stat-label">Conflicts</div>
          </div>
        </div>

        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10 }}>
          <div className="card">
            <div className="field-label" style={{ marginBottom: 11 }}>
              Storage
            </div>
            <div className="col">
              <div style={{ display: "flex", justifyContent: "space-between", gap: 12 }}>
                <span className="muted">Encryption</span>
                <span style={{ display: "flex", alignItems: "center", gap: 6, color: "var(--ok)" }}>
                  <span className="statusbar-dot" style={{ marginRight: 0 }} />
                  SQLCipher, key in OS keychain
                </span>
              </div>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 12 }}>
                <span className="muted">Case id</span>
                <span className="mono" style={{ fontSize: 12 }}>
                  {current.id}
                </span>
              </div>
            </div>
          </div>
          <div className="card">
            <div className="field-label" style={{ marginBottom: 11 }}>
              Audit trail
            </div>
            <div className="col">
              {audit && audit.length === 0 && <p className="muted">No audit events yet.</p>}
              {audit?.map((a) => (
                <div key={a.id} style={{ display: "flex", gap: 10, alignItems: "baseline" }}>
                  <span className="mono" style={{ fontSize: 11, color: "var(--text-mono-faint)", flex: "none" }}>
                    {fmtTime(a.occurred_at_unix_ms)}
                  </span>
                  <span style={{ color: "var(--text-dimmer)", fontSize: 12 }}>{a.description}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
