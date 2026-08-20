import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { check as checkForUpdate } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import "./App.css";

interface CaseInfo {
  id: string;
  name: string;
  path: string;
}

interface EntitySummary {
  id: string;
  entity_type: string;
  canonical_key: string | null;
  display_label: string;
}

interface AttributeRecord {
  fact_id: string;
  key: string;
  value: string;
  source: string;
  collected_at_unix_ms: number;
  is_current: boolean;
  conflicting: boolean;
}

interface EntityDetail extends EntitySummary {
  attributes: AttributeRecord[];
}

interface ScanProgress {
  scan_id: string;
  plugin_name: string;
  status: string;
  error_message: string | null;
}

interface ScanSummary {
  id: string;
  status: string;
  target_entity_id: string;
  started_at_unix_ms: number | null;
  completed_at_unix_ms: number | null;
}

interface PluginSummary {
  name: string;
  version: string;
  signed: boolean;
  entrypoint: string;
}

interface TrustedKey {
  name: string;
  public_key: string;
}

// G1 (SPEC.md §9.6): create/open/close a case from the GUI, backed by the
// real keychain + SQLCipher path — the same eumeaus-engine::Case the CLI
// uses, not a mock. One case open at a time in this window (case_create/
// case_open both reject a second concurrent case; see case_state.rs).
function CaseScreen({
  current,
  onChange,
}: {
  current: CaseInfo | null;
  onChange: (c: CaseInfo | null) => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [dir, setDir] = useState("");
  const [name, setName] = useState("");
  const [openPath, setOpenPath] = useState("");
  const [listDir, setListDir] = useState("");
  const [listing, setListing] = useState<CaseInfo[] | null>(null);

  async function create(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      onChange(await invoke<CaseInfo>("case_create", { dir, name }));
    } catch (e) {
      setError(String(e));
    }
  }

  async function open(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      onChange(await invoke<CaseInfo>("case_open", { path: openPath }));
    } catch (e) {
      setError(String(e));
    }
  }

  async function close() {
    setError(null);
    try {
      await invoke("case_close");
      onChange(null);
    } catch (e) {
      setError(String(e));
    }
  }

  async function list(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      setListing(await invoke<CaseInfo[]>("case_list", { dir: listDir }));
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section>
      <h2>Case</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}

      {current ? (
        <div>
          <p>
            <strong>{current.name}</strong> ({current.id})
            <br />
            {current.path}
          </p>
          <button onClick={close}>Close case</button>
        </div>
      ) : (
        <div className="row">
          <form onSubmit={create}>
            <h3>Create</h3>
            <input
              placeholder="Directory"
              value={dir}
              onChange={(e) => setDir(e.currentTarget.value)}
            />
            <input
              placeholder="Case name"
              value={name}
              onChange={(e) => setName(e.currentTarget.value)}
            />
            <button type="submit">Create</button>
          </form>

          <form onSubmit={open}>
            <h3>Open</h3>
            <input
              placeholder="Path to .eum file"
              value={openPath}
              onChange={(e) => setOpenPath(e.currentTarget.value)}
            />
            <button type="submit">Open</button>
          </form>

          <form onSubmit={list}>
            <h3>List</h3>
            <input
              placeholder="Directory"
              value={listDir}
              onChange={(e) => setListDir(e.currentTarget.value)}
            />
            <button type="submit">List</button>
            {listing && (
              <ul>
                {listing.map((c) => (
                  <li key={c.id}>
                    {c.name} — {c.path}
                  </li>
                ))}
              </ul>
            )}
          </form>
        </div>
      )}
    </section>
  );
}

