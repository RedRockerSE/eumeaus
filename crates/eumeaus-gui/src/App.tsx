import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
// make. Write path (add/merge/split) starts at G4.
function EntityScreen() {
  const [entities, setEntities] = useState<EntitySummary[] | null>(null);
  const [selected, setSelected] = useState<EntityDetail | null>(null);
  const [typeFilter, setTypeFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

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
    try {
      setSelected(await invoke<EntityDetail>("entity_show", { id }));
    } catch (e) {
      setError(String(e));
    }
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
          <h4>Attributes</h4>
          {selected.attributes.length === 0 && <p>(none)</p>}
          {selected.attributes.length > 0 && (
            <ul>
              {selected.attributes.map((a) => (
                <li key={a.fact_id}>
                  {a.is_current ? "*" : " "} {a.key} = {a.value} (fact:{" "}
                  {a.fact_id}, source: {a.source})
                </li>
              ))}
            </ul>
          )}
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
      <p>GUI scaffold (SPEC.md §9, milestone G3)</p>
      <CaseScreen current={currentCase} onChange={setCurrentCase} />
      {currentCase && <EntityScreen />}
      {currentCase && <ScanScreen />}
      <EntityTypesScreen />
    </main>
  );
}

export default App;
