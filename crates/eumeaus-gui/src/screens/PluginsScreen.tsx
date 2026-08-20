import { useState } from "react";
import type { PluginSummary } from "../api";
import { pluginInstall, pluginList, pluginVerify } from "../api";

export default function PluginsScreen() {
  const [pluginsDir, setPluginsDir] = useState("");
  const [plugins, setPlugins] = useState<PluginSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [installOpen, setInstallOpen] = useState(false);
  const [sourcePath, setSourcePath] = useState("");
  const [verifyTrust, setVerifyTrust] = useState<Record<string, string>>({});
  const [verifyResult, setVerifyResult] = useState<Record<string, string>>({});

  async function refresh(e?: React.FormEvent) {
    e?.preventDefault();
    setError(null);
    try {
      setPlugins(await pluginList(pluginsDir));
    } catch (e) {
      setError(String(e));
    }
  }

  async function install(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await pluginInstall(sourcePath, pluginsDir);
      setSourcePath("");
      setInstallOpen(false);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function verify(name: string) {
    setError(null);
    setVerifyResult((r) => ({ ...r, [name]: "" }));
    try {
      const trust = verifyTrust[name];
      await pluginVerify(name, pluginsDir, null, trust || null);
      setVerifyResult((r) => ({ ...r, [name]: "valid" }));
    } catch (e) {
      setVerifyResult((r) => ({ ...r, [name]: String(e) }));
    }
  }

  return (
    <div style={{ flex: 1, overflow: "auto", padding: "22px 26px" }}>
      <div style={{ maxWidth: 980 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: 6 }}>
          <h1 style={{ margin: 0, fontSize: 20, fontWeight: 600, letterSpacing: "-0.3px" }}>Plugins</h1>
          <button className="btn" style={{ marginLeft: "auto" }} onClick={() => setInstallOpen((v) => !v)}>
            Install plugin…
          </button>
        </div>

        <form className="row" style={{ marginBottom: 16 }} onSubmit={refresh}>
          <input
            className="mono"
            style={{ flex: 1 }}
            value={pluginsDir}
            onChange={(e) => setPluginsDir(e.currentTarget.value)}
            placeholder="Plugins directory"
          />
          <button className="btn" type="submit">
            List
          </button>
        </form>

        {installOpen && (
          <form className="row" style={{ marginBottom: 16 }} onSubmit={install}>
            <input
              className="mono"
              style={{ flex: 1 }}
              value={sourcePath}
              onChange={(e) => setSourcePath(e.currentTarget.value)}
              placeholder="Source directory (contains plugin.toml)"
            />
            <button className="btn btn-primary" type="submit">
              Install
            </button>
          </form>
        )}

        {error && <p className="error-text">{error}</p>}

        <div className="table">
          <div className="table-head" style={{ gridTemplateColumns: "1fr 80px 200px 140px 160px" }}>
            <div>Plugin</div>
            <div>Version</div>
            <div>Signature</div>
            <div>Handles</div>
            <div>Verify</div>
          </div>
          {plugins?.map((p) => (
            <div key={p.name} className="table-row" style={{ gridTemplateColumns: "1fr 80px 200px 140px 160px" }}>
              <div style={{ minWidth: 0 }}>
                <div className="mono" style={{ fontSize: 12.5, color: "var(--text)", marginBottom: 2 }}>
                  {p.name}
                </div>
                <div style={{ fontSize: 11.5, color: "var(--text-fainter)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {p.description}
                </div>
              </div>
              <div className="mono" style={{ fontSize: 11.5, color: "var(--text-dimmer)" }}>
                {p.version}
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 7, minWidth: 0 }}>
                <span style={{ width: 6, height: 6, borderRadius: "50%", flex: "none", background: p.signed ? "var(--ok)" : "var(--danger)" }} />
                <span style={{ fontSize: 11.5, color: p.signed ? "var(--ok)" : "var(--danger)" }}>
                  {p.signed ? "signed" : "unsigned"}
                </span>
              </div>
              <div style={{ fontSize: 11.5, color: "var(--text-faint)" }}>{p.input_entity_types.join(", ")}</div>
              <div className="col" style={{ gap: 4 }}>
                <div className="row">
                  <input
                    className="mono"
                    style={{ flex: 1, padding: "3px 6px", fontSize: 10.5 }}
                    placeholder="trust name"
                    value={verifyTrust[p.name] ?? ""}
                    onChange={(e) => setVerifyTrust((v) => ({ ...v, [p.name]: e.currentTarget.value }))}
                  />
                  <button className="btn btn-small" onClick={() => verify(p.name)}>
                    Verify
                  </button>
                </div>
                {verifyResult[p.name] && (
                  <span style={{ fontSize: 10.5, color: verifyResult[p.name] === "valid" ? "var(--ok)" : "var(--danger)" }}>
                    {verifyResult[p.name]}
                  </span>
                )}
              </div>
            </div>
          ))}
          {plugins && plugins.length === 0 && <p className="muted" style={{ padding: 12 }}>No plugins found in that directory.</p>}
        </div>
        <div style={{ marginTop: 12, fontSize: 11.5, color: "var(--text-mono-muted)", lineHeight: 1.6, maxWidth: "70ch" }}>
          Unsigned plugins run, but nothing vouches for them. Add the author&rsquo;s public key under
          Settings › Trust store to have the signature checked before every spawn.
        </div>
      </div>
    </div>
  );
}
