import { useEffect, useRef, useState } from "react";
import type {
  AttributeInput,
  AuditEvent,
  EntityDetail,
  EntityImageSummary,
  EntitySummary,
  RelationshipDto,
} from "../api";
import {
  entityAdd,
  entityAddFact,
  entityAddImage,
  entityAudit,
  entityGetImage,
  entityHide,
  entityList,
  entityListImages,
  entityMerge,
  entityShow,
  entitySplit,
  entityUnhide,
  factRedact,
  relationshipAdd,
  relationshipList,
} from "../api";
import { ENTITY_TYPES, RELATIONSHIP_TYPES, styleForEntityType } from "../entityStyle";
import EntityPicker from "../components/EntityPicker";
import { pickImageFile } from "../pickers";

type Tab = "facts" | "links" | "history" | "images";
const CUSTOM_REL_TYPE = "__custom__";

export default function EntitiesScreen({ onEntitiesChanged }: { onEntitiesChanged: () => void }) {
  const [entities, setEntities] = useState<EntitySummary[] | null>(null);
  const [typeFilter, setTypeFilter] = useState("All");
  const [showHidden, setShowHidden] = useState(false);
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

  const [hideOpen, setHideOpen] = useState(false);
  const [hideReason, setHideReason] = useState("");

  const [splitMode, setSplitMode] = useState(false);
  const [splitFactIds, setSplitFactIds] = useState<Set<string>>(new Set());
  const [splitType, setSplitType] = useState("Person");
  const [splitKey, setSplitKey] = useState("");
  // Last value toggleSplitFact itself put into splitKey — lets it keep
  // following the checkbox selection until the user types something of
  // their own, without needing to track a separate "touched" flag.
  const autoSplitKeyRef = useRef("");

  const [relationships, setRelationships] = useState<RelationshipDto[] | null>(null);
  const [relToId, setRelToId] = useState("");
  const [relType, setRelType] = useState(RELATIONSHIP_TYPES[2]); // "AssociatedWith"
  const [customRelType, setCustomRelType] = useState("");

  const [history, setHistory] = useState<AuditEvent[] | null>(null);

  const [images, setImages] = useState<EntityImageSummary[] | null>(null);
  const [imageDataById, setImageDataById] = useState<Map<string, string>>(new Map());

  async function refresh() {
    setError(null);
    try {
      const list = await entityList(typeFilter === "All" ? null : typeFilter, showHidden);
      setEntities(list);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [typeFilter, showHidden]);

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

  // Fetched whenever the selection changes, independent of which tab is
  // open — the header avatar (below) needs the current image's metadata
  // even when the Images tab was never opened.
  useEffect(() => {
    if (!selectedId) {
      setImages(null);
      return;
    }
    entityListImages(selectedId)
      .then(setImages)
      .catch((e) => setError(String(e)));
  }, [selectedId]);

  // Lazily fetches image bytes: always for the current (avatar) image,
  // and for every image once the Images tab is actually opened — a
  // gallery of a few avatar-sized images doesn't need pagination, but
  // there's no reason to fetch bytes for images nobody has looked at yet.
  useEffect(() => {
    if (!images) return;
    const toFetch = tab === "images" ? images : images.filter((i) => i.is_current);
    toFetch.forEach((img) => {
      if (imageDataById.has(img.id)) return;
      entityGetImage(img.id)
        .then((data) => {
          setImageDataById((prev) => {
            const next = new Map(prev);
            next.set(img.id, `data:${data.mime_type};base64,${data.data_base64}`);
            return next;
          });
        })
        .catch((e) => setError(String(e)));
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [images, tab]);

  function selectEntity(id: string) {
    setSelectedId(id);
    setTab("facts");
    setSplitMode(false);
    setSplitFactIds(new Set());
    setMergeOpen(false);
    setHideOpen(false);
    setHideReason("");
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

  async function doHide(e: React.FormEvent) {
    e.preventDefault();
    if (!selectedId) return;
    setError(null);
    try {
      await entityHide(selectedId, hideReason.trim() || null);
      setHideOpen(false);
      setHideReason("");
      await refresh();
      const detail = await entityShow(selectedId);
      setSelected(detail);
      onEntitiesChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doUnhide() {
    if (!selectedId) return;
    setError(null);
    try {
      await entityUnhide(selectedId);
      await refresh();
      const detail = await entityShow(selectedId);
      setSelected(detail);
      onEntitiesChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  async function doSplit() {
    if (!selectedId || splitFactIds.size === 0) return;
    setError(null);
    try {
      await entitySplit(selectedId, Array.from(splitFactIds), splitType, splitKey || null);
      setSplitMode(false);
      setSplitFactIds(new Set());
      setSplitKey("");
      autoSplitKeyRef.current = "";
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

  async function addImage() {
    if (!selectedId) return;
    setError(null);
    try {
      const path = await pickImageFile();
      if (!path) return; // user cancelled the dialog
      await entityAddImage(selectedId, path);
      const list = await entityListImages(selectedId);
      setImages(list);
      onEntitiesChanged(); // bumps the Overview screen's fact_count
    } catch (e) {
      setError(String(e));
    }
  }

  async function removeImage(image: EntityImageSummary) {
    if (!selectedId) return;
    setError(null);
    try {
      await factRedact(image.fact_id, "removed via GUI");
      const list = await entityListImages(selectedId);
      setImages(list);
      onEntitiesChanged();
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
    const next = new Set(splitFactIds);
    if (next.has(factId)) next.delete(factId);
    else next.add(factId);
    setSplitFactIds(next);

    // Pre-fills the key with the split-off value when it's unambiguous —
    // exactly one attribute among the selected facts (a fact can carry
    // several, and more than one fact can be selected). Only overwrites
    // what this function itself last put there, so it keeps tracking the
    // selection right up until the user types something of their own —
    // at which point their edit wins for the rest of this split session.
    if (selected && splitKey === autoSplitKeyRef.current) {
      const matching = selected.attributes.filter((a) => next.has(a.fact_id));
      const suggestion = matching.length === 1 ? matching[0].value : "";
      setSplitKey(suggestion);
      autoSplitKeyRef.current = suggestion;
    }
  }

  const selStyle = selected ? styleForEntityType(selected.entity_type) : null;
  const entityById = new Map((entities ?? []).map((e) => [e.id, e]));
  const relevantLinks = (relationships ?? []).filter(
    (r) => r.from === selectedId || r.to === selectedId,
  );
  const currentImage = images?.find((i) => i.is_current);
  const avatarUri = currentImage ? imageDataById.get(currentImage.id) : undefined;

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
          <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12, color: "var(--text-muted)" }}>
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => setShowHidden(e.currentTarget.checked)}
            />
            Show hidden
          </label>
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
                    {e.hidden && (
                      <span className="mono" style={{ marginLeft: 6, fontSize: 10, color: "var(--text-faint)" }}>
                        (hidden)
                      </span>
                    )}
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
                {avatarUri ? (
                  <img
                    src={avatarUri}
                    alt=""
                    className="badge-lg"
                    style={{ objectFit: "cover", flex: "none" }}
                  />
                ) : (
                  <div className="badge badge-lg" style={{ background: selStyle.bg, color: selStyle.fg }}>
                    {selStyle.abbr}
                  </div>
                )}
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
                    <h2 style={{ margin: 0, fontSize: 18, fontWeight: 600 }}>{selected.display_label}</h2>
                    <span className="status-pill" style={{ background: "#24242e", color: "var(--text-muted)" }}>
                      {selected.entity_type}
                    </span>
                    {selected.hidden && (
                      <span className="status-pill" style={{ background: "rgba(217,161,59,0.15)", color: "var(--warn)", borderColor: "rgba(217,161,59,0.35)" }}>
                        hidden
                      </span>
                    )}
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
                  <button className="btn" onClick={addImage}>
                    Add image…
                  </button>
                  <button className="btn" onClick={() => setMergeOpen((v) => !v)}>
                    Merge…
                  </button>
                  {selected.hidden ? (
                    <button className="btn" onClick={doUnhide}>
                      Unhide
                    </button>
                  ) : (
                    <button className="btn" onClick={() => setHideOpen((v) => !v)}>
                      Hide…
                    </button>
                  )}
                  <button
                    className={"btn" + (splitMode ? " btn-primary" : "")}
                    onClick={() => {
                      setSplitMode((v) => {
                        const next = !v;
                        // Defaults to the source's own type — same
                        // pre-fill-then-override pattern as newType above
                        // — but only when it's one of the fixed types this
                        // dropdown offers; a Custom(name) source falls back
                        // to the same baseline newType itself starts at.
                        if (next) {
                          setSplitType(
                            ENTITY_TYPES.includes(selected.entity_type)
                              ? selected.entity_type
                              : ENTITY_TYPES[0],
                          );
                        }
                        return next;
                      });
                      setSplitFactIds(new Set());
                      setSplitKey("");
                      autoSplitKeyRef.current = "";
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

              {hideOpen && (
                <form className="row" style={{ marginBottom: 14 }} onSubmit={doHide}>
                  <input
                    style={{ flex: 1 }}
                    value={hideReason}
                    onChange={(e) => setHideReason(e.currentTarget.value)}
                    placeholder="Reason (optional)"
                  />
                  <button type="submit" className="btn btn-primary">
                    Hide
                  </button>
                </form>
              )}

              <div className="tabs">
                {(["facts", "links", "history", "images"] as Tab[]).map((t) => (
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
                      <input
                        className="mono"
                        style={{ marginLeft: "auto", width: 140 }}
                        value={splitKey}
                        onChange={(e) => setSplitKey(e.currentTarget.value)}
                        placeholder="Key (for scanning)"
                      />
                      <select
                        className="btn-small"
                        style={{ width: 130 }}
                        value={splitType}
                        onChange={(e) => setSplitType(e.currentTarget.value)}
                      >
                        {ENTITY_TYPES.map((t) => (
                          <option key={t}>{t}</option>
                        ))}
                      </select>
                      <button className="btn btn-primary btn-small" onClick={doSplit} disabled={splitFactIds.size === 0}>
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

              {tab === "images" && (
                <div style={{ display: "flex", flexWrap: "wrap", gap: 14 }}>
                  {images && images.length === 0 && <p className="muted">No images.</p>}
                  {images?.map((img) => {
                    const uri = imageDataById.get(img.id);
                    return (
                      <div key={img.id} className="card" style={{ width: 160, padding: 10 }}>
                        <div
                          style={{
                            width: "100%",
                            height: 140,
                            borderRadius: 4,
                            overflow: "hidden",
                            background: "#191921",
                            display: "grid",
                            placeItems: "center",
                          }}
                        >
                          {uri ? (
                            <img src={uri} alt="" style={{ width: "100%", height: "100%", objectFit: "cover" }} />
                          ) : (
                            <span className="muted" style={{ fontSize: 11 }}>Loading…</span>
                          )}
                        </div>
                        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginTop: 8 }}>
                          <span className="mono" style={{ fontSize: 10.5, color: "var(--text-mono-muted)" }}>
                            {new Date(img.collected_at_unix_ms).toISOString().slice(0, 10)}
                          </span>
                          {img.is_current && (
                            <span className="status-pill" style={{ background: "rgba(60,166,110,0.15)", color: "var(--ok)" }}>
                              current
                            </span>
                          )}
                        </div>
                        <button
                          className="btn btn-small"
                          style={{ width: "100%", marginTop: 8 }}
                          onClick={() => removeImage(img)}
                        >
                          Remove
                        </button>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
