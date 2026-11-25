import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  dateBounds, DEFAULT_FILTERS, Field, FileResult, Filters, Hit, Kind, KINDS, parseTerms, SearchEvent, SearchOpts, splitList, Unreadable,
} from "./types";
import "./App.css";

type Status = "idle" | "searching" | "done" | "error";
type Mode = "single" | "bulk";
type Sort = "found" | "name" | "newest" | "hits";
type BulkTab = "all" | "found" | "missing";
interface Summary { scanned: number; matched: number; hits: number; elapsedMs: number; cancelled: boolean; errors: number }

const store = {
  get<T>(k: string, d: T): T {
    try { const v = localStorage.getItem("vq." + k); return v ? (JSON.parse(v) as T) : d; } catch { return d; }
  },
  set(k: string, v: unknown) { try { localStorage.setItem("vq." + k, JSON.stringify(v)); } catch { /* ignore */ } },
};

const fmtSize = (b: number) =>
  b < 1024 ? `${b} B` : b < 1048576 ? `${(b / 1024).toFixed(0)} KB` : `${(b / 1048576).toFixed(1)} MB`;
const fmtDate = (ms: number | null) =>
  ms ? new Date(ms).toLocaleDateString(undefined, { day: "2-digit", month: "short", year: "numeric" }) : "";
const fmtNum = (n: number) => n.toLocaleString();
const rowText = (h: Hit) => h.fields?.map((f) => `${f.name}: ${f.value}`).join(" | ") ?? h.ctx ?? "";

const Icon = {
  folder: <svg viewBox="0 0 24 24"><path d="M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2h9A1.5 1.5 0 0 1 21 8.5v9A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5z" /></svg>,
  copy: <svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2" /><path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8" /></svg>,
  filter: <svg viewBox="0 0 24 24"><path d="M4 5h16l-6 7.5V19l-4-2v-4.5z" /></svg>,
  warn: <svg viewBox="0 0 24 24"><path d="M12 4 2.5 20h19z" /><path d="M12 10v4.5M12 17.5v.01" /></svg>,
};

