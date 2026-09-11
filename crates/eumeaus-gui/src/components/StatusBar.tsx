export default function StatusBar({
  entityCount,
  factCount,
  statusRight,
  scanRunning,
}: {
  entityCount: number;
  factCount: number;
  statusRight: string;
  scanRunning: boolean;
}) {
  return (
    <div className="statusbar">
      <span>
        <span className="statusbar-dot" />
        Encrypted · local only
      </span>
      <span className="statusbar-mono">
        {entityCount} entities · {factCount} facts
      </span>
      <span style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 7 }}>
        {scanRunning && <span className="statusbar-scan-dot" />}
        <span style={scanRunning ? { color: "var(--accent)" } : undefined}>{statusRight}</span>
      </span>
    </div>
  );
}
