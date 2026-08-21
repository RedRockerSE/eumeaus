import { useState } from "react";
import type { CaseInfo } from "../api";
import { caseCreate, caseList, caseOpen } from "../api";
import { pickCaseFile, pickDirectory } from "../pickers";

// The design's right-hand panel shows a static "Recent" case list — no
// such MRU tracking exists in the backend (Case::list only lists cases
// in a given directory, on demand). Adapted honestly rather than faked:
// a directory browser using the real case_list, not invented history.
export default function Launcher({ onOpened }: { onOpened: (c: CaseInfo) => void }) {
  const [error, setError] = useState<string | null>(null);
  const [openPath, setOpenPath] = useState("");
  const [newDir, setNewDir] = useState("");
  const [newName, setNewName] = useState("");
  const [browseDir, setBrowseDir] = useState("");
  const [browseResults, setBrowseResults] = useState<CaseInfo[] | null>(null);

  async function open(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      onOpened(await caseOpen(openPath));
    } catch (e) {
      setError(String(e));
    }
  }

  async function create(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      onOpened(await caseCreate(newDir, newName));
    } catch (e) {
      setError(String(e));
    }
  }

  async function browse(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      setBrowseResults(await caseList(browseDir));
    } catch (e) {
      setError(String(e));
    }
  }

  async function openFromList(c: CaseInfo) {
    setError(null);
    try {
      onOpened(await caseOpen(c.path));
    } catch (e) {
      setError(String(e));
    }
  }

  async function browseForOpenPath() {
    const picked = await pickCaseFile();
    if (picked) setOpenPath(picked);
  }

  async function browseForNewDir() {
    const picked = await pickDirectory();
    if (picked) setNewDir(picked);
  }

  async function browseForBrowseDir() {
    const picked = await pickDirectory();
    if (picked) setBrowseDir(picked);
  }

  return (
    <div className="launcher">
      <div className="launcher-left">
        <div>
          <h1 style={{ margin: "0 0 10px", fontSize: 30, fontWeight: 600, letterSpacing: "-0.5px" }}>
            Open a case
          </h1>
          <p style={{ margin: 0, color: "var(--text-faint)", fontSize: 13, maxWidth: "40ch", lineHeight: 1.6 }}>
            Cases are single encrypted files on this machine. Nothing is uploaded, and one case is
            open per window.
          </p>
        </div>
        {error && <p className="error-text">{error}</p>}

        <form className="col" style={{ maxWidth: 400 }} onSubmit={open}>
          <label className="field-label">Case file</label>
          <div className="row">
            <input
              className="mono"
              style={{ flex: 1 }}
              value={openPath}
              onChange={(e) => setOpenPath(e.currentTarget.value)}
              placeholder="/home/you/Cases/nightjar.eum"
            />
            <button type="button" className="btn" onClick={browseForOpenPath}>
              Browse…
            </button>
            <button type="submit" className="btn btn-primary">
              Open case
            </button>
          </div>
        </form>

        <form className="col" style={{ maxWidth: 400 }} onSubmit={create}>
          <label className="field-label">New case</label>
          <div className="row">
            <input
              className="mono"
              style={{ flex: 1 }}
              value={newDir}
              onChange={(e) => setNewDir(e.currentTarget.value)}
              placeholder="Directory"
            />
            <button type="button" className="btn" onClick={browseForNewDir}>
              Browse…
            </button>
            <input
              style={{ flex: 1 }}
              value={newName}
              onChange={(e) => setNewName(e.currentTarget.value)}
              placeholder="Case name"
            />
            <button type="submit" className="btn btn-ghost">
              New case…
            </button>
          </div>
        </form>
      </div>

      <div className="launcher-right">
        <div className="field-label">Browse a directory</div>
        <form className="row" onSubmit={browse}>
          <input
            className="mono"
            style={{ flex: 1 }}
            value={browseDir}
            onChange={(e) => setBrowseDir(e.currentTarget.value)}
            placeholder="/home/you/Cases"
          />
          <button type="button" className="btn" onClick={browseForBrowseDir}>
            Browse…
          </button>
          <button type="submit" className="btn">
            List
          </button>
        </form>
        <div className="col">
          {browseResults && browseResults.length === 0 && (
            <p className="muted">No .eum files found there.</p>
          )}
          {browseResults?.map((c) => (
            <button key={c.path} className="recent-case-row" onClick={() => openFromList(c)}>
              <div className="badge" style={{ background: "#24242e" }}>
                <svg width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="#8b7ce8" strokeWidth="1.4">
                  <rect x="3" y="7" width="10" height="7" rx="1.5"></rect>
                  <path d="M5.5 7V5a2.5 2.5 0 015 0v2"></path>
                </svg>
              </div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13, color: "var(--text)", marginBottom: 2 }}>{c.name}</div>
                <div className="mono" style={{ fontSize: 10.5, color: "var(--text-mono-muted)" }}>
                  {c.path}
                </div>
              </div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