export default function App() {
  const [mode, setMode] = useState<Mode>(() => store.get("mode", "single"));
  const [root, setRoot] = useState<string>(() => store.get("root", ""));
  const [recent, setRecent] = useState<string[]>(() => store.get("recent", []));
  const [query, setQuery] = useState("");
  const [bulkText, setBulkText] = useState<string>(() => store.get("bulkText", ""));
  const [caseSensitive, setCase] = useState<boolean>(() => store.get("case", false));
  const [regex, setRegex] = useState<boolean>(() => store.get("regex", false));
  const [wholeWord, setWord] = useState<boolean>(() => store.get("word", false));
  const [kinds, setKinds] = useState<Kind[]>(() => store.get("kinds", KINDS.map((k) => k.id)));
  const [filters, setFilters] = useState<Filters>(() => ({ ...DEFAULT_FILTERS, ...store.get("filters", {}) }));
  const [showFilters, setShowFilters] = useState(false);

  const [status, setStatus] = useState<Status>("idle");
  const [error, setError] = useState("");
  const [progress, setProgress] = useState({ scanned: 0, total: 0 });
  const [results, setResults] = useState<FileResult[]>([]);
  const [unreadable, setUnreadable] = useState<Unreadable[]>([]);
  const [showUnreadable, setShowUnreadable] = useState(false);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [searchedTerms, setSearchedTerms] = useState<string[] | null>(null);
  const [sort, setSort] = useState<Sort>("found");
  const [bulkTab, setBulkTab] = useState<BulkTab>("all");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [toast, setToast] = useState("");

  const queryRef = useRef<HTMLInputElement>(null);
  const bulkRef = useRef<HTMLTextAreaElement>(null);
  const pending = useRef<FileResult[]>([]);
  const pendingBad = useRef<Unreadable[]>([]);
  const flushTimer = useRef<number | null>(null);
  const searchId = useRef(0);

  useEffect(() => { store.set("root", root); }, [root]);
  useEffect(() => { store.set("mode", mode); }, [mode]);
  useEffect(() => { store.set("bulkText", bulkText); }, [bulkText]);
  useEffect(() => { store.set("case", caseSensitive); store.set("regex", regex); store.set("word", wholeWord); }, [caseSensitive, regex, wholeWord]);
  useEffect(() => { store.set("kinds", kinds); }, [kinds]);
  useEffect(() => { store.set("filters", filters); }, [filters]);
  useEffect(() => { (mode === "single" ? queryRef.current : bulkRef.current)?.focus(); }, [mode]);

  const flash = (msg: string) => { setToast(msg); window.setTimeout(() => setToast(""), 2200); };
  const terms = useMemo(() => parseTerms(bulkText), [bulkText]);
  const activeFilters =
    (filters.date !== "any" ? 1 : 0) + (filters.include ? 1 : 0) + (filters.exclude ? 1 : 0) + (filters.excludeDirs ? 1 : 0) +
    (filters.maxMb !== DEFAULT_FILTERS.maxMb ? 1 : 0);

  // Matches can arrive hundreds per second: batch them into one render.
  const flush = useCallback(() => {
    flushTimer.current = null;
    if (pending.current.length) { const b = pending.current; pending.current = []; setResults((r) => r.concat(b)); }
    if (pendingBad.current.length) { const b = pendingBad.current; pendingBad.current = []; setUnreadable((r) => r.concat(b)); }
  }, []);
  const schedule = () => { if (flushTimer.current === null) flushTimer.current = window.setTimeout(flush, 60); };

  const pickFolder = async () => {
    const dir = await open({ directory: true, defaultPath: root || undefined, title: "Choose a folder to search" });
    if (typeof dir === "string") setRoot(dir);
  };

  const runSearch = async () => {
    const q = query.trim();
    if (mode === "single" && !q) { queryRef.current?.focus(); return; }
    if (mode === "bulk" && !terms.length) { bulkRef.current?.focus(); return; }
    if (!root.trim()) { await pickFolder(); return; }
    if (!kinds.length) { setError("Choose at least one file type."); setStatus("error"); return; }

    const id = ++searchId.current;
    pending.current = []; pendingBad.current = [];
    setResults([]); setUnreadable([]); setShowUnreadable(false); setExpanded(new Set());
    setSummary(null); setError(""); setProgress({ scanned: 0, total: 0 }); setStatus("searching");
    setSearchedTerms(mode === "bulk" ? terms : null);
    setBulkTab("all");
    const nextRecent = [root, ...recent.filter((r) => r !== root)].slice(0, 6);
    setRecent(nextRecent); store.set("recent", nextRecent);

    const ch = new Channel<SearchEvent>();
    ch.onmessage = (ev) => {
      if (id !== searchId.current) return; // stale search
      switch (ev.type) {
        case "started": setProgress({ scanned: 0, total: ev.total }); break;
        case "progress": setProgress({ scanned: ev.scanned, total: ev.total }); break;
        case "match": pending.current.push(ev.file); schedule(); break;
        case "unreadable": pendingBad.current.push({ path: ev.path, reason: ev.reason }); schedule(); break;
        case "done": flush(); setSummary(ev); setStatus("done"); break;
        case "failed": setError(ev.message); setStatus("error"); break;
      }
    };
    const [after, before] = dateBounds(filters);
    const opts: SearchOpts = {
      root: root.trim(), query: mode === "single" ? q : "", terms: mode === "bulk" ? terms : [],
      caseSensitive, regex: mode === "single" && regex, wholeWord, kinds,
      maxHitsPerFile: 50, maxFileMb: filters.maxMb || 300,
      modifiedAfter: after, modifiedBefore: before,
      nameInclude: splitList(filters.include), nameExclude: splitList(filters.exclude), excludeDirs: splitList(filters.excludeDirs),
    };
    try { await invoke("search", { opts, onEvent: ch }); }
    catch (e) { setError(String(e)); setStatus("error"); }
  };

  const cancel = () => invoke("cancel_search");

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && status === "searching") cancel();
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setMode("single"); queryRef.current?.select(); }
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); status === "searching" ? cancel() : runSearch(); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const sorted = useMemo(() => {
    const r = [...results];
    if (sort === "name") r.sort((a, b) => a.name.localeCompare(b.name));
    if (sort === "newest") r.sort((a, b) => (b.modified ?? 0) - (a.modified ?? 0));
    if (sort === "hits") r.sort((a, b) => b.hits.length - a.hits.length);
    return r;
  }, [results, sort]);

  // Bulk: term → [{file, hit}]
  const byTerm = useMemo(() => {
    if (!searchedTerms) return null;
    const m = new Map<string, { file: FileResult; hit: Hit }[]>();
    for (const t of searchedTerms) m.set(t, []);
    for (const f of sorted) for (const h of f.hits) if (h.term) m.get(h.term)?.push({ file: f, hit: h });
    return m;
  }, [searchedTerms, sorted]);
  const bulkCounts = useMemo(() => {
    if (!byTerm) return { found: 0, missing: 0 };
    let found = 0;
    byTerm.forEach((v) => { if (v.length) found++; });
    return { found, missing: byTerm.size - found };
  }, [byTerm]);

  const exportCsv = async () => {
    const label = searchedTerms ? `bulk ${searchedTerms.length} terms` : query.trim().slice(0, 40);
    const path = await save({ defaultPath: `VaultQuery - ${label}.csv`, filters: [{ name: "CSV", extensions: ["csv"] }] });
    if (!path) return;
    const esc = (s: string) => `"${String(s).replace(/"/g, '""')}"`;
    const rows: string[] = [];
    if (byTerm) {
      rows.push(["Term", "Status", "File", "Folder", "Location", "Row", "Modified"].map(esc).join(","));
      byTerm.forEach((hits, term) => {
        if (!hits.length) rows.push([term, "NOT FOUND", "", "", "", "", ""].map(esc).join(","));
        for (const { file, hit } of hits) rows.push([term, "found", file.name, file.dir, hit.loc, rowText(hit) || hit.pre + hit.mat + hit.post, fmtDate(file.modified)].map(esc).join(","));
      });
    } else {
      rows.push(["File", "Folder", "Location", "Match", "Row", "Modified"].map(esc).join(","));
      for (const f of sorted) for (const h of f.hits) rows.push([f.name, f.dir, h.loc, h.pre + h.mat + h.post, rowText(h), fmtDate(f.modified)].map(esc).join(","));
    }
    try { await invoke("export_csv", { path, content: rows.join("\r\n") }); flash(`Exported ${fmtNum(rows.length - 1)} rows`); }
    catch (e) { flash(`Export failed: ${e}`); }
  };

  const toggleKind = (k: Kind) => setKinds((ks) => (ks.includes(k) ? ks.filter((x) => x !== k) : [...ks, k]));
  const toggleExpand = (k: string) => setExpanded((s) => { const n = new Set(s); n.has(k) ? n.delete(k) : n.add(k); return n; });
  const setF = (p: Partial<Filters>) => setFilters((f) => ({ ...f, ...p }));
  const openFile = (path: string) => invoke("open_file", { path }).catch((e) => flash(String(e)));
  const reveal = (path: string) => invoke("reveal_file", { path }).catch((e) => flash(String(e)));
  const copy = (text: string, msg = "Copied") => { navigator.clipboard.writeText(text); flash(msg); };

  const pct = progress.total ? Math.round((progress.scanned / progress.total) * 100) : 0;
  const searching = status === "searching";

  return (
    <div className="app">
      <header className="top">
        <div className="brandRow">
          <div className="brand">
            <svg viewBox="0 0 24 24" className="logo" aria-hidden><circle cx="10.5" cy="10.5" r="6.5" /><path d="M15.5 15.5 21 21" /></svg>
            <span>VaultQuery</span>
          </div>
          <div className="modes" role="tablist">
            <button role="tab" className={mode === "single" ? "on" : ""} onClick={() => setMode("single")}>Search</button>
            <button role="tab" className={mode === "bulk" ? "on" : ""} onClick={() => setMode("bulk")}>Bulk lookup</button>
          </div>
        </div>

        {mode === "single" ? (
          <form className="searchbar" onSubmit={(e) => { e.preventDefault(); searching ? cancel() : runSearch(); }}>
            <input ref={queryRef} className="query" placeholder="Search inside files: device ID, phone number, plate, name…"
              value={query} onChange={(e) => setQuery(e.target.value)} spellCheck={false} />
            <div className="toggles">
              <button type="button" className={`tog ${caseSensitive ? "on" : ""}`} title="Match case" onClick={() => setCase(!caseSensitive)}>Aa</button>
              <button type="button" className={`tog ${wholeWord ? "on" : ""}`} title="Whole word" onClick={() => setWord(!wholeWord)}><u>ab</u></button>
              <button type="button" className={`tog ${regex ? "on" : ""}`} title="Regular expression" onClick={() => setRegex(!regex)}>.*</button>
            </div>
            <button className={`go ${searching ? "stop" : ""}`} type="submit">{searching ? "Stop" : "Search"}</button>
          </form>
        ) : (
          <div className="bulkbox">
            <textarea ref={bulkRef} value={bulkText} onChange={(e) => setBulkText(e.target.value)} spellCheck={false}
              placeholder={"Paste a list: one per line, or separated by commas\n356938035643809\n356938035643817\nKT 9012 DM"} />
            <div className="bulkside">
              <div className="termcount"><b>{fmtNum(terms.length)}</b> unique term{terms.length === 1 ? "" : "s"}</div>
              <div className="toggles">
                <button type="button" className={`tog ${caseSensitive ? "on" : ""}`} title="Match case" onClick={() => setCase(!caseSensitive)}>Aa</button>
                <button type="button" className={`tog ${wholeWord ? "on" : ""}`} title="Whole word" onClick={() => setWord(!wholeWord)}><u>ab</u></button>
              </div>
              <button className={`go ${searching ? "stop" : ""}`} onClick={() => (searching ? cancel() : runSearch())}>{searching ? "Stop" : "Look up all"}</button>
              <span className="hint">Ctrl+Enter</span>
            </div>
          </div>
        )}

        <div className="folderRow">
          <div className="folder" title={root}>
            {Icon.folder}
            <input className="folderPath" value={root} placeholder="Paste a folder path, or browse…" spellCheck={false}
              onChange={(e) => setRoot(e.target.value.replace(/^"|"$/g, ""))}
              onKeyDown={(e) => { if (e.key === "Enter") runSearch(); }} />
            <button className="browse" onClick={pickFolder}>Browse</button>
          </div>
          {recent.filter((r) => r !== root).slice(0, 3).map((r) => (
            <button key={r} className="chip ghost" onClick={() => setRoot(r)} title={r}>{r.split(/[\\/]/).pop() || r}</button>
          ))}
        </div>

        <div className="kinds">
          {KINDS.map((k) => (
            <button key={k.id} className={`chip kind-${k.id} ${kinds.includes(k.id) ? "on" : ""}`} onClick={() => toggleKind(k.id)} title={k.exts}>
              <span className="dot" />{k.label}
            </button>
          ))}
          <div className="spacer" />
          <button className={`chip ${showFilters || activeFilters ? "on" : ""}`} onClick={() => setShowFilters(!showFilters)}>
            {Icon.filter} Filters{activeFilters ? <span className="count">{activeFilters}</span> : null}
          </button>
        </div>

        {showFilters && (
          <div className="filters">
            <label>
              <span>Modified</span>
              <select value={filters.date} onChange={(e) => setF({ date: e.target.value as Filters["date"] })}>
                <option value="any">Any time</option>
                <option value="today">Today</option>
                <option value="7d">Last 7 days</option>
                <option value="30d">Last 30 days</option>
                <option value="90d">Last 90 days</option>
                <option value="year">This year</option>
                <option value="custom">Custom range…</option>
              </select>
              {filters.date === "custom" && (
                <span className="range">
                  <input type="date" value={filters.from} onChange={(e) => setF({ from: e.target.value })} />–
                  <input type="date" value={filters.to} onChange={(e) => setF({ to: e.target.value })} />
                </span>
              )}
            </label>
            <label><span>File name</span><input value={filters.include} onChange={(e) => setF({ include: e.target.value })} placeholder="e.g. *timesheet*, *.xlsx" /></label>
            <label><span>Skip names</span><input value={filters.exclude} onChange={(e) => setF({ exclude: e.target.value })} placeholder="e.g. backup, *copy*" /></label>
            <label><span>Skip folders</span><input value={filters.excludeDirs} onChange={(e) => setF({ excludeDirs: e.target.value })} placeholder="e.g. Archive, old*" /></label>
            <label className="small"><span>Max size</span><input type="number" min={1} value={filters.maxMb} onChange={(e) => setF({ maxMb: Number(e.target.value) })} /> MB</label>
            {activeFilters > 0 && <button className="link" onClick={() => setFilters(DEFAULT_FILTERS)}>Reset filters</button>}
          </div>
        )}
      </header>

      <div className="statusbar">
        {searching && (
          <>
            <div className="bar"><div style={{ width: `${pct}%` }} /></div>
            <span>Scanning {fmtNum(progress.scanned)} / {fmtNum(progress.total)} files · {fmtNum(results.length)} matched</span>
          </>
        )}
        {status === "done" && summary && (
          <span>
            {byTerm
              ? <><b>{fmtNum(bulkCounts.found)}</b> of {fmtNum(byTerm.size)} terms found · </>
              : null}
            <b>{fmtNum(summary.matched)}</b> files · <b>{fmtNum(summary.hits)}</b> matches · {fmtNum(summary.scanned)} files scanned in {(summary.elapsedMs / 1000).toFixed(1)} s
            {summary.cancelled && " · stopped"}
          </span>
        )}
        {status === "error" && <span className="err">{error}</span>}
        {status === "idle" && <span className="muted">Searches file contents on demand. No index, nothing running in the background.</span>}
        {unreadable.length > 0 && (
          <button className="chip warn" onClick={() => setShowUnreadable(!showUnreadable)}>{Icon.warn} {fmtNum(unreadable.length)} unreadable</button>
        )}
        <div className="spacer" />
        {results.length > 0 && (
          <>
            {!byTerm && (
              <select value={sort} onChange={(e) => setSort(e.target.value as Sort)} title="Sort">
                <option value="found">Order found</option>
                <option value="newest">Newest first</option>
                <option value="name">Name</option>
                <option value="hits">Most matches</option>
              </select>
            )}
            <button className="chip" onClick={exportCsv}>Export CSV</button>
          </>
        )}
        {status === "done" && byTerm && !results.length && <button className="chip" onClick={exportCsv}>Export CSV</button>}
      </div>

      {showUnreadable && (
        <div className="unreadable">
          <div className="uhead">
            <b>Files that could not be read</b>
            <span className="muted">These were skipped, so matches inside them may be missing.</span>
            <div className="spacer" />
            <button className="link" onClick={() => setShowUnreadable(false)}>Close</button>
          </div>
          <ul>
            {unreadable.map((u) => (
              <li key={u.path}>
                <span className="reason">{u.reason}</span>
                <button className="fname small" onClick={() => openFile(u.path)} title={u.path}>{u.path}</button>
                <button className="icon" title="Show in folder" onClick={() => reveal(u.path)}>{Icon.folder}</button>
              </li>
            ))}
          </ul>
        </div>
      )}

      <main className="results">
        {status === "idle" && <Empty mode={mode} />}
        {status === "done" && !results.length && !byTerm && (
          <div className="empty"><h2>No matches</h2><p>Nothing in {fmtNum(summary?.scanned ?? 0)} files contains <code>{query}</code>.</p></div>
        )}

        {byTerm ? (
          <BulkView byTerm={byTerm} tab={bulkTab} setTab={setBulkTab} counts={bulkCounts} expanded={expanded}
            toggle={toggleExpand} openFile={openFile} reveal={reveal} copy={copy} done={status === "done"} />
        ) : (
          sorted.map((f) => {
            const isOpen = expanded.has(f.path);
            const shown = isOpen ? f.hits : f.hits.slice(0, 3);
            return (
              <article key={f.path} className="file">
                <FileHead f={f} openFile={openFile} reveal={reveal} copy={copy} />
                <ul className="hits">{shown.map((h, i) => <HitRow key={i} h={h} />)}</ul>
                {f.hits.length > 3 && (
                  <button className="more" onClick={() => toggleExpand(f.path)}>
                    {isOpen ? "Show less" : `Show ${f.hits.length - 3}${f.truncated ? "+" : ""} more`}
                  </button>
                )}
              </article>
            );
          })
        )}
      </main>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

// ---------------------------------------------------------------- pieces

function FileHead({ f, openFile, reveal, copy }: { f: FileResult; openFile: (p: string) => void; reveal: (p: string) => void; copy: (t: string, m?: string) => void }) {
  return (
    <>
      <div className="fhead">
        <span className={`badge kind-${f.kind}`}>{f.name.split(".").pop()}</span>
        <button className="fname" onClick={() => openFile(f.path)} title="Open file">{f.name}</button>
        <span className="meta">{f.hits.length}{f.truncated ? "+" : ""} match{f.hits.length > 1 ? "es" : ""} · {fmtSize(f.size)} · {fmtDate(f.modified)}</span>
        <div className="spacer" />
        <button className="icon" title="Show in folder" onClick={() => reveal(f.path)}>{Icon.folder}</button>
        <button className="icon" title="Copy path" onClick={() => copy(f.path, "Path copied")}>{Icon.copy}</button>
      </div>
      <div className="fdir" title={f.dir}>{f.dir}</div>
    </>
  );
}

function Fields({ fields }: { fields: Field[] }) {
  const [all, setAll] = useState(false);
  const hitIdx = fields.findIndex((f) => f.hit);
  // show the matched field and its neighbours first
  const ordered = hitIdx < 0 ? fields : [fields[hitIdx], ...fields.filter((_, i) => i !== hitIdx)];
  const shown = all ? ordered : ordered.slice(0, 7);
  return (
    <div className="fields">
      {shown.map((f, i) => (
        <span key={i} className={`field ${f.hit ? "hit" : ""}`}><span className="fk">{f.name}</span><span className="fv">{f.value}</span></span>
      ))}
      {fields.length > 7 && <button className="link" onClick={() => setAll(!all)}>{all ? "less" : `+${fields.length - 7} more`}</button>}
    </div>
  );
}

function HitRow({ h, showFile }: { h: Hit; showFile?: React.ReactNode }) {
  return (
    <li>
      <span className="loc" title={h.loc}>{h.loc}</span>
      <div className="hit">
        {showFile}
        {h.fields?.length ? <Fields fields={h.fields} /> : (
          <>
            <div className="line">{h.pre}<mark>{h.mat}</mark>{h.post}</div>
            {h.ctx && <div className="ctx">{h.ctx}</div>}
          </>
        )}
      </div>
    </li>
  );
}

function BulkView(p: {
  byTerm: Map<string, { file: FileResult; hit: Hit }[]>; tab: BulkTab; setTab: (t: BulkTab) => void;
  counts: { found: number; missing: number }; expanded: Set<string>; toggle: (k: string) => void;
  openFile: (path: string) => void; reveal: (path: string) => void; copy: (t: string, m?: string) => void; done: boolean;
}) {
  const entries = [...p.byTerm.entries()].filter(([, v]) => (p.tab === "found" ? v.length : p.tab === "missing" ? !v.length : true));
  const missing = [...p.byTerm.entries()].filter(([, v]) => !v.length).map(([t]) => t);
  return (
    <div className="bulk">
      <div className="bulktabs">
        <button className={p.tab === "all" ? "on" : ""} onClick={() => p.setTab("all")}>All <span>{p.byTerm.size}</span></button>
        <button className={p.tab === "found" ? "on" : ""} onClick={() => p.setTab("found")}>Found <span className="ok">{p.counts.found}</span></button>
        <button className={p.tab === "missing" ? "on" : ""} onClick={() => p.setTab("missing")}>Not found <span className="no">{p.done ? p.counts.missing : "…"}</span></button>
        <div className="spacer" />
        {p.done && missing.length > 0 && <button className="link" onClick={() => p.copy(missing.join("\n"), `Copied ${missing.length} not-found terms`)}>Copy not-found list</button>}
      </div>
      <div className="termtable">
        {entries.map(([term, hits]) => {
          const files = new Set(hits.map((h) => h.file.path)).size;
          const open = p.expanded.has("t:" + term);
          const first = hits[0];
          return (
            <div key={term} className={`term ${hits.length ? "found" : p.done ? "missing" : "pending"}`}>
              <button className="trow" onClick={() => hits.length && p.toggle("t:" + term)}>
                <span className="tstate">{hits.length ? "✓" : p.done ? "✕" : "·"}</span>
                <span className="tname">{term}</span>
                <span className="tsum">
                  {hits.length
                    ? <>{files} file{files > 1 ? "s" : ""} · {first.file.name} <span className="muted">› {first.hit.loc}</span></>
                    : p.done ? "Not found in any file" : "searching…"}
                </span>
                {hits.length > 0 && <span className="chev">{open ? "▾" : "▸"}</span>}
              </button>
              {open && (
                <ul className="hits">
                  {hits.map(({ file, hit }, i) => (
                    <HitRow key={i} h={hit} showFile={
                      <div className="hitfile">
                        <span className={`badge kind-${file.kind}`}>{file.name.split(".").pop()}</span>
                        <button className="fname small" onClick={() => p.openFile(file.path)} title={file.path}>{file.name}</button>
                        <span className="meta">{fmtDate(file.modified)}</span>
                        <button className="icon" title="Show in folder" onClick={() => p.reveal(file.path)}>{Icon.folder}</button>
                      </div>
                    } />
                  ))}
                </ul>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Empty({ mode }: { mode: Mode }) {
  return mode === "single" ? (
    <div className="empty">
      <h2>Find text inside your files</h2>
      <p>Excel, CSV, Word, PowerPoint, PDF and even files inside zip archives, with the exact sheet, cell and row for every match.</p>
      <div className="tips">
        <div><kbd>Enter</kbd> search</div>
        <div><kbd>Esc</kbd> stop</div>
        <div><kbd>Ctrl</kbd>+<kbd>K</kbd> focus search</div>
        <div><code>.*</code> regex, e.g. <code>35693\d{"{10}"}</code></div>
      </div>
    </div>
  ) : (
    <div className="empty">
      <h2>Look up a whole list at once</h2>
      <p>Paste hundreds of IDs, phone numbers or plates. VaultQuery searches them all in one pass, then tells you where each one appears and which ones appear nowhere.</p>
      <div className="tips"><div><kbd>Ctrl</kbd>+<kbd>Enter</kbd> look up</div><div>Export the result, including the not-found list, to CSV</div></div>
    </div>
  );
}
