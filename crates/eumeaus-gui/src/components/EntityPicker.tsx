import { useEffect, useRef, useState } from "react";
import type { EntitySummary } from "../api";

// Searchable/filterable entity combobox (exploratory test §3.2/§4.2:
// "should be selectable from a dropdown," not a free-text id/value the
// user has to already know by heart). Reused in two places: picking a
// relationship's other entity (EntitiesScreen), and picking a scan's
// target value among entities of the chosen type (ScansScreen) — both
// need the same "type to filter, click to pick" behavior, just over a
// different candidate list.
export default function EntityPicker({
  entities,
  value,
  onChange,
  placeholder,
  excludeId,
}: {
  entities: EntitySummary[];
  value: string;
  onChange: (id: string) => void;
  placeholder?: string;
  excludeId?: string;
}) {
  const candidates = entities.filter((e) => e.id !== excludeId);
  const selected = candidates.find((e) => e.id === value) ?? null;
  const [query, setQuery] = useState(selected?.display_label ?? "");
  const [open, setOpen] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setQuery(selected?.display_label ?? "");
    // Only re-sync when the externally-controlled selection changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  useEffect(() => {
    function onDocClick(e: MouseEvent) {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, []);

  const q = query.trim().toLowerCase();
  const matches = q
    ? candidates.filter(
        (e) =>
          e.display_label.toLowerCase().includes(q) ||
          (e.canonical_key ?? "").toLowerCase().includes(q),
      )
    : candidates;

  function pick(e: EntitySummary) {
    onChange(e.id);
    setQuery(e.display_label);
    setOpen(false);
  }

  return (
    <div ref={boxRef} style={{ position: "relative", flex: 1, minWidth: 0 }}>
      <input
        className="mono"
        style={{ width: "100%" }}
        value={query}
        placeholder={placeholder}
        onFocus={() => setOpen(true)}
        onChange={(e) => {
          setQuery(e.currentTarget.value);
          setOpen(true);
          if (value) onChange("");
        }}
      />
      {open && (
        <div
          style={{
            position: "absolute",
            top: "100%",
            left: 0,
            right: 0,
            zIndex: 20,
            maxHeight: 220,
            overflow: "auto",
            background: "var(--bg-panel)",
            border: "1px solid #2c2c36",
            borderRadius: 5,
            marginTop: 3,
            boxShadow: "0 6px 18px rgba(0,0,0,0.35)",
          }}
        >
          {matches.length === 0 && (
            <div style={{ padding: "8px 9px", fontSize: 11.5, color: "var(--text-mono-muted)" }}>
              No matching entities.
            </div>
          )}
          {matches.slice(0, 50).map((e) => (
            <div
              key={e.id}
              onMouseDown={(ev) => {
                // Fires before the input's onBlur, so the click still
                // registers instead of the dropdown closing first.
                ev.preventDefault();
                pick(e);
              }}
              style={{ padding: "7px 9px", cursor: "pointer", fontSize: 12 }}
              className="entity-picker-row"
            >
              <span style={{ color: "var(--text)" }}>{e.display_label}</span>{" "}
              <span className="mono" style={{ color: "var(--text-mono-muted)", fontSize: 10.5 }}>
                {e.entity_type}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