// G2 (SPEC.md §9.6): entity/fact browsing, read-only — wraps entity_list/
// entity_show (entity_state.rs), the same Case::list_entities/get_entity/
// list_attribute_records calls eumeaus-cli's `entity list`/`entity show`
// make.
//
// G4: the write path — entity_add/entity_merge/entity_split/
// relationship_add. Every write refreshes the list afterwards rather than
// trying to patch state locally, since a merge/split can change which
// entities exist at all.
function EntityScreen() {
  const [entities, setEntities] = useState<EntitySummary[] | null>(null);
  const [selected, setSelected] = useState<EntityDetail | null>(null);
  const [typeFilter, setTypeFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  const [newType, setNewType] = useState("Person");
  const [newKey, setNewKey] = useState("");
  const [newAttrKey, setNewAttrKey] = useState("");
  const [newAttrValue, setNewAttrValue] = useState("");

  const [mergeId1, setMergeId1] = useState("");
  const [mergeId2, setMergeId2] = useState("");

  const [splitFactIds, setSplitFactIds] = useState<Set<string>>(new Set());

  const [relFrom, setRelFrom] = useState("");
  const [relTo, setRelTo] = useState("");
  const [relType, setRelType] = useState("AssociatedWith");

  async function refresh(e?: React.FormEvent) {
    e?.preventDefault();
    setError(null);
    try {
      setEntities(
        await invoke<EntitySummary[]>("entity_list", {
          entityType: typeFilter || null,
        }),
      );
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function show(id: string) {
    setError(null);
    setSplitFactIds(new Set());
    try {
      setSelected(await invoke<EntityDetail>("entity_show", { id }));
    } catch (e) {
      setError(String(e));
    }
  }

  async function addEntity(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const attrs = newAttrKey.trim()
        ? [{ key: newAttrKey, value: newAttrValue }]
        : [];
      await invoke("entity_add", {
        entityType: newType,
        key: newKey || null,
        attrs,
      });
      setNewKey("");
      setNewAttrKey("");
      setNewAttrValue("");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function mergeEntities(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await invoke("entity_merge", { id1: mergeId1, id2: mergeId2 });
      setMergeId1("");
      setMergeId2("");
      setSelected(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function splitEntity() {
    if (!selected || splitFactIds.size === 0) return;
    setError(null);
    try {
      await invoke("entity_split", {
        id: selected.id,
        factIds: Array.from(splitFactIds),
      });
      setSelected(null);
      setSplitFactIds(new Set());
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function addRelationship(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await invoke("relationship_add", {
        from: relFrom,
        to: relTo,
        relType,
        attrs: [],
      });
      setRelFrom("");
      setRelTo("");
    } catch (e) {
      setError(String(e));
    }
  }

  function toggleSplitFact(factId: string) {
    setSplitFactIds((prev) => {
      const next = new Set(prev);
      if (next.has(factId)) next.delete(factId);
      else next.add(factId);
      return next;
    });
  }

  return (
    <section>
      <h2>Entities</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}

      <form onSubmit={refresh}>
        <input
          placeholder="Filter by type (optional)"
          value={typeFilter}
          onChange={(e) => setTypeFilter(e.currentTarget.value)}
        />
        <button type="submit">Refresh</button>
      </form>

      <form onSubmit={addEntity}>
        <h3>Add entity</h3>
        <input
          placeholder="Type"
          value={newType}
          onChange={(e) => setNewType(e.currentTarget.value)}
        />
        <input
          placeholder="Canonical key (optional)"
          value={newKey}
          onChange={(e) => setNewKey(e.currentTarget.value)}
        />
        <input
          placeholder="Attribute key (optional)"
          value={newAttrKey}
          onChange={(e) => setNewAttrKey(e.currentTarget.value)}
        />
        <input
          placeholder="Attribute value"
          value={newAttrValue}
          onChange={(e) => setNewAttrValue(e.currentTarget.value)}
        />
        <button type="submit">Add</button>
      </form>

      <form onSubmit={mergeEntities}>
        <h3>Merge</h3>
        <input
          placeholder="Entity id 1 (survivor)"
          value={mergeId1}
          onChange={(e) => setMergeId1(e.currentTarget.value)}
        />
        <input
          placeholder="Entity id 2"
          value={mergeId2}
          onChange={(e) => setMergeId2(e.currentTarget.value)}
        />
        <button type="submit">Merge</button>
      </form>

      <form onSubmit={addRelationship}>
        <h3>Add relationship</h3>
        <input
          placeholder="From entity id"
          value={relFrom}
          onChange={(e) => setRelFrom(e.currentTarget.value)}
        />
        <input
          placeholder="To entity id"
          value={relTo}
          onChange={(e) => setRelTo(e.currentTarget.value)}
        />
        <input
          placeholder="Relationship type"
          value={relType}
          onChange={(e) => setRelType(e.currentTarget.value)}
        />
        <button type="submit">Add relationship</button>
      </form>

      {entities && entities.length === 0 && <p>(no entities)</p>}
      {entities && entities.length > 0 && (
        <ul>
          {entities.map((ent) => (
            <li key={ent.id}>
              <button onClick={() => show(ent.id)}>
                {ent.entity_type}: {ent.display_label} ({ent.canonical_key ?? "-"})
              </button>
            </li>
          ))}
        </ul>
      )}

      {selected && (
        <div>
          <h3>{selected.display_label}</h3>
          <p>
            id: {selected.id}
            <br />
            type: {selected.entity_type}
            <br />
            canonical_key: {selected.canonical_key ?? "-"}
          </p>
          <h4>Attributes (check to split into a new entity)</h4>
          {selected.attributes.length === 0 && <p>(none)</p>}
          {selected.attributes.length > 0 && (
            <ul>
              {selected.attributes.map((a) => (
                <li key={a.fact_id}>
                  <label>
                    <input
                      type="checkbox"
                      checked={splitFactIds.has(a.fact_id)}
                      onChange={() => toggleSplitFact(a.fact_id)}
                    />
                    {a.is_current ? "*" : " "} {a.key} = {a.value} (fact:{" "}
                    {a.fact_id}, source: {a.source})
                  </label>
                </li>
              ))}
            </ul>
          )}
          <button onClick={splitEntity} disabled={splitFactIds.size === 0}>
            Split checked facts into a new entity
          </button>
        </div>
      )}
    </section>
  );
}

// G3 (SPEC.md §9.6): scan run + live progress. scan_run returns as soon
// as the scan is created; per-plugin RUNNING/SUCCESS/TIMEOUT/ERROR
// transitions arrive afterwards as "scan-progress" events (scan_state.rs)
// rather than through the command's own return value.
function ScanScreen() {
  const [pluginsDir, setPluginsDir] = useState("");
  const [plugin, setPlugin] = useState("");
  const [targetType, setTargetType] = useState("Username");
  const [targetValue, setTargetValue] = useState("");
  const [activeScanId, setActiveScanId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ScanProgress[]>([]);
  const [scans, setScans] = useState<ScanSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const activeScanIdRef = useRef<string | null>(null);

  useEffect(() => {
    const unlisten = listen<ScanProgress>("scan-progress", (event) => {
      if (event.payload.scan_id !== activeScanIdRef.current) return;
      setProgress((prev) => [...prev, event.payload]);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  async function run(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setProgress([]);
    // A blank plugins directory reaches Case::create_scan as Path::new("")
    // and fails with an engine error that reads oddly with nothing to
    // fill its {0} — caught during G3's own live testing. Catch it here
    // with a clearer message instead.
    if (!pluginsDir.trim() || !targetType.trim() || !targetValue.trim()) {
      setError("Plugins directory, target type, and target key are all required.");
      return;
    }
    try {
      const scanId = await invoke<string>("scan_run", {
        pluginsDir,
        plugin: plugin
          .split(",")
          .map((p) => p.trim())
          .filter(Boolean),
        targetType,
        targetValue,
      });
      activeScanIdRef.current = scanId;
      setActiveScanId(scanId);
    } catch (e) {
      setError(String(e));
    }
  }

  async function refreshScans() {
    setError(null);
    try {
      setScans(await invoke<ScanSummary[]>("scan_list"));
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section>
      <h2>Scan</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}

      <form onSubmit={run}>
        <input
          placeholder="Plugins directory"
          value={pluginsDir}
          onChange={(e) => setPluginsDir(e.currentTarget.value)}
        />
        <input
          placeholder="Plugin name(s), comma-separated (blank = all compatible)"
          value={plugin}
          onChange={(e) => setPlugin(e.currentTarget.value)}
        />
        <input
          placeholder="Target entity type"
          value={targetType}
          onChange={(e) => setTargetType(e.currentTarget.value)}
        />
        <input
          placeholder="Target entity key"
          value={targetValue}
          onChange={(e) => setTargetValue(e.currentTarget.value)}
        />
        <button type="submit">Run scan</button>
      </form>

      {activeScanId && (
        <div>
          <h3>Scan {activeScanId}</h3>
          {progress.length === 0 && <p>Waiting for progress…</p>}
          <ul>
            {progress.map((p, i) => (
              <li key={i}>
                {p.plugin_name}: {p.status}
                {p.error_message ? ` (${p.error_message})` : ""}
              </li>
            ))}
          </ul>
        </div>
      )}

      <button onClick={refreshScans}>Refresh scan list</button>
      {scans && (
        <ul>
          {scans.map((s) => (
            <li key={s.id}>
              {s.id}: {s.status}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// G5 (SPEC.md §9.6): plugin list/install/verify. Not case-scoped (a
// plugins_dir is just a path) — unlike Entity/Scan screens, this one
// doesn't need a case open at all.
function PluginScreen() {
  const [pluginsDir, setPluginsDir] = useState("");
  const [plugins, setPlugins] = useState<PluginSummary[] | null>(null);
  const [sourcePath, setSourcePath] = useState("");
  const [verifyName, setVerifyName] = useState("");
  const [verifyTrust, setVerifyTrust] = useState("");
  const [verifyResult, setVerifyResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function list(e?: React.FormEvent) {
    e?.preventDefault();
    setError(null);
    try {
      setPlugins(await invoke<PluginSummary[]>("plugin_list", { pluginsDir }));
    } catch (e) {
      setError(String(e));
    }
  }

  async function install(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      await invoke("plugin_install", { sourcePath, pluginsDir });
      setSourcePath("");
      await list();
    } catch (e) {
      setError(String(e));
    }
  }

  async function verify(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setVerifyResult(null);
    try {
      await invoke("plugin_verify", {
        name: verifyName,
        pluginsDir,
        trust: verifyTrust || null,
      });
      setVerifyResult("valid");
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section>
      <h2>Plugins</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}

      <form onSubmit={list}>
        <input
          placeholder="Plugins directory"
          value={pluginsDir}
          onChange={(e) => setPluginsDir(e.currentTarget.value)}
        />
        <button type="submit">List</button>
      </form>

      {plugins && (
        <ul>
          {plugins.map((p) => (
            <li key={p.name}>
              {p.name} {p.version} ({p.signed ? "signed" : "unsigned"}) —{" "}
              {p.entrypoint}
            </li>
          ))}
        </ul>
      )}

      <form onSubmit={install}>
        <h3>Install</h3>
        <input
          placeholder="Source directory (has plugin.toml)"
          value={sourcePath}
          onChange={(e) => setSourcePath(e.currentTarget.value)}
        />
        <button type="submit">Install</button>
      </form>

      <form onSubmit={verify}>
        <h3>Verify</h3>
        <input
          placeholder="Plugin name"
          value={verifyName}
          onChange={(e) => setVerifyName(e.currentTarget.value)}
        />
        <input
          placeholder="Trust store name"
          value={verifyTrust}
          onChange={(e) => setVerifyTrust(e.currentTarget.value)}
        />
        <button type="submit">Verify</button>
        {verifyResult && <span> {verifyResult}</span>}
      </form>
    </section>
  );
}

// G5: credential management, global to the OS user account (not case-
// scoped) — a normal password <input> submitted through invoke() is the
// GUI-native equivalent of the CLI's rpassword TTY prompt (see
// credential_state.rs's doc for why that's not a shortcut around the
// concern rpassword addresses).
function CredentialScreen() {
  const [names, setNames] = useState<string[] | null>(null);
  const [newName, setNewName] = useState("");
  const [newValue, setNewValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    setError(null);
    try {
      setNames(await invoke<string[]>("credential_list"));
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
      await invoke("credential_set", { name: newName, value: newValue });
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
      await invoke("credential_remove", { name });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section>
      <h2>Credentials</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}

      <form onSubmit={set}>
        <input
          placeholder="Name"
          value={newName}
          onChange={(e) => setNewName(e.currentTarget.value)}
        />
        <input
          type="password"
          placeholder="Value"
          value={newValue}
          onChange={(e) => setNewValue(e.currentTarget.value)}
        />
        <button type="submit">Set</button>
      </form>

      {names && (
        <ul>
          {names.map((n) => (
            <li key={n}>
              {n} <button onClick={() => remove(n)}>Remove</button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// G5: local trust store (SPEC.md §8 open question 2) — named public keys,
// not secrets, so a plain file rather than the OS keychain.
function TrustScreen() {
  const [keys, setKeys] = useState<TrustedKey[] | null>(null);
  const [newName, setNewName] = useState("");
  const [newKey, setNewKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    setError(null);
    try {
      setKeys(await invoke<TrustedKey[]>("trust_list"));
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
      await invoke("trust_add", { name: newName, publicKey: newKey });
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
      await invoke("trust_remove", { name });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <section>
      <h2>Trust store</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}

      <form onSubmit={add}>
        <input
          placeholder="Name"
          value={newName}
          onChange={(e) => setNewName(e.currentTarget.value)}
        />
        <input
          placeholder="Public key (hex)"
          value={newKey}
          onChange={(e) => setNewKey(e.currentTarget.value)}
        />
        <button type="submit">Add</button>
      </form>

      {keys && (
        <ul>
          {keys.map((k) => (
            <li key={k.name}>
              {k.name}: {k.public_key}{" "}
              <button onClick={() => remove(k.name)}>Remove</button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// G6 (SPEC.md §9.4/§9.6, resolves §8.8): tauri-plugin-updater — checks
// the endpoint configured in tauri.conf.json (a latest.json the release
// workflow publishes, same GitHub Releases hosting install.sh/.ps1
// already use for the CLI). Signature verification against the pubkey
// in tauri.conf.json happens inside the plugin itself before download()
// even starts; a tampered/unsigned latest.json fails there, not here.
function UpdateScreen() {
  const [status, setStatus] = useState<
    "idle" | "checking" | "up-to-date" | "available" | "installing" | "installed" | "error"
  >("idle");
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
      setStatus("installed");
      await relaunch();
    } catch (e) {
      setError(String(e));
      setStatus("error");
    }
  }

  return (
    <section>
      <h2>Updates</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}
      <button onClick={check} disabled={status === "checking"}>
        Check for updates
      </button>
      {status === "up-to-date" && <p>Up to date.</p>}
      {status === "available" && (
        <div>
          <p>Version {availableVersion} is available.</p>
          <button onClick={installAndRestart}>Install and restart</button>
        </div>
      )}
      {status === "installing" && <p>Installing…</p>}
    </section>
  );
}

// G0 (SPEC.md §9.6): fetch the taxonomy from eumeaus-engine via the
// list_entity_types command, proving the IPC boundary works end to end
// (frontend -> Tauri command -> eumeaus-engine -> back).
function EntityTypesScreen() {
  const [entityTypes, setEntityTypes] = useState<string[] | null>(null);

  useEffect(() => {
    invoke<string[]>("list_entity_types").then(setEntityTypes);
  }, []);

  return (
    <section>
      <h2>Entity types (from eumeaus-engine)</h2>
      {!entityTypes && <p>Loading…</p>}
      {entityTypes && (
        <ul>
          {entityTypes.map((t) => (
            <li key={t}>{t}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

function App() {
  const [currentCase, setCurrentCase] = useState<CaseInfo | null>(null);

  useEffect(() => {
    invoke<CaseInfo | null>("case_current").then(setCurrentCase);
  }, []);

  return (
    <main className="container">
      <h1>Eumeaus</h1>
      <p>GUI scaffold (SPEC.md §9, milestone G6)</p>
      <CaseScreen current={currentCase} onChange={setCurrentCase} />
      {currentCase && <EntityScreen />}
      {currentCase && <ScanScreen />}
      <PluginScreen />
      <CredentialScreen />
      <TrustScreen />
      <UpdateScreen />
      <EntityTypesScreen />
    </main>
  );
}

export default App;
