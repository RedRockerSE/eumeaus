import { useEffect, useState } from "react";
import type { AttributeInput, AuditEvent, EntityDetail, EntitySummary, RelationshipDto } from "../api";
import {
  entityAdd,
  entityAddFact,
  entityAudit,
  entityList,
  entityMerge,
  entityShow,
  entitySplit,
  relationshipAdd,
  relationshipList,
} from "../api";
import { ENTITY_TYPES, RELATIONSHIP_TYPES, styleForEntityType } from "../entityStyle";
import EntityPicker from "../components/EntityPicker";

type Tab = "facts" | "links" | "history";
const CUSTOM_REL_TYPE = "__custom__";

export default function EntitiesScreen({ onEntitiesChanged }: { onEntitiesChanged: () => void }) {
  const [entities, setEntities] = useState<EntitySummary[] | null>(null);
  const [typeFilter, setTypeFilter] = useState("All");
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selected, setSelected] = useState<EntityDetail | null>(null);
  const [tab, setTab] = useState<Tab>("facts");
  const [error, setError] = useState<string | null>(null);

  const [addOpen, setAddOpen] = useState(false);
  const [newType, setNewType] = useState("Person");
  const [newKey, setNewKey] = useState("");
  const [newAttrKey, setNewAttrKey] = useState("");
  const [newAttrValue, setNewAttrValue] = useState("");

  const [addFactOpen, setAddFactOpen] = useState(false);
  const [newFactKey, setNewFactKey] = useState("");
  const [newFactValue, setNewFactValue] = useState("");

  const [mergeOpen, setMergeOpen] = useState(false);
  const [mergeOther, setMergeOther] = useState("");

  const [splitMode, setSplitMode] = useState(false);
  const [splitFactIds, setSplitFactIds] = useState<Set<string>>(new Set());

  const [relationships, setRelationships] = useState<RelationshipDto[] | null>(null);
  const [relToId, setRelToId] = useState("");
  const [relType, setRelType] = useState(RELATIONSHIP_TYPES[2]); // "AssociatedWith"
  const [customRelType, setCustomRelType] = useState("");

  const [history, setHistory] = useState<AuditEvent[] | null>(null);

  async function refresh() {
    setError(null);
    try {
      const list = await entityList(typeFilter === "All" ? null : typeFilter);
      setEntities(list);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [typeFilter]);

  useEffect(() => {
    if (!selectedId) {
      setSelected(null);
      return;
    }
    entityShow(selectedId)
      .then(setSelected)
      .catch((e) => setError(String(e)));
  }, [selectedId]);

  useEffect(() => {
    if (!selectedId || tab !== "links") return;
    relationshipList()
      .then(setRelationships)
      .catch((e) => setError(String(e)));
  }, [selectedId, tab]);

  useEffect(() => {
    if (!selectedId || tab !== "history") return;
    entityAudit(selectedId)
      .then(setHistory)
      .catch((e) => setError(String(e)));
  }, [selectedId, tab]);

  function selectEntity(id: string) {
    setSelectedId(id);
    setTab("facts");
    setSplitMode(false);
    setSplitFactIds(new Set());
    setMergeOpen(false);
    setAddFactOpen(false);
    setRelToId("");
  }

  const filtered = (entities ?? []).filter((e) => {
    if (!query.trim()) return true;
    const q = query.toLowerCase();
    return (
      e.display_label.toLowerCase().includes(q) ||
      (e.canonical_key ?? "").toLowerCase().includes(q)
    );
  });

  async function addEntity(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const attrs: AttributeInput[] = newAttrKey.trim() ? [{ key: newAttrKey, value: newAttrValue }] : [];
      await entityAdd(newType, newKey || null, attrs);
      setNewKey("");
      setNewAttrKey("");
      setNewAttrValue("");
      setAddOpen(false);
      await refresh();
      onEntitiesChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doMerge(e: React.FormEvent) {
    e.preventDefault();
    if (!selectedId) return;
    setError(null);
    try {
      const survivor = await entityMerge(selectedId, mergeOther);
      setMergeOpen(false);
      setMergeOther("");
      await refresh();
      onEntitiesChanged();
      selectEntity(survivor.id);
    } catch (e) {
      setError(String(e));
    }
  }

  async function doSplit() {
    if (!selectedId || splitFactIds.size === 0) return;
    setError(null);
    try {
      await entitySplit(selectedId, Array.from(splitFactIds));
      setSplitMode(false);
      setSplitFactIds(new Set());
      await refresh();
      onEntitiesChanged();
      selectEntity(selectedId); // re-fetch the (now smaller) entity
    } catch (e) {
      setError(String(e));
    }
  }

  async function addFact(e: React.FormEvent) {
    e.preventDefault();
    if (!selectedId || !newFactKey.trim()) return;
    setError(null);
    try {
      const detail = await entityAddFact(selectedId, [{ key: newFactKey, value: newFactValue }]);
      setSelected(detail);
      setNewFactKey("");
      setNewFactValue("");
      setAddFactOpen(false);
      onEntitiesChanged(); // bumps the Overview screen's fact_count
    } catch (e) {
      setError(String(e));
    }
  }

  async function addRelationship(e: React.FormEvent) {
    e.preventDefault();
    if (!selectedId || !relToId) return;
    setError(null);
    const effectiveType = relType === CUSTOM_REL_TYPE ? customRelType.trim() : relType;
    if (!effectiveType) return;
    try {
      await relationshipAdd(selectedId, relToId, effectiveType, []);
      setRelToId("");
      setCustomRelType("");
      const rels = await relationshipList();
      setRelationships(rels);
      onEntitiesChanged(); // bumps the Overview screen's relationship_count/fact_count
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

  const selStyle = selected ? styleForEntityType(selected.entity_type) : null;
  const entityById = new Map((entities ?? []).map((e) => [e.id, e]));
  const relevantLinks = (relationships ?? []).filter(
    (r) => r.from === selectedId || r.to === selectedId,
  );

  return (
    <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
      <div style={{ width: 372, flex: "none", borderRight: "1px solid var(--border-subtle)", display: "flex", flexDirection: "column", minHeight: 0 }}>
        <div style={{ padding: "14px 14px 10px", borderBottom: "1px solid var(--border-subtle)", display: "flex", flexDirection: "column", gap: 10 }}>
          <div className="row">
            <input
              style={{ flex: 1 }}
              value={query}
              onChange={(e) => setQuery(e.currentTarget.value)}
              placeholder="Search entities"
            />
            <button className="btn" title="Add entity" onClick={() => setAddOpen((v) => !v)}>
              +
            </button>
          </div>
          <div style={{ display: "flex", gap: 5, flexWrap: "wrap" }}>
            {["All", ...ENTITY_TYPES].map((t) => (
              <button
                key={t}
                className={"chip" + (typeFilter === t ? " active" : "")}
                onClick={() => setTypeFilter(t)}
              >
                {t}
              </button>
            ))}
          </div>
        </div>

        {addOpen && (
          <form className="col" style={{ padding: "13px 14px", borderBottom: "1px solid var(--border-subtle)", background: "#191921" }} onSubmit={addEntity}>
            <div className="field-label" style={{ color: "var(--accent)" }}>
              New entity
            </div>
            <div className="row">
              <select style={{ width: 130 }} value={newType} onChange={(e) => setNewType(e.currentTarget.value)}>
                {ENTITY_TYPES.map((t) => (
                  <option key={t}>{t}</option>
                ))}
              </select>
              <input style={{ flex: 1 }} className="mono" value={newKey} onChange={(e) => setNewKey(e.currentTarget.value)} placeholder="Canonical key" />
            </div>
            <div className="row">
              <input style={{ flex: 1 }} value={newAttrKey} onChange={(e) => setNewAttrKey(e.currentTarget.value)} placeholder="Attribute (optional)" />
              <input style={{ flex: 1 }} value={newAttrValue} onChange={(e) => setNewAttrValue(e.currentTarget.value)} placeholder="Value" />
            </div>
            <div className="row" style={{ justifyContent: "flex-end" }}>
              <button type="button" className="btn btn-ghost" onClick={() => setAddOpen(false)}>
                Cancel
              </button>
              <button type="submit" className="btn btn-primary">
                Add entity
              </button>
            </div>
          </form>
        )}

        {error && <p className="error-text" style={{ padding: "0 14px" }}>{error}</p>}

        <div style={{ flex: 1, overflow: "auto", padding: 6 }}>
          {filtered.map((e) => {
            const st = styleForEntityType(e.entity_type);
            const on = e.id === selectedId;
            return (
              <button
                key={e.id}
                onClick={() => selectEntity(e.id)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 11,
                  padding: "8px 9px",
                  borderRadius: 5,
                  cursor: "pointer",
                  borderLeft: "2px solid " + (on ? "var(--accent)" : "transparent"),
                  background: on ? "rgba(139,124,232,0.13)" : "transparent",
                  border: "none",
                  borderLeftWidth: 2,
                  width: "100%",
                  textAlign: "left",
                  color: "inherit",
                  font: "inherit",
                }}
              >
                <div className="badge" style={{ background: st.bg, color: st.fg }}>
                  {st.abbr}
                </div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 12.5, color: "var(--text)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                    {e.display_label}
                  </div>
                  <div className="mono" style={{ fontSize: 10.5, color: "var(--text-mono-muted)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                    {e.canonical_key ?? "—"}
                  </div>
                </div>
              </button>
            );
          })}
          {entities && filtered.length === 0 && <p className="muted" style={{ padding: 8 }}>No entities.</p>}
        </div>
      </div>

      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
        {!selected && (
          <div className="empty-state">
            <div>Select an entity to see its facts and links</div>
          </div>
        )}

        {selected && selStyle && (
          <>
            <div style={{ padding: "18px 24px 0", borderBottom: "1px solid var(--border-subtle)" }}>
              <div style={{ display: "flex", alignItems: "flex-start", gap: 14, marginBottom: 14 }}>
                <div className="badge badge-lg" style={{ background: selStyle.bg, color: selStyle.fg }}>
                  {selStyle.abbr}
                </div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
                    <h2 style={{ margin: 0, fontSize: 18, fontWeight: 600 }}>{selected.display_label}</h2>
                    <span className="status-pill" style={{ background: "#24242e", color: "var(--text-muted)" }}>
                      {selected.entity_type}
                    </span>
                  </div>
                  <div className="mono" style={{ fontSize: 11, color: "var(--text-mono-muted)", marginTop: 4 }}>
                    {selected.id}
                  </div>
                </div>
                <div className="row" style={{ flex: "none" }}>
                  <button
                    className={"btn" + (addFactOpen ? " btn-primary" : "")}
                    onClick={() => setAddFactOpen((v) => !v)}
                  >
                    Add fact…
                  </button>
                  <button className="btn" onClick={() => setMergeOpen((v) => !v)}>
                    Merge…
                  </button>
                  <button
                    className={"btn" + (splitMode ? " btn-primary" : "")}
                    onClick={() => {
                      setSplitMode((v) => !v);
                      setSplitFactIds(new Set());
                    }}
                  >
                    Split…
                  </button>
                </div>
              </div>

              {addFactOpen && (
                <form className="row" style={{ marginBottom: 14 }} onSubmit={addFact}>
                  <input
                    style={{ width: 160 }}
                    value={newFactKey}
                    onChange={(e) => setNewFactKey(e.currentTarget.value)}
                    placeholder="Attribute"
                  />
                  <input
                    style={{ flex: 1 }}
                    value={newFactValue}
                    onChange={(e) => setNewFactValue(e.currentTarget.value)}
                    placeholder="Value"
                  />
                  <button type="submit" className="btn btn-primary">
                    Add fact
                  </button>
                </form>
              )}

              {mergeOpen && (
                <form className="row" style={{ marginBottom: 14 }} onSubmit={doMerge}>
                  <input
                    className="mono"
                    style={{ flex: 1 }}
                    value={mergeOther}
                    onChange={(e) => setMergeOther(e.currentTarget.value)}
                    placeholder="Other entity id to merge into this one"
                  />
                  <button type="submit" className="btn btn-primary">
                    Merge
                  </button>
                </form>
              )}

              <div className="tabs">
                {(["facts", "links", "history"] as Tab[]).map((t) => (
                  <button key={t} className={"tab" + (tab === t ? " active" : "")} onClick={() => setTab(t)}>
                    {t[0].toUpperCase() + t.slice(1)}
                  </button>
                ))}
              </div>
            </div>

            <div style={{ flex: 1, overflow: "auto", padding: "18px 24px" }}>
              {tab === "facts" && (
                <>
                  {splitMode && (
                    <div style={{ display: "flex", alignItems: "center", gap: 12, padding: "9px 13px", marginBottom: 12, background: "rgba(139,124,232,0.1)", border: "1px solid var(--border-accent-subtle)", borderRadius: 6 }}>
                      <span style={{ fontSize: 12.5, color: "var(--accent-light)" }}>
                        {splitFactIds.size === 0
                          ? "Pick the facts that belong to a different entity."
                          : `${splitFactIds.size} fact${splitFactIds.size === 1 ? "" : "s"} will move to a new entity.`}
                      </span>
                      <button className="btn btn-primary btn-small" style={{ marginLeft: "auto" }} onClick={doSplit} disabled={splitFactIds.size === 0}>
                        Split into new entity
                      </button>
                    </div>
                  )}
                  <div className="table">
                    <div className="table-head" style={{ gridTemplateColumns: "26px 140px minmax(200px,1fr) 130px 110px" }}>
                      <div></div>
                      <div>Attribute</div>
                      <div>Value</div>
                      <div>Source</div>
                      <div>Collected</div>
                    </div>
                    {selected.attributes.map((f) => (
                      <div
                        key={f.fact_id}
                        className="table-row"
                        style={{ gridTemplateColumns: "26px 140px minmax(200px,1fr) 130px 110px", cursor: splitMode ? "pointer" : "default" }}
                        onClick={() => splitMode && toggleSplitFact(f.fact_id)}
                      >
                        <div style={{ display: "grid", placeItems: "center" }}>
                          {splitMode && (
                            <input
                              type="checkbox"
                              checked={splitFactIds.has(f.fact_id)}
                              onChange={() => toggleSplitFact(f.fact_id)}
                            />
                          )}
                        </div>
                        <div className="mono" style={{ fontSize: 11.5, color: "var(--text-dimmer)", display: "flex", alignItems: "center", gap: 6 }}>
                          {f.is_current && <span title="current" style={{ width: 5, height: 5, borderRadius: "50%", background: "var(--ok)" }} />}
                          {f.key}
                        </div>
                        <div style={{ fontSize: 12.5, color: f.is_current ? "var(--text)" : "var(--text-fainter)", display: "flex", gap: 7, minWidth: 0 }}>
                          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{f.value}</span>
                          {f.conflicting && (
                            <span className="status-pill" style={{ background: "rgba(217,161,59,0.15)", color: "var(--warn)", borderColor: "rgba(217,161,59,0.35)" }}>
                              conflict
                            </span>
                          )}
                        </div>
                        <div className="mono" style={{ fontSize: 11, color: "var(--text-faint)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          {f.source}
                        </div>
                        <div className="mono" style={{ fontSize: 11, color: "var(--text-mono-muted)" }}>
                          {new Date(f.collected_at_unix_ms).toISOString().slice(0, 16).replace("T", " ")}
                        </div>
                      </div>
                    ))}
                    {selected.attributes.length === 0 && <p className="muted" style={{ padding: 12 }}>No facts.</p>}
                  </div>
                </>
              )}

              {tab === "links" && (
                <div className="col" style={{ maxWidth: 720 }}>
                  {relevantLinks.length === 0 && <p className="muted">No relationships.</p>}
                  {relevantLinks.map((l) => {
                    const otherId = l.from === selectedId ? l.to : l.from;
                    const other = entityById.get(otherId);
                    return (
                      <div key={l.id} className="card" style={{ display: "flex", alignItems: "center", gap: 13 }}>
                        <span style={{ padding: "2px 8px", borderRadius: 3, fontSize: 10.5, background: "#24242e", color: "var(--accent-mid)", flex: "none" }} className="mono">
                          {l.relationship_type}
                        </span>
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ fontSize: 12.5, color: "var(--text)" }}>{other?.display_label ?? otherId}</div>
                          <div className="mono" style={{ fontSize: 10.5, color: "var(--text-mono-muted)" }}>{otherId}</div>
                        </div>
                      </div>
                    );
                  })}
                  <form className="col" onSubmit={addRelationship}>
                    <div className="row">
                      <EntityPicker
                        entities={entities ?? []}
                        value={relToId}
                        onChange={setRelToId}
                        excludeId={selectedId ?? undefined}
                        placeholder="Other entity…"
                      />
                      <select
                        style={{ width: 160 }}
                        value={relType}
                        onChange={(e) => setRelType(e.currentTarget.value)}
                      >
                        {RELATIONSHIP_TYPES.map((t) => (
                          <option key={t}>{t}</option>
                        ))}
                        <option value={CUSTOM_REL_TYPE}>Custom…</option>
                      </select>
                    </div>
                    {relType === CUSTOM_REL_TYPE && (
                      <input
                        style={{ maxWidth: 220 }}
                        value={customRelType}
                        onChange={(e) => setCustomRelType(e.currentTarget.value)}
                        placeholder="Custom relationship type"
                      />
                    )}
                    <button type="submit" className="btn" style={{ alignSelf: "flex-start" }}>
                      Add relationship…
                    </button>
                  </form>
                </div>
              )}

              {tab === "history" && (
                <div className="col" style={{ maxWidth: 720 }}>
                  {history && history.length === 0 && <p className="muted">No history.</p>}
                  {history?.map((h) => (
                    <div key={h.id} style={{ display: "flex", gap: 14, padding: "11px 0", borderBottom: "1px solid var(--border-row)" }}>
                      <span className="mono" style={{ fontSize: 11, color: "var(--text-mono-faint)", flex: "none", width: 140 }}>
                        {new Date(h.occurred_at_unix_ms).toISOString().slice(0, 16).replace("T", " ")}
                      </span>
                      <span style={{ fontSize: 12.5, color: "var(--text-dimmer)", flex: 1 }}>{h.description}</span>
                      <span className="mono" style={{ fontSize: 11, color: "var(--text-fainter)", flex: "none" }}>
                        {h.actor}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
