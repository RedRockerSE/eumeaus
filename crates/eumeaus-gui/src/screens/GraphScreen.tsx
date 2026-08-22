import { useEffect, useMemo, useRef, useState } from "react";
import type { EntitySummary, RelationshipDto } from "../api";
import { entityList, entityListPositions, entitySetPosition, relationshipList } from "../api";
import { ENTITY_TYPES, isDirectionalRelationship, styleForEntityType } from "../entityStyle";

const ACCENT = "#8b7ce8";
const W = 900;
const H = 560;
const NODE_RADIUS = 20;
const FOCUS_RADIUS = 26;
// Below this many SVG user units of pointer movement, a press+release on
// a node is treated as a click (select/focus) rather than a drag — lets
// the existing "click a node to focus it" behavior keep working without
// a real physical-mouse-didn't-move check being defeated by sub-pixel
// jitter.
const DRAG_THRESHOLD = 4;

// The design's mock data came with hand-placed node coordinates; real
// case data has no such layout, so a circular layout is the starting
// point — simple, stable (same case always starts out the same way),
// and good enough for the entity counts this tool is meant for without
// a physics-simulation dependency this project doesn't have. Nodes are
// then draggable (see `positions` state below) to reposition from there;
// a drag persists via `entitySetPosition` (`entity_positions` table), so
// it survives the Graph screen remounting — this only seeds entities that
// have never been dragged (or are brand new).
function circleLayout(ids: string[]): Map<string, { x: number; y: number }> {
  const cx = W / 2;
  const cy = H / 2;
  const r = Math.min(W, H) / 2 - 80;
  const pos = new Map<string, { x: number; y: number }>();
  ids.forEach((id, i) => {
    const angle = (2 * Math.PI * i) / Math.max(1, ids.length) - Math.PI / 2;
    pos.set(id, { x: cx + r * Math.cos(angle), y: cy + r * Math.sin(angle) });
  });
  return pos;
}

type Point = { x: number; y: number };

// Pulls a relationship line's endpoints back from each node's center to its
// outer edge (radius ra/rb — a focused node renders larger, see FOCUS_RADIUS
// below, so the two ends can differ), so the line stops at the circle's
// outline instead of running underneath it and the node's own label. Falls
// back to the untrimmed points if the two nodes sit exactly on top of each
// other (distance 0) — pathological, but division by zero otherwise.
function trimToEdges(a: Point, ra: number, b: Point, rb: number) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const dist = Math.hypot(dx, dy);
  if (dist === 0) return { x1: a.x, y1: a.y, x2: b.x, y2: b.y };
  const ux = dx / dist;
  const uy = dy / dist;
  return {
    x1: a.x + ux * ra,
    y1: a.y + uy * ra,
    x2: b.x - ux * rb,
    y2: b.y - uy * rb,
  };
}

