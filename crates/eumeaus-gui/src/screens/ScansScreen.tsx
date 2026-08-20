import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { PluginSummary, ScanProgress, ScanSummary } from "../api";
import { pluginList, scanList, scanRun } from "../api";

export default function ScansScreen() {
  const [pluginsDir, setPluginsDir] = useState("");
  const [plugins, setPlugins] = useState<PluginSummary[] | null>(null);
  const [pluginChecked, setPluginChecked] = useState<Set<string>>(new Set());
  const [targetType, setTargetType] = useState("Username");
  const [targetValue, setTargetValue] = useState("");
  const [activeScanId, setActiveScanId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ScanProgress[]>([]);
  const [history, setHistory] = useState<ScanSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const activeScanIdRef = useRef<string | null>(null);

  useEffect(() => {
    const unlisten = listen<ScanProgress>("scan-progress", (event) => {
      if (event.payload.scan_id !== activeScanIdRef.current) return;
      setProgress((prev) => [...prev, event.payload]);
      if (event.payload.status === "SUCCESS" || event.payload.status === "ERROR" || event.payload.status === "TIMEOUT") {
        refreshHistory();
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    refreshHistory();
  }, []);

  async function refreshHistory() {
    try {
      setHistory(await scanList());
    } catch (e) {
      setError(String(e));
    }
  }

  async function loadPlugins(e?: React.FormEvent) {
    e?.preventDefault();
    setError(null);
    try {
      setPlugins(await pluginList(pluginsDir));
    } catch (e) {
      setError(String(e));
    }
  }

  function togglePlugin(name: string) {
    setPluginChecked((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }

  async function run(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setProgress([]);
    if (!pluginsDir.trim() || !targetType.trim() || !targetValue.trim()) {
      setError("Plugins directory, target type, and target key are all required.");
      return;
    }
    try {
      const scanId = await scanRun(pluginsDir, Array.from(pluginChecked), targetType, targetValue);
      activeScanIdRef.current = scanId;
      setActiveScanId(scanId);
    } catch (e) {
      setError(String(e));
    }
  }

  const running = progress.length > 0 && progress.some((p) => p.status === "RUNNING") && !progress.some((p) => p.status !== "RUNNING" && p.status !== "SUCCESS");

  return (
    <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
      <div style={{ width: 330, flex: "none", borderRight: "1px solid var(--border-subtle)", padding: 18, display: "flex", flexDirection: "column", gap: 14, overflow: "auto" }}>
        <div className="field-label">New scan</div>
        {error && <p className="error-text">{error}</p>}

        <div className="col">
          <label className="field-label-sm">Target</label>
          <div className="row">
            <select style={{ width: 130 }} value={targetType} onChange={(e) => setTargetType(e.currentTarget.value)}>
              <option>Username</option>
              <option>Email</option>
              <option>Domain</option>
              <option>PhoneNumber</option>
            </select>
            <input className="mono" style={{ flex: 1, minWidth: 0 }} value={targetValue} onChange={(e) => setTargetValue(e.currentTarget.value)} />
          </div>
        </div>

        <div className="col">
          <label className="field-label-sm">Plugins directory</label>
          <form className="row" onSubmit={loadPlugins}>
            <input className="mono" style={{ flex: 1 }} value={pluginsDir} onChange={(e) => setPluginsDir(e.currentTarget.value)} />
            <button className="btn btn-small" type="submit">
              List
            </button>
          </form>
        </div>

        <div className="col">
          <label className="field-label-sm">Plugins (blank selection = all compatible)</label>
          <div style={{ display: "flex", flexDirection: "column", gap: 4, padding: 6, background: "var(--bg-panel)", border: "1px solid #2c2c36", borderRadius: 5 }}>
            {plugins?.map((p) => (
              <div
                key={p.name}
                onClick={() => togglePlugin(p.name)}
                style={{ display: "flex", alignItems: "center", gap: 9, padding: "6px 7px", borderRadius: 4, cursor: "pointer" }}
              >
                <input type="checkbox" checked={pluginChecked.has(p.name)} onChange={() => togglePlugin(p.name)} />
                <span className="mono" style={{ fontSize: 11.5, color: "var(--text-dim)", flex: 1 }}>
                  {p.name}
                </span>
              </div>
            ))}
            {plugins && plugins.length === 0 && <p className="muted" style={{ margin: 0, padding: 6 }}>No plugins found.</p>}
          </div>
        </div>

        <button className="btn btn-primary" onClick={run}>
          Run scan
        </button>
        <div style={{ fontSize: 11.5, color: "var(--text-mono-muted)", lineHeight: 1.5 }}>
          Results merge into the case automatically. A scan can be resumed if interrupted.
        </div>
      </div>

      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", minHeight: 0 }}>
        <div style={{ padding: "16px 22px 12px", borderBottom: "1px solid var(--border-subtle)", display: "flex", alignItems: "center", gap: 12 }}>
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 600 }}>
            {activeScanId ? "Scan " + activeScanId.slice(0, 8) : "No active scan"}
          </h2>
          {activeScanId && (
            <span
              className="status-pill"
              style={{
                background: running ? "rgba(139,124,232,0.14)" : "rgba(111,185,138,0.13)",
                color: running ? "var(--accent-mid)" : "var(--ok)",
                borderColor: running ? "var(--border-accent-subtle)" : "rgba(111,185,138,0.3)",
              }}
            >
              {running ? "running" : "done"}
            </span>
          )}
        </div>
        <div style={{ flex: 1, overflow: "auto", padding: "16px 22px" }}>
          <div className="col" style={{ marginBottom: 26 }}>
            {progress.map((p, i) => {
              const color = p.status === "SUCCESS" ? "var(--ok)" : p.status === "RUNNING" ? "var(--accent)" : p.status === "TIMEOUT" || p.status === "ERROR" ? "var(--warn)" : "var(--text-mono-faint)";
              return (
                <div key={i} className="card" style={{ display: "flex", alignItems: "center", gap: 13 }}>
                  <span style={{ width: 7, height: 7, borderRadius: "50%", flex: "none", background: color }} />
                  <span className="mono" style={{ fontSize: 12, color: "var(--text)", width: 180, flex: "none" }}>
                    {p.plugin_name}
                  </span>
                  <span style={{ flex: 1, fontSize: 11.5, color: "var(--text-faint)" }}>
                    {p.error_message ?? ""}
                  </span>
                  <span className="status-pill" style={{ color, background: "#22222b", flex: "none" }}>
                    {p.status}
                  </span>
                </div>
              );
            })}
            {activeScanId && progress.length === 0 && <p className="muted">Waiting for progress…</p>}
          </div>

          <div className="field-label" style={{ marginBottom: 10 }}>
            Scan history
          </div>
          <div className="table">
            <div className="table-head" style={{ gridTemplateColumns: "180px 1fr 160px 90px" }}>
              <div>Id</div>
              <div>Target entity</div>
              <div>When</div>
              <div>Status</div>
            </div>
            {history?.map((s) => (
              <div key={s.id} className="table-row" style={{ gridTemplateColumns: "180px 1fr 160px 90px" }}>
                <div className="mono" style={{ fontSize: 11, color: "var(--text-fainter)" }}>
                  {s.id.slice(0, 8)}
                </div>
                <div className="mono" style={{ fontSize: 11.5, color: "var(--text-dim)" }}>
                  {s.target_entity_id.slice(0, 8)}
                </div>
                <div className="mono" style={{ fontSize: 11, color: "var(--text-mono-muted)" }}>
                  {s.started_at_unix_ms ? new Date(s.started_at_unix_ms).toISOString().slice(0, 16).replace("T", " ") : "-"}
                </div>
                <div style={{ fontSize: 11, fontWeight: 600, textTransform: "uppercase", color: s.status === "COMPLETED" ? "var(--ok)" : s.status === "PARTIALLY_FAILED" ? "var(--warn)" : "var(--info)" }}>
                  {s.status}
                </div>
              </div>
            ))}
            {history && history.length === 0 && <p className="muted" style={{ padding: 12 }}>No scans yet.</p>}
          </div>
        </div>
      </div>
    </div>
  );
}
