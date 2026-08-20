export type Screen = "overview" | "entities" | "graph" | "scans" | "plugins" | "settings";

function NavItem({
  label,
  active,
  onClick,
  count,
  pulsing,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  count?: number;
  pulsing?: boolean;
}) {
  return (
    <button className={"nav-item" + (active ? " active" : "")} onClick={onClick}>
      <span className="nav-item-label">{label}</span>
      {count !== undefined && <span className="nav-item-count">{count}</span>}
      {pulsing && <span className="nav-item-dot" />}
    </button>
  );
}

export default function Sidebar({
  screen,
  onNavigate,
  onCloseCase,
  entityCount,
  scanRunning,
}: {
  screen: Screen;
  onNavigate: (s: Screen) => void;
  onCloseCase: () => void;
  entityCount: number;
  scanRunning: boolean;
}) {
  return (
    <div className="sidebar">
      <div className="sidebar-group-label">Case</div>
      <NavItem label="Overview" active={screen === "overview"} onClick={() => onNavigate("overview")} />
      <NavItem
        label="Entities"
        active={screen === "entities"}
        onClick={() => onNavigate("entities")}
        count={entityCount}
      />
      <NavItem label="Graph" active={screen === "graph"} onClick={() => onNavigate("graph")} />
      <NavItem
        label="Scans"
        active={screen === "scans"}
        onClick={() => onNavigate("scans")}
        pulsing={scanRunning}
      />

      <div className="sidebar-group-label spaced">Machine</div>
      <NavItem label="Plugins" active={screen === "plugins"} onClick={() => onNavigate("plugins")} />
      <NavItem label="Settings" active={screen === "settings"} onClick={() => onNavigate("settings")} />

      <div className="sidebar-footer">
        <button className="btn" onClick={onCloseCase}>
          Close case
        </button>
      </div>
    </div>
  );
}