export default function GraphScreen() {
  const [entities, setEntities] = useState<EntitySummary[] | null>(null);
  const [relationships, setRelationships] = useState<RelationshipDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [typeFilter, setTypeFilter] = useState("All");
  const [focusId, setFocusId] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);

  const [positions, setPositions] = useState<Map<string, Point>>(new Map());
  const svgRef = useRef<SVGSVGElement>(null);
  const zoomGroupRef = useRef<SVGGElement>(null);
  const dragRef = useRef<{
    id: string;
    offsetX: number;
    offsetY: number;
    moved: boolean;
    lastX: number;
    lastY: number;
  } | null>(null);

  useEffect(() => {
    Promise.all([entityList(null), relationshipList(), entityListPositions()])
      .then(([e, r, pos]) => {
        setEntities(e);
        setRelationships(r);
        setPositions(new Map(pos.map((p) => [p.entity_id, { x: p.x, y: p.y }])));
      })
      .catch((e) => setError(String(e)));
  }, []);

  // Seeds a circle position for any entity that doesn't have one yet
  // (first load, or a node added since); leaves already-placed nodes —
  // including anything the user has dragged — untouched.
  useEffect(() => {
    if (!entities) return;
    setPositions((prev) => {
      const missing = entities.map((e) => e.id).filter((id) => !prev.has(id));
      if (missing.length === 0) return prev;
      const seeded = circleLayout(entities.map((e) => e.id));
      const next = new Map(prev);
      missing.forEach((id) => {
        const p = seeded.get(id);
        if (p) next.set(id, p);
      });
      return next;
    });
  }, [entities]);

  // Converts a pointer event's screen coordinates into the zoom group's
  // local coordinate space — the same space `positions` is stored in —
  // via the group's own screen CTM, so this stays correct regardless of
  // the SVG's rendered size (viewBox scaling) or the current zoom level.
  function localPoint(e: React.PointerEvent): Point | null {
    const svg = svgRef.current;
    const group = zoomGroupRef.current;
    if (!svg || !group) return null;
    const ctm = group.getScreenCTM();
    if (!ctm) return null;
    const pt = svg.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    const local = pt.matrixTransform(ctm.inverse());
    return { x: local.x, y: local.y };
  }

  function handleNodePointerDown(e: React.PointerEvent<SVGGElement>, id: string) {
    const p = positions.get(id);
    const local = localPoint(e);
    if (!p || !local) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragRef.current = {
      id,
      offsetX: p.x - local.x,
      offsetY: p.y - local.y,
      moved: false,
      lastX: p.x,
      lastY: p.y,
    };
  }

  function handleNodePointerMove(e: React.PointerEvent<SVGGElement>) {
    const drag = dragRef.current;
    if (!drag) return;
    const local = localPoint(e);
    if (!local) return;
    const nx = local.x + drag.offsetX;
    const ny = local.y + drag.offsetY;
    if (!drag.moved) {
      const p = positions.get(drag.id);
      if (p && Math.hypot(nx - p.x, ny - p.y) >= DRAG_THRESHOLD) drag.moved = true;
    }
    if (drag.moved) {
      drag.lastX = nx;
      drag.lastY = ny;
      setPositions((prev) => {
        const next = new Map(prev);
        next.set(drag.id, { x: nx, y: ny });
        return next;
      });
    }
  }

  // Persists only on release, not on every pointermove — a drag is one
  // Tauri call, not a stream of them.
  function handleNodePointerUp(e: React.PointerEvent<SVGGElement>, id: string) {
    e.currentTarget.releasePointerCapture(e.pointerId);
    const drag = dragRef.current;
    dragRef.current = null;
    if (!drag) return;
    if (!drag.moved) {
      setFocusId(id);
      return;
    }
    entitySetPosition(drag.id, drag.lastX, drag.lastY).catch((err) => setError(String(err)));
  }

  const adjacency = useMemo(() => {
    const m = new Map<string, { id: string; label: string }[]>();
    for (const r of relationships ?? []) {
      if (!m.has(r.from)) m.set(r.from, []);
      if (!m.has(r.to)) m.set(r.to, []);
      m.get(r.from)!.push({ id: r.to, label: r.relationship_type });
      m.get(r.to)!.push({ id: r.from, label: r.relationship_type });
    }
    return m;
  }, [relationships]);

  const byId = new Map((entities ?? []).map((e) => [e.id, e]));
  const neighbourIds = focusId ? (adjacency.get(focusId) ?? []).map((n) => n.id) : [];
  const typeOn = (e: EntitySummary) => typeFilter === "All" || e.entity_type === typeFilter;

  return (
    <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", minHeight: 0 }}>
        <div style={{ flex: "none", padding: "13px 20px", borderBottom: "1px solid var(--border-subtle)", display: "flex", alignItems: "center", gap: 14 }}>
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 600, flex: "none" }}>Link graph</h2>
          <span style={{ fontSize: 11.5, color: "var(--text-mono-muted)", flex: "none" }}>
            {(relationships ?? []).length} relationships across {(entities ?? []).length} entities
          </span>
          <div style={{ marginLeft: "auto", display: "flex", gap: 5, flexWrap: "wrap", justifyContent: "flex-end" }}>
            {["All", ...ENTITY_TYPES].map((t) => (
              <button key={t} className={"chip" + (typeFilter === t ? " active" : "")} onClick={() => setTypeFilter(t)}>
                {t}
              </button>
            ))}
          </div>
          <div style={{ flex: "none", display: "flex", gap: 4, marginLeft: 6 }}>
            <button className="btn btn-small" onClick={() => setZoom((z) => Math.max(0.6, +(z - 0.2).toFixed(2)))}>
              −
            </button>
            <button className="btn btn-small" onClick={() => setZoom((z) => Math.min(2, +(z + 0.2).toFixed(2)))}>
              +
            </button>
            <button className="btn btn-small" onClick={() => setZoom(1)}>
              Fit
            </button>
          </div>
        </div>

        {error && <p className="error-text" style={{ padding: 12 }}>{error}</p>}

        <div style={{ flex: 1, minHeight: 0, background: "#131318", position: "relative" }}>
          <svg
            ref={svgRef}
            viewBox={`0 0 ${W} ${H}`}
            preserveAspectRatio="xMidYMid meet"
            style={{ width: "100%", height: "100%", display: "block" }}
          >
            <defs>
              {/* refX=10 (the triangle's own tip) lands right on the line's
                  trimmed endpoint, which trimToEdges already placed at the
                  target node's outline — so the arrow points at, but never
                  overlaps, the circle. One marker per line color so it
                  matches whether the line itself is dimmed. */}
              <marker id="graph-arrow-touch" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="9" markerHeight="9" markerUnits="userSpaceOnUse" orient="auto">
                <path d="M0,0 L10,5 L0,10 z" fill="#6fb98a" />
              </marker>
              <marker id="graph-arrow-dim" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="9" markerHeight="9" markerUnits="userSpaceOnUse" orient="auto">
                <path d="M0,0 L10,5 L0,10 z" fill="#3a3a46" />
              </marker>
            </defs>
            <rect x={0} y={0} width={W} height={H} fill="transparent" onClick={() => setFocusId(null)} />
            <g
              ref={zoomGroupRef}
              transform={`translate(${((1 - zoom) * W) / 2},${((1 - zoom) * H) / 2}) scale(${zoom})`}
            >
              {(relationships ?? []).map((r) => {
                const a = positions.get(r.from);
                const b = positions.get(r.to);
                if (!a || !b) return null;
                const touches = !focusId || r.from === focusId || r.to === focusId;
                const ea = byId.get(r.from);
                const eb = byId.get(r.to);
                const visible = ea && eb && typeOn(ea) && typeOn(eb);
                const ra = r.from === focusId ? FOCUS_RADIUS : NODE_RADIUS;
                const rb = r.to === focusId ? FOCUS_RADIUS : NODE_RADIUS;
                const { x1, y1, x2, y2 } = trimToEdges(a, ra, b, rb);
                return (
                  <g key={r.id}>
                    <line
                      x1={x1}
                      y1={y1}
                      x2={x2}
                      y2={y2}
                      stroke={touches ? "#6fb98a" : "#3a3a46"}
                      strokeWidth={touches ? 1.8 : 1.2}
                      opacity={!visible ? 0.1 : touches ? 0.85 : 0.32}
                      markerEnd={
                        isDirectionalRelationship(r.relationship_type)
                          ? `url(#graph-arrow-${touches ? "touch" : "dim"})`
                          : undefined
                      }
                    />
                    <text
                      x={(a.x + b.x) / 2}
                      y={(a.y + b.y) / 2 - 6}
                      fill="#8b8b9c"
                      fontSize={10}
                      fontFamily="'JetBrains Mono', monospace"
                      textAnchor="middle"
                      opacity={touches && visible ? 0.9 : 0}
                    >
                      {r.relationship_type}
                    </text>
                  </g>
                );
              })}
              {(entities ?? []).map((e) => {
                const p = positions.get(e.id);
                if (!p) return null;
                const st = styleForEntityType(e.entity_type);
                const isFocus = e.id === focusId;
                const dim = !typeOn(e) ? 0.18 : !focusId || isFocus || neighbourIds.includes(e.id) ? 1 : 0.3;
                const r = isFocus ? FOCUS_RADIUS : NODE_RADIUS;
                return (
                  <g
                    key={e.id}
                    opacity={dim}
                    style={{ cursor: "grab", touchAction: "none" }}
                    onPointerDown={(ev) => handleNodePointerDown(ev, e.id)}
                    onPointerMove={handleNodePointerMove}
                    onPointerUp={(ev) => handleNodePointerUp(ev, e.id)}
                  >
                    {isFocus && <circle cx={p.x} cy={p.y} r={33} fill="none" stroke={ACCENT} strokeWidth={1.5} />}
                    <circle cx={p.x} cy={p.y} r={r} fill={st.bg} stroke={isFocus ? ACCENT : st.fg} strokeWidth={1.5} />
                    <text x={p.x} y={p.y + 4} fill={st.fg} fontSize={10.5} fontWeight={600} fontFamily="'JetBrains Mono', monospace" textAnchor="middle">
                      {st.abbr}
                    </text>
                    <text x={p.x} y={p.y + r + 15} fill={isFocus ? "#e8e8ee" : "#9b9baa"} fontSize={11.5} textAnchor="middle">
                      {e.display_label.length > 20 ? e.display_label.slice(0, 19) + "…" : e.display_label}
                    </text>
                  </g>
                );
              })}
            </g>
          </svg>
        </div>
      </div>

      {focusId && byId.get(focusId) && (
        <div style={{ width: 296, flex: "none", borderLeft: "1px solid var(--border-subtle)", padding: 18, display: "flex", flexDirection: "column", gap: 16, overflow: "auto" }}>
          {(() => {
            const e = byId.get(focusId)!;
            const st = styleForEntityType(e.entity_type);
            const nbs = adjacency.get(focusId) ?? [];
            return (
              <>
                <div style={{ display: "flex", alignItems: "flex-start", gap: 11 }}>
                  <div className="badge" style={{ background: st.bg, color: st.fg, width: 32, height: 32, fontSize: 11 }}>
                    {st.abbr}
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 14, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {e.display_label}
                    </div>
                    <div className="mono" style={{ fontSize: 10.5, color: "var(--text-mono-muted)", marginTop: 3 }}>
                      {e.id}
                    </div>
                  </div>
                </div>
                <div>
                  <div className="field-label" style={{ marginBottom: 8 }}>
                    Connected to ({nbs.length})
                  </div>
                  <div className="col">
                    {nbs.map((n, i) => {
                      const ne = byId.get(n.id);
                      if (!ne) return null;
                      const nst = styleForEntityType(ne.entity_type);
                      return (
                        <button
                          key={i}
                          className="card"
                          style={{ display: "flex", alignItems: "center", gap: 9, padding: "7px 9px", cursor: "pointer", textAlign: "left", font: "inherit", color: "inherit" }}
                          onClick={() => setFocusId(ne.id)}
                        >
                          <div className="badge" style={{ background: nst.bg, color: nst.fg, width: 20, height: 20, fontSize: 9 }}>
                            {nst.abbr}
                          </div>
                          <div style={{ flex: 1, minWidth: 0 }}>
                            <div style={{ fontSize: 12, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{ne.display_label}</div>
                            <div className="mono" style={{ fontSize: 10, color: "var(--text-mono-muted)" }}>{n.label}</div>
                          </div>
                        </button>
                      );
                    })}
                  </div>
                </div>
                <button className="btn btn-ghost" style={{ marginTop: "auto" }} onClick={() => setFocusId(null)}>
                  Clear selection
                </button>
              </>
            );
          })()}
        </div>
      )}
    </div>
  );
}
