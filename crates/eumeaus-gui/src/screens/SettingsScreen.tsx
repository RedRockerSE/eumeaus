import { useEffect, useState } from "react";
import { check as checkForUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import type { TrustedKey } from "../api";
import { credentialList, credentialRemove, credentialSet, trustAdd, trustList, trustRemove } from "../api";

type SettingsTab = "creds" | "trust" | "updates";

function CredentialsPane() {
  const [names, setNames] = useState<string[] | null>(null);
  const [newName, setNewName] = useState("");
  const [newValue, setNewValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    setError(null);
    try {
      setNames(await credentialList());
    } catch (e) {
      setError(String(e));
    }
  }
  useEffect(() => {
    refresh();
  }, []);

  async function set(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await credentialSet(newName, newValue);
      setNewName("");
      setNewValue("");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove(name: string) {
    setError(null);
    try {
      await credentialRemove(name);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <h1 style={{ margin: "0 0 6px", fontSize: 20, fontWeight: 600, letterSpacing: "-0.3px" }}>Credentials</h1>
      <p style={{ margin: "0 0 20px", color: "var(--text-faint)", fontSize: 12.5, lineHeight: 1.6, maxWidth: "62ch" }}>
        API keys live in the OS keychain, and are handed to a plugin only inside its request. They
        never touch the case file, the command line, or the environment.
      </p>
      {error && <p className="error-text">{error}</p>}
      <div className="table" style={{ marginBottom: 16 }}>
        {names?.map((n) => (
          <div key={n} className="table-row" style={{ gridTemplateColumns: "1fr 130px 90px", alignItems: "center" }}>
            <span className="mono" style={{ fontSize: 12.5, color: "var(--text)" }}>
              {n}
            </span>
            <span className="mono" style={{ fontSize: 11.5, color: "var(--text-mono-faint)", letterSpacing: 1 }}>
              ••••••••••••
            </span>
            <button className="btn btn-small btn-danger-hover" onClick={() => remove(n)}>
              Remove
            </button>
          </div>
        ))}
        {names && names.length === 0 && <p className="muted" style={{ padding: 12, margin: 0 }}>No credentials stored.</p>}
      </div>
      <form className="row" onSubmit={set}>
        <input style={{ width: 200 }} className="mono" placeholder="Name" value={newName} onChange={(e) => setNewName(e.currentTarget.value)} />
        <input type="password" style={{ flex: 1 }} placeholder="Value" value={newValue} onChange={(e) => setNewValue(e.currentTarget.value)} />
        <button className="btn btn-primary" type="submit">
          Store
        </button>
      </form>
    </>
  );
}

function TrustPane() {
  const [keys, setKeys] = useState<TrustedKey[] | null>(null);
  const [newName, setNewName] = useState("");
  const [newKey, setNewKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    setError(null);
    try {
      setKeys(await trustList());
    } catch (e) {
      setError(String(e));
    }
  }
  useEffect(() => {
    refresh();
  }, []);

  async function add(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await trustAdd(newName, newKey);
      setNewName("");
      setNewKey("");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove(name: string) {
    setError(null);
    try {
      await trustRemove(name);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <h1 style={{ margin: "0 0 6px", fontSize: 20, fontWeight: 600, letterSpacing: "-0.3px" }}>Trust store</h1>
      <p style={{ margin: "0 0 20px", color: "var(--text-faint)", fontSize: 12.5, lineHeight: 1.6, maxWidth: "62ch" }}>
        Named public keys used to verify plugin signatures. These are not secrets, so they sit in a
        plain file next to your settings.
      </p>
      {error && <p className="error-text">{error}</p>}
      <div className="table" style={{ marginBottom: 16 }}>
        {keys?.map((k) => (
          <div key={k.name} className="table-row" style={{ gridTemplateColumns: "130px 1fr 90px", alignItems: "center" }}>
            <span style={{ fontSize: 12.5, color: "var(--text)" }}>{k.name}</span>
            <span className="mono" style={{ fontSize: 11, color: "var(--text-fainter)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {k.public_key}
            </span>
            <button className="btn btn-small btn-danger-hover" onClick={() => remove(k.name)}>
              Remove
            </button>
          </div>
        ))}
        {keys && keys.length === 0 && <p className="muted" style={{ padding: 12, margin: 0 }}>No trusted keys.</p>}
      </div>
      <form className="row" onSubmit={add}>
        <input style={{ width: 200 }} placeholder="Name" value={newName} onChange={(e) => setNewName(e.currentTarget.value)} />
        <input className="mono" style={{ flex: 1 }} placeholder="Public key (hex)" value={newKey} onChange={(e) => setNewKey(e.currentTarget.value)} />
        <button className="btn btn-primary" type="submit">
          Add key
        </button>
      </form>
    </>
  );
}

function UpdatesPane() {
  const [status, setStatus] = useState<"idle" | "checking" | "up-to-date" | "available" | "installing" | "error">("idle");
  const [availableVersion, setAvailableVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function check() {
    setStatus("checking");
    setError(null);
    try {
      const update = await checkForUpdate();
      if (update) {
        setAvailableVersion(update.version);
        setStatus("available");
      } else {
        setStatus("up-to-date");
      }
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }

  async function installAndRestart() {
    setStatus("installing");
    setError(null);
    try {
      const update = await checkForUpdate();
      if (!update) {
        setStatus("up-to-date");
        return;
      }
      await update.downloadAndInstall();
      await relaunch();
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }

  const border = status === "available" ? "var(--border-accent-subtle)" : "var(--border)";

  return (
    <>
      <h1 style={{ margin: "0 0 6px", fontSize: 20, fontWeight: 600, letterSpacing: "-0.3px" }}>Updates</h1>
      <p style={{ margin: "0 0 20px", color: "var(--text-faint)", fontSize: 12.5, lineHeight: 1.6, maxWidth: "62ch" }}>
        Updates are signature-checked before anything is installed.
      </p>
      {error && <p className="error-text">{error}</p>}
      <div className="card" style={{ display: "flex", alignItems: "center", gap: 16, borderColor: border }}>
        <div style={{ flex: 1 }}>
          <div style={{ fontSize: 14, color: "var(--text)", marginBottom: 4 }}>
            {status === "idle" && "Check for updates"}
            {status === "checking" && "Checking…"}
            {status === "up-to-date" && "You're up to date"}
            {status === "available" && `Version ${availableVersion} is available`}
            {status === "installing" && "Installing…"}
            {status === "error" && "Couldn't check for updates"}
          </div>
        </div>
        {status === "available" ? (
          <button className="btn btn-primary" onClick={installAndRestart}>
            Install and restart
          </button>
        ) : (
          <button className="btn" onClick={check} disabled={status === "checking" || status === "installing"}>
            Check for updates
          </button>
        )}
      </div>
      <div className="col" style={{ marginTop: 18 }}>
        <div style={{ display: "flex", justifyContent: "space-between", paddingBottom: 9, borderBottom: "1px solid var(--border-row)" }}>
          <span className="muted">Installed version</span>
          <span className="mono" style={{ fontSize: 12 }}>
            0.1.0
          </span>
        </div>
      </div>
    </>
  );
}

export default function SettingsScreen() {
  const [tab, setTab] = useState<SettingsTab>("creds");
  const tabs: { k: SettingsTab; label: string }[] = [
    { k: "creds", label: "Credentials" },
    { k: "trust", label: "Trust store" },
    { k: "updates", label: "Updates" },
  ];

  return (
    <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
      <div style={{ width: 178, flex: "none", borderRight: "1px solid var(--border-subtle)", padding: "16px 10px", display: "flex", flexDirection: "column", gap: 2 }}>
        {tabs.map((t) => (
          <button
            key={t.k}
            className={"nav-item" + (tab === t.k ? " active" : "")}
            onClick={() => setTab(t.k)}
          >
            <span className="nav-item-label">{t.label}</span>
          </button>
        ))}
      </div>
      <div style={{ flex: 1, overflow: "auto", padding: "22px 26px" }}>
        <div style={{ maxWidth: 680 }}>
          {tab === "creds" && <CredentialsPane />}
          {tab === "trust" && <TrustPane />}
          {tab === "updates" && <UpdatesPane />}
        </div>
      </div>
    </div>
  );
}
