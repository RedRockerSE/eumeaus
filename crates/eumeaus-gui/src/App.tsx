import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import type { CaseInfo, CaseStats, ScanProgress } from "./api";
import { caseClose, caseCurrent, caseStats, scanList } from "./api";
import TitleBar from "./components/TitleBar";
import Sidebar, { type Screen } from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import Launcher from "./screens/Launcher";
import OverviewScreen from "./screens/OverviewScreen";
import EntitiesScreen from "./screens/EntitiesScreen";
import GraphScreen from "./screens/GraphScreen";
import ScansScreen from "./screens/ScansScreen";
import PluginsScreen from "./screens/PluginsScreen";
import SettingsScreen from "./screens/SettingsScreen";

// The app shell (SPEC.md §9, UX redesign — Claude Design handover): a
// custom-titlebar window with a sidebar-navigated screen router when a
// case is open, and a launcher when none is. Every screen wires to real
// backend commands (case_state.rs / entity_state.rs / scan_state.rs /
// plugin_state.rs / credential_state.rs / trust_state.rs /
// overview_state.rs) — no mock data anywhere in the redesign, unlike the
// design file's own preview, which necessarily used fixtures.
function App() {
  const [currentCase, setCurrentCase] = useState<CaseInfo | null>(null);
  const [screen, setScreen] = useState<Screen>("overview");
  const [stats, setStats] = useState<CaseStats | null>(null);
  const [scanRunning, setScanRunning] = useState(false);
  const [statusRight, setStatusRight] = useState("Ready");

  useEffect(() => {
    caseCurrent().then((c) => {
      setCurrentCase(c);
      if (c) setScreen("overview");
    });
  }, []);

  useEffect(() => {
    if (!currentCase) {
      setStats(null);
      return;
    }
    refreshStats();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentCase?.id]);

  async function refreshStats() {
    try {
      setStats(await caseStats());
    } catch {
      // no case open yet, or a transient error — the relevant screen
      // already surfaces its own error state.
    }
  }

  // Best-effort "is a scan running" tracker for the sidebar's pulsing dot
  // — ScansScreen owns the detailed progress UI; this only needs a
  // coarse yes/no. A RUNNING event marks the flag true; any terminal
  // event re-checks that scan's own overall status via scan_list (a
  // progress event alone doesn't say whether it was the *last* plugin).
  useEffect(() => {
    const unlisten = listen<ScanProgress>("scan-progress", async (event) => {
      if (event.payload.status === "RUNNING") {
        setScanRunning(true);
        setStatusRight("Scan running…");
        return;
      }
      try {
        const scans = await scanList();
        const still = scans.some((s) => s.id === event.payload.scan_id && s.status === "RUNNING");
        setScanRunning(still);
        if (!still) setStatusRight("Ready");
      } catch {
        // ignore — the dot just stays as it was
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  async function closeCase() {
    try {
      await caseClose();
      setCurrentCase(null);
    } catch {
      // surfaced nowhere specific — closing a case the user just asked
      // to close failing is rare enough not to warrant its own screen.
    }
  }

  return (
    <div className="app-root">
      <TitleBar current={currentCase} />

      {!currentCase && <Launcher onOpened={setCurrentCase} />}

      {currentCase && (
        <>
          <div className="shell">
            <Sidebar
              screen={screen}
              onNavigate={setScreen}
              onCloseCase={closeCase}
              entityCount={stats?.entity_count ?? 0}
              scanRunning={scanRunning}
            />
            <div className="main">
              {screen === "overview" && <OverviewScreen current={currentCase} />}
              {screen === "entities" && <EntitiesScreen onEntitiesChanged={refreshStats} />}
              {screen === "graph" && <GraphScreen />}
              {screen === "scans" && <ScansScreen />}
              {screen === "plugins" && <PluginsScreen />}
              {screen === "settings" && <SettingsScreen />}
            </div>
          </div>
          <StatusBar
            entityCount={stats?.entity_count ?? 0}
            factCount={stats?.fact_count ?? 0}
            statusRight={statusRight}
          />
        </>
      )}
    </div>
  );
}

export default App;
