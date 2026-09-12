import { useEffect, useMemo, useState } from "react";
import type { EntitySummary, MapPoint } from "../api";
import { entityList, mapPoints } from "../api";
import { styleForEntityType } from "../entityStyle";
import worldLand from "../assets/worldLand110m.json";

// Equirectangular (Plate Carrée) projection — the simplest possible
// lat/lon -> pixel mapping, exactly proportioned (360:180 = 2:1) so land
// shapes aren't stretched. No projection library: same "hand-roll plain
// SVG, no external visualization dependency" precedent GraphScreen's own
// circleLayout already set, just applied to geographic coordinates
// instead of a relationship layout.
const W = 960;
const H = 480;
function project(lat: number, lon: number): { x: number; y: number } {
  return { x: ((lon + 180) / 360) * W, y: ((90 - lat) / 180) * H };
}

// Vendored from Natural Earth's 110m "land" polygons (public domain),
// flattened to a plain array of rings (each an array of [lon, lat]
// pairs) and rounded to 2 decimal places — plenty for an outline this
// small, and a quarter the size of the original GeoJSON. No network
// fetch, ever: bundled at build time like any other static asset, in
// keeping with the Map screen's whole point (SPEC.md §9.3) — a
// case's locations never get sent anywhere, including to a map-tile
// provider, to draw this screen.
type Ring = [number, number][];
const WORLD_LAND = worldLand as Ring[];

export default function MapScreen() {
  const [points, setPoints] = useState<MapPoint[] | null>(null);
  const [entities, setEntities] = useState<EntitySummary[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setError(null);
    Promise.all([mapPoints(), entityList(null, true)])
      .then(([p, e]) => {
        setPoints(p);
        setEntities(e);
      })
      .catch((e) => setError(String(e)));
  }, []);

  const byId = useMemo(() => new Map((entities ?? []).map((e) => [e.id, e])), [entities]);

  const landPaths = useMemo(
    () =>
      WORLD_LAND.map((ring) =>
        ring
          .map(([lon, lat], i) => {
            const { x, y } = project(lat, lon);
            return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
          })
          .join(" ") + "Z",
      ),
    [],
  );

  const selected = points?.find((p) => p.entity_id === selectedId) ?? null;

  return (
    <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", minHeight: 0 }}>
        <div style={{ flex: "none", padding: "13px 20px", borderBottom: "1px solid var(--border-subtle)", display: "flex", alignItems: "center", gap: 14 }}>
          <h2 style={{ margin: 0, fontSize: 15, fontWeight: 600, flex: "none" }}>Map</h2>
          <span style={{ fontSize: 11.5, color: "var(--text-mono-muted)", flex: "none" }}>
            {(points ?? []).length} location{(points ?? []).length === 1 ? "" : "s"} found
          </span>
        </div>

        {error && <p className="error-text" style={{ padding: 12 }}>{error}</p>}
        {points && points.length === 0 && (
          <p className="muted" style={{ padding: 12 }}>
            No locations found in this case yet. A Location entity with a known coordinate (e.g. from
            ip-lookup, or a photo's GPS data) will show up here as a pin.
          </p>
        )}

        <div style={{ flex: 1, minHeight: 0, background: "#10151c", position: "relative" }}>
          <svg
            viewBox={`0 0 ${W} ${H}`}
            preserveAspectRatio="xMidYMid meet"
            style={{ width: "100%", height: "100%", display: "block" }}
          >
            <rect x={0} y={0} width={W} height={H} fill="#10151c" onClick={() => setSelectedId(null)} />
            {landPaths.map((d, i) => (
              <path key={i} d={d} fill="#262b34" stroke="#343a46" strokeWidth={0.75} />
            ))}
            {(points ?? []).map((p) => {
              const { x, y } = project(p.lat, p.lon);
              const st = styleForEntityType(p.entity_type);
              const isSelected = p.entity_id === selectedId;
              return (
                <circle
                  key={p.entity_id}
                  cx={x}
                  cy={y}
                  r={isSelected ? 7 : 5}
                  fill={st.fg}
                  stroke="#10151c"
                  strokeWidth={1.5}
                  style={{ cursor: "pointer" }}
                  onClick={(e) => {
                    e.stopPropagation();
                    setSelectedId(p.entity_id);
                  }}
                >
                  <title>{p.display_label}</title>
                </circle>
              );
            })}
          </svg>
        </div>
      </div>

      {selected && (
        <div style={{ width: 296, flex: "none", borderLeft: "1px solid var(--border-subtle)", padding: 18, display: "flex", flexDirection: "column", gap: 16, overflow: "auto" }}>
          {(() => {
            const st = styleForEntityType(selected.entity_type);
            return (
              <>
                <div style={{ display: "flex", alignItems: "flex-start", gap: 11 }}>
                  <div className="badge" style={{ background: st.bg, color: st.fg, width: 32, height: 32, fontSize: 11 }}>
                    {st.abbr}
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 14, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {selected.display_label}
                    </div>
                    <div className="mono" style={{ fontSize: 10.5, color: "var(--text-mono-muted)", marginTop: 3 }}>
                      {selected.lat.toFixed(5)}, {selected.lon.toFixed(5)}
                    </div>
                  </div>
                </div>

                {selected.source === "location_entity" && (
                  <div>
                    <div className="field-label" style={{ marginBottom: 8 }}>
                      Located here ({selected.related_entity_ids.length})
                    </div>
                    {selected.related_entity_ids.length === 0 && (
                      <p className="muted" style={{ margin: 0, fontSize: 12 }}>
                        No entities are linked to this location yet.
                      </p>
                    )}
                    <div className="col">
                      {selected.related_entity_ids.map((id) => {
                        const re = byId.get(id);
                        if (!re) return null;
                        const rst = styleForEntityType(re.entity_type);
                        return (
                          <div
                            key={id}
                            className="card"
                            style={{ display: "flex", alignItems: "center", gap: 9, padding: "7px 9px" }}
                          >
                            <div className="badge" style={{ background: rst.bg, color: rst.fg, width: 20, height: 20, fontSize: 9 }}>
                              {rst.abbr}
                            </div>
                            <div style={{ flex: 1, minWidth: 0, fontSize: 12, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                              {re.display_label}
                            </div>
                          </div>
                        );
                      })}
                    </div>
                  </div>
                )}

                <button className="btn btn-ghost" style={{ marginTop: "auto" }} onClick={() => setSelectedId(null)}>
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
