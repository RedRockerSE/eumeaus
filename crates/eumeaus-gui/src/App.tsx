import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface CaseInfo {
  id: string;
  name: string;
  path: string;
}

// G1 (SPEC.md §9.6): create/open/close a case from the GUI, backed by the
// real keychain + SQLCipher path — the same eumeaus-engine::Case the CLI
// uses, not a mock. One case open at a time in this window (case_create/
// case_open both reject a second concurrent case; see case_state.rs).
function CaseScreen() {
  const [current, setCurrent] = useState<CaseInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dir, setDir] = useState("");
  const [name, setName] = useState("");
  const [openPath, setOpenPath] = useState("");
  const [listDir, setListDir] = useState("");
  const [listing, setListing] = useState<CaseInfo[] | null>(null);

  useEffect(() => {
    invoke<CaseInfo | null>("case_current")
      .then(setCurrent)
      .catch((e) => setError(String(e)));
  }, []);

  async function create(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      setCurrent(await invoke<CaseInfo>("case_create", { dir, name }));
    } catch (e) {
      setError(String(e));
    }
  }

  async function open(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      setCurrent(await invoke<CaseInfo>("case_open", { path: openPath }));
    } catch (e) {
      setError(String(e));
    }
  }

  async function close() {
    setError(null);
    try {
      await invoke("case_close");
      setCurrent(null);
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
  return (
    <main className="container">
      <h1>Eumeaus</h1>
      <p>GUI scaffold (SPEC.md §9, milestone G1)</p>
      <CaseScreen />
      <EntityTypesScreen />
    </main>
  );
}

export default App;
