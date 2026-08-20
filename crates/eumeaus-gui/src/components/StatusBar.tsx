export default function StatusBar({
  entityCount,
  factCount,
  statusRight,
}: {
  entityCount: number;
  factCount: number;
  statusRight: string;
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
      <span style={{ marginLeft: "auto" }}>{statusRight}</span>
    </div>
  );
}
