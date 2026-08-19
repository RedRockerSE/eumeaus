import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

// G0 (SPEC.md §9.6): fetch the taxonomy from eumeaus-engine via the
// list_entity_types command, proving the IPC boundary works end to end
// (frontend -> Tauri command -> eumeaus-engine -> back). Real case/entity
// screens start at G1/G2.
function App() {
  const [entityTypes, setEntityTypes] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string[]>("list_entity_types")
      .then(setEntityTypes)
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <main className="container">
      <h1>Eumeaus</h1>
      <p>GUI scaffold (SPEC.md §9, milestone G0)</p>

      <h2>Entity types (from eumeaus-engine)</h2>
      {error && <p style={{ color: "red" }}>{error}</p>}
      {!error && !entityTypes && <p>Loading…</p>}
      {entityTypes && (
        <ul>
          {entityTypes.map((t) => (
            <li key={t}>{t}</li>
          ))}
        </ul>
      )}
    </main>
  );
}

export default App;
