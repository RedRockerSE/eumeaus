// Per-entity-type badge styling (abbreviation, background, foreground) —
// ported from the design's TYPE_STYLES map, extended to cover every real
// entity type in eumeaus-engine::EntityType (SPEC.md §4.3), not just the
// 7 the mockup's sample data happened to use. Custom(name) (§4.4's escape
// hatch) falls back to name's own first two letters on a neutral color.

export interface TypeStyle {
  abbr: string;
  bg: string;
  fg: string;
}

const TYPE_STYLES: Record<string, TypeStyle> = {
  Person: { abbr: "PE", bg: "rgba(139,124,232,0.16)", fg: "#a394f2" },
  Username: { abbr: "UN", bg: "rgba(111,185,138,0.14)", fg: "#6fb98a" },
  Email: { abbr: "EM", bg: "rgba(217,161,59,0.14)", fg: "#d9a13b" },
  PhoneNumber: { abbr: "PH", bg: "rgba(200,120,160,0.14)", fg: "#cc85a8" },
  Domain: { abbr: "DO", bg: "rgba(90,160,200,0.14)", fg: "#68a8cc" },
  IpAddress: { abbr: "IP", bg: "rgba(90,160,200,0.14)", fg: "#5a9fc4" },
  OnlineAccount: { abbr: "OA", bg: "rgba(111,185,138,0.10)", fg: "#5aa878" },
  Organization: { abbr: "OR", bg: "rgba(150,150,170,0.14)", fg: "#a0a0b4" },
  Location: { abbr: "LO", bg: "rgba(160,140,110,0.14)", fg: "#b39a76" },
  Document: { abbr: "DC", bg: "rgba(150,150,170,0.10)", fg: "#8f8fa4" },
  Image: { abbr: "IM", bg: "rgba(150,150,170,0.10)", fg: "#8f8fa4" },
  Vehicle: { abbr: "VE", bg: "rgba(217,161,59,0.10)", fg: "#c68f3a" },
  CryptoWallet: { abbr: "CW", bg: "rgba(217,161,59,0.16)", fg: "#e0b04a" },
  Url: { abbr: "UR", bg: "rgba(90,160,200,0.10)", fg: "#5a9fc4" },
};

const FALLBACK: TypeStyle = { abbr: "??", bg: "rgba(150,150,170,0.10)", fg: "#8f8fa4" };

export function styleForEntityType(entityType: string): TypeStyle {
  const known = TYPE_STYLES[entityType];
  if (known) return known;
  // Custom(name) — EntityType's Display prints "Custom(name)" verbatim.
  const m = /^Custom\((.+)\)$/.exec(entityType);
  const label = m ? m[1] : entityType;
  return { ...FALLBACK, abbr: label.slice(0, 2).toUpperCase() };
}

export const ENTITY_TYPES = Object.keys(TYPE_STYLES);
