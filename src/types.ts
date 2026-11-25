export type Kind = "text" | "spreadsheet" | "document" | "pdf" | "archive";

export interface Field {
  name: string;
  value: string;
  hit: boolean;
}

export interface Hit {
  loc: string;
  pre: string;
  mat: string;
  post: string;
  ctx: string | null;
  fields: Field[] | null;
  term: string | null;
}

export interface FileResult {
  path: string;
  name: string;
  dir: string;
  kind: Kind;
  size: number;
  modified: number | null;
  hits: Hit[];
  truncated: boolean;
}

export interface Unreadable {
  path: string;
  reason: string;
}

export type SearchEvent =
  | { type: "started"; total: number; bulk: boolean; terms: number }
  | { type: "progress"; scanned: number; total: number }
  | { type: "match"; file: FileResult }
  | ({ type: "unreadable" } & Unreadable)
  | { type: "done"; scanned: number; matched: number; hits: number; elapsedMs: number; cancelled: boolean; errors: number }
  | { type: "failed"; message: string };

export interface SearchOpts {
  root: string;
  query: string;
  terms: string[];
  caseSensitive: boolean;
  regex: boolean;
  wholeWord: boolean;
  kinds: Kind[];
  maxHitsPerFile: number;
  maxFileMb: number;
  modifiedAfter: number | null;
  modifiedBefore: number | null;
  nameInclude: string[];
  nameExclude: string[];
  excludeDirs: string[];
}

export const KINDS: { id: Kind; label: string; exts: string }[] = [
  { id: "spreadsheet", label: "Spreadsheets", exts: "xlsx · xlsm · xlsb · xls · ods" },
  { id: "text", label: "Text & data", exts: "csv · txt · json · log · xml · kml …" },
  { id: "document", label: "Word & PowerPoint", exts: "docx · pptx" },
  { id: "pdf", label: "PDF", exts: "pdf" },
  { id: "archive", label: "Inside zip", exts: "zip" },
];

export type DatePreset = "any" | "today" | "7d" | "30d" | "90d" | "year" | "custom";

export interface Filters {
  date: DatePreset;
  from: string; // yyyy-mm-dd (custom)
  to: string;
  include: string;
  exclude: string;
  excludeDirs: string;
  maxMb: number;
}

export const DEFAULT_FILTERS: Filters = { date: "any", from: "", to: "", include: "", exclude: "", excludeDirs: "", maxMb: 300 };

/** Turn the date preset into [after, before) unix-ms bounds. */
export function dateBounds(f: Filters): [number | null, number | null] {
  const now = new Date();
  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const day = 86_400_000;
  switch (f.date) {
    case "today": return [startOfDay(now), null];
    case "7d": return [startOfDay(now) - 6 * day, null];
    case "30d": return [startOfDay(now) - 29 * day, null];
    case "90d": return [startOfDay(now) - 89 * day, null];
    case "year": return [new Date(now.getFullYear(), 0, 1).getTime(), null];
    case "custom": {
      const a = f.from ? new Date(f.from + "T00:00:00").getTime() : null;
      const b = f.to ? new Date(f.to + "T00:00:00").getTime() + day : null;
      return [a, b];
    }
    default: return [null, null];
  }
}

export const splitList = (s: string) => s.split(/[,;\n]/).map((x) => x.trim()).filter(Boolean);

/** Bulk terms: one per line, or separated by comma / semicolon / tab. Spaces are kept (plates, names). */
export const parseTerms = (s: string) => {
  const seen = new Set<string>();
  return s
    .split(/[\n,;\t]/)
    .map((x) => x.trim())
    .filter((x) => x && !seen.has(x.toLowerCase()) && seen.add(x.toLowerCase()));
};
