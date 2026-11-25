//! VaultQuery search engine.
//!
//! Walks a folder and searches inside file *contents* on demand (no index).
//! Supports plain text/CSV, spreadsheets (xlsx/xlsm/xlsb/xls/ods), Office documents (docx/pptx),
//! PDF, and files inside zip archives. Two modes:
//! - single query (literal or regex)
//! - bulk: many literal terms at once (e.g. a pasted list of IDs); every hit is tagged with its term

use calamine::{open_workbook_auto, open_workbook_auto_from_rs, Data, Reader, Sheets};
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Seek};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};
use walkdir::WalkDir;

const DEFAULT_SKIP_DIRS: &[&str] = &["node_modules", ".git", "__pycache__", "$RECYCLE.BIN", "System Volume Information"];
const MAX_ZIP_ENTRIES: usize = 5_000;
const MAX_ZIP_ENTRY_BYTES: u64 = 200 * 1024 * 1024;
const BULK_HITS_PER_TERM: usize = 3;
const BULK_HITS_PER_FILE: usize = 2_000;
const MAX_FIELDS: usize = 40;

// ================================================================ types

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchOpts {
    pub root: String,
    /// Single-query mode.
    pub query: String,
    /// Bulk mode: when non-empty, `query` and `regex` are ignored and every term is searched literally.
    pub terms: Vec<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
    /// enabled kinds: "text", "spreadsheet", "document", "pdf", "archive"
    pub kinds: Vec<String>,
    pub max_hits_per_file: usize,
    pub max_file_mb: u64,
    /// Only files modified at/after this time (unix ms).
    pub modified_after: Option<u64>,
    /// Only files modified before this time (unix ms).
    pub modified_before: Option<u64>,
    /// File-name patterns to include (any match). `*`/`?` wildcards; plain text means "contains".
    pub name_include: Vec<String>,
    /// File-name patterns to exclude.
    pub name_exclude: Vec<String>,
    /// Folder-name patterns to skip entirely.
    pub exclude_dirs: Vec<String>,
}

impl SearchOpts {
    pub fn is_bulk(&self) -> bool {
        self.terms.iter().any(|t| !t.trim().is_empty())
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    pub value: String,
    /// true for the cell/column that contains the match
    pub hit: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// Human readable location, e.g. "Sheet1 ! D8", "Line 12", "Slide 3", "Page 2"
    pub loc: String,
    pub pre: String,
    pub mat: String,
    pub post: String,
    /// The whole row (spreadsheets / CSV) as plain text.
    pub ctx: Option<String>,
    /// The row as header → value pairs, when a header row could be detected.
    pub fields: Option<Vec<Field>>,
    /// Bulk mode: which of the input terms this hit belongs to.
    pub term: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub kind: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub hits: Vec<Hit>,
    pub truncated: bool,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SearchEvent {
    Started { total: usize, bulk: bool, terms: usize },
    Progress { scanned: usize, total: usize },
    Match { file: FileResult },
    Unreadable { path: String, reason: String },
    Done { scanned: usize, matched: usize, hits: usize, elapsed_ms: u64, cancelled: bool, errors: usize },
    Failed { message: String },
}

// ================================================================ small helpers

pub fn kind_of(ext: &str) -> Option<&'static str> {
    match ext {
        "csv" | "tsv" | "txt" | "json" | "log" | "xml" | "kml" | "geojson" | "html" | "htm" | "md" | "sql"
        | "ini" | "yaml" | "yml" | "js" | "ts" | "py" | "cfg" | "conf" | "nmea" => Some("text"),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => Some("spreadsheet"),
        "docx" | "pptx" => Some("document"),
        "pdf" => Some("pdf"),
        "zip" => Some("archive"),
        _ => None,
    }
}

pub fn ext_of(name: &str) -> String {
    Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Case-insensitive wildcard match (`*`, `?`). A pattern without wildcards matches as "contains".
pub fn name_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.trim().to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    if p.is_empty() {
        return false;
    }
    if !p.iter().any(|c| *c == '*' || *c == '?') {
        let ps: String = p.iter().collect();
        return name.to_lowercase().contains(&ps);
    }
    // iterative glob with backtracking on the last '*'
    let (mut pi, mut ni, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = ni;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn clean_terms(o: &SearchOpts) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    o.terms
        .iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .filter(|t| seen.insert(norm(t, o.case_sensitive)))
        .collect()
}

fn norm(s: &str, case_sensitive: bool) -> String {
    if case_sensitive { s.to_string() } else { s.to_lowercase() }
}

pub fn build_regex(o: &SearchOpts) -> Result<Regex, String> {
    let mut pat = if o.is_bulk() {
        let mut terms = clean_terms(o);
        terms.sort_by_key(|t| std::cmp::Reverse(t.len())); // prefer longest term on overlap
        terms.iter().map(|t| regex::escape(t)).collect::<Vec<_>>().join("|")
    } else {
        let q = o.query.trim();
        if q.is_empty() {
            return Err("Query is empty".into());
        }
        if o.regex { q.to_string() } else { regex::escape(q) }
    };
    if o.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    RegexBuilder::new(&pat)
        .case_insensitive(!o.case_sensitive)
        .size_limit(256 * 1024 * 1024)
        .build()
        .map_err(|e| format!("Invalid regex: {e}"))
}

fn tail_chars(s: &str, n: usize) -> String {
    let s = s.trim_start();
    match s.char_indices().rev().nth(n) {
        Some((i, _)) => format!("…{}", &s[i + s[i..].chars().next().map_or(0, |c| c.len_utf8())..]),
        None => s.to_string(),
    }
}

fn head_chars(s: &str, n: usize) -> String {
    let s = s.trim_end();
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\r'], " ")
}

/// Decode bytes as text: UTF-8 (with/without BOM) and UTF-16 LE/BE.
pub fn decode(bytes: &[u8]) -> String {
    fn utf16(b: &[u8], le: bool) -> String {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    }
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => {
            // BOM-less UTF-16 LE heuristic: most odd bytes are NUL
            let probe = &bytes[..bytes.len().min(2000)];
            let pairs = probe.len() / 2;
            let odd_nuls = probe.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
            if pairs > 10 && odd_nuls * 10 > pairs * 6 {
                utf16(bytes, true)
            } else {
                String::from_utf8_lossy(bytes).into_owned()
            }
        }
    }
}

/// Minimal XML → text: paragraph ends become newlines, tags removed, entities unescaped.
pub fn xml_to_text(xml: &str) -> String {
    let xml = xml.replace("</w:p>", "\n").replace("</a:p>", "\n").replace("<w:tab/>", "\t").replace("<w:br/>", "\n");
    let mut out = String::with_capacity(xml.len() / 3);
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    c += 1;
    while c > 0 {
        s.push(b'A' + ((c - 1) % 26) as u8);
        c = (c - 1) / 26;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

pub fn cell_str(d: &Data) -> Option<String> {
    match d {
        Data::Empty => None,
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e16 => Some(format!("{}", *f as i64)),
        Data::String(s) if s.trim().is_empty() => None,
        other => Some(other.to_string()),
    }
}

fn is_texty(s: &str) -> bool {
    s.chars().any(|c| c.is_alphabetic()) && s.trim().parse::<f64>().is_err()
}

/// Header row = the first row within the top 30 that is mostly text with ≥2 filled cells.
fn detect_header(rows: &[Vec<Option<String>>]) -> Option<usize> {
    rows.iter().take(30).position(|row| {
        let filled: Vec<&String> = row.iter().flatten().collect();
        let texty = filled.iter().filter(|s| is_texty(s)).count();
        texty >= 2 && texty * 10 >= filled.len() * 6
    })
}

fn row_fields(header: &[Option<String>], row: &[Option<String>], col0: u32, hit_col: usize) -> Vec<Field> {
    row.iter()
        .enumerate()
        .filter_map(|(c, v)| {
            let value = v.clone()?;
            let name = header
                .get(c)
                .cloned()
                .flatten()
                .unwrap_or_else(|| col_name(col0 + c as u32));
            Some(Field { name: clean(&head_chars(&name, 40)), value: clean(&head_chars(&value, 120)), hit: c == hit_col })
        })
        .take(MAX_FIELDS)
        .collect()
}

/// Split one CSV line, honouring double quotes.
pub fn split_csv(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delim && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

fn detect_delim(first_line: &str) -> char {
    [',', ';', '\t', '|'].into_iter().max_by_key(|d| first_line.matches(*d).count()).unwrap_or(',')
}

fn friendly_error(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("password") || l.contains("encrypt") {
        "Password-protected".into()
    } else if l.contains("access is denied") || l.contains("permission") || l.contains("os error 5") {
        "Access denied".into()
    } else if l.contains("being used by another process") || l.contains("os error 32") {
        "File is locked by another program".into()
    } else if l.contains("crashed") {
        "Unsupported or damaged PDF".into()
    } else {
        format!("Unreadable: {}", head_chars(e.lines().next().unwrap_or(e), 120))
    }
}

// ================================================================ collector

pub struct Collector<'a> {
    rx: &'a Regex,
    max: usize,
    /// bulk mode: normalized matched text → original term
    terms: Option<&'a HashMap<String, String>>,
    case_sensitive: bool,
    per_term: HashMap<String, usize>,
    pub hits: Vec<Hit>,
    pub truncated: bool,
}

impl<'a> Collector<'a> {
    pub fn new(rx: &'a Regex, max: usize) -> Self {
        Self { rx, max: max.max(1), terms: None, case_sensitive: false, per_term: HashMap::new(), hits: Vec::new(), truncated: false }
    }

    pub fn bulk(rx: &'a Regex, terms: &'a HashMap<String, String>, case_sensitive: bool) -> Self {
        Self { terms: Some(terms), case_sensitive, ..Self::new(rx, BULK_HITS_PER_FILE) }
    }

    fn full(&mut self) -> bool {
        if self.hits.len() >= self.max {
            self.truncated = true;
            true
        } else {
            false
        }
    }

    /// Bulk: resolve which term a match belongs to and enforce the per-term cap.
    /// Returns None when this match should be skipped.
    fn accept(&mut self, matched: &str) -> Option<Option<String>> {
        let Some(map) = self.terms else { return Some(None) };
        let term = map.get(&norm(matched, self.case_sensitive))?.clone();
        let n = self.per_term.entry(term.clone()).or_insert(0);
        if *n >= BULK_HITS_PER_TERM {
            self.truncated = true;
            return None;
        }
        *n += 1;
        Some(Some(term))
    }

    /// Search a block of text. `loc` builds the location label from the 1-based line number.
    /// With `csv`, lines are also split into header → value fields.
    pub fn text(&mut self, text: &str, loc: &dyn Fn(usize) -> String, csv: bool) {
        let (delim, header) = if csv {
            let first = text.lines().next().unwrap_or("");
            let d = detect_delim(first);
            let h: Vec<Option<String>> = split_csv(first, d).into_iter().map(|s| (!s.is_empty()).then_some(s)).collect();
            let texty = h.iter().flatten().filter(|s| is_texty(s)).count();
            (d, (texty >= 2).then_some(h))
        } else {
            (',', None)
        };
        let mut line = 1usize;
        let mut last = 0usize;
        for m in self.rx.find_iter(text) {
            if m.start() == m.end() {
                continue;
            }
            if self.full() {
                break;
            }
            line += text[last..m.start()].bytes().filter(|b| *b == b'\n').count();
            last = m.start();
            let Some(term) = self.accept(m.as_str()) else { continue };
            let ls = text[..m.start()].rfind('\n').map_or(0, |i| i + 1);
            let le = text[m.end()..].find('\n').map_or(text.len(), |i| m.end() + i);
            let fields = header.as_ref().filter(|_| line > 1).map(|h| {
                let cells = split_csv(&text[ls..le], delim);
                // which column holds the match: count delimiters before it (good enough for unquoted data)
                let col = text[ls..m.start()].matches(delim).count();
                let row: Vec<Option<String>> = cells.into_iter().map(|s| (!s.is_empty()).then_some(s)).collect();
                row_fields(h, &row, 0, col)
            });
            self.hits.push(Hit {
                loc: loc(line),
                pre: clean(&tail_chars(&text[ls..m.start()], 80)),
                mat: clean(&head_chars(m.as_str(), 200)),
                post: clean(&head_chars(&text[m.end()..le], 120)),
                ctx: None,
                fields,
                term,
            });
        }
    }

    pub fn sheets<RS: Read + Seek>(&mut self, wb: &mut Sheets<RS>, prefix: &str) {
        for name in wb.sheet_names() {
            if self.full() {
                return;
            }
            let Ok(range) = wb.worksheet_range(&name) else { continue };
            let (r0, c0) = range.start().unwrap_or((0, 0));
            // stringify lazily: only the first rows (for header detection) until a match is found
            let mut header: Option<Option<(usize, Vec<Option<String>>)>> = None;
            for (r, row) in range.rows().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    let Some(s) = cell_str(cell) else { continue };
                    let Some(m) = self.rx.find(&s).filter(|m| m.start() != m.end()) else { continue };
                    if self.full() {
                        return;
                    }
                    let Some(term) = self.accept(m.as_str()) else { continue };
                    let hdr = header.get_or_insert_with(|| {
                        let top: Vec<Vec<Option<String>>> =
                            range.rows().take(30).map(|row| row.iter().map(cell_str).collect()).collect();
                        detect_header(&top).map(|i| (i, top[i].clone()))
                    });
                    let row_s: Vec<Option<String>> = row.iter().map(cell_str).collect();
                    let fields = match hdr {
                        Some((hi, h)) if *hi < r => Some(row_fields(h, &row_s, c0, c)),
                        _ => None,
                    };
                    let ctx: Vec<&String> = row_s.iter().flatten().collect();
                    self.hits.push(Hit {
                        loc: format!("{prefix}{name} ! {}{}", col_name(c0 + c as u32), r0 as usize + r + 1),
                        pre: clean(&tail_chars(&s[..m.start()], 60)),
                        mat: clean(&head_chars(m.as_str(), 200)),
                        post: clean(&head_chars(&s[m.end()..], 80)),
                        ctx: Some(clean(&head_chars(&ctx.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("  │  "), 260))),
                        fields,
                        term,
                    });
                }
            }
        }
    }
}

// ================================================================ per-format scanners

fn read_entry<R: Read + Seek>(z: &mut zip::ZipArchive<R>, idx: usize) -> Option<String> {
    let mut f = z.by_index(idx).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn scan_ooxml_doc(bytes: &[u8], ext: &str, prefix: &str, c: &mut Collector) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let names: Vec<(usize, String)> =
        (0..z.len()).filter_map(|i| Some((i, z.by_index(i).ok()?.name().ok()?.to_string()))).collect();
    if ext == "docx" {
        if let Some((idx, _)) = names.iter().find(|(_, n)| n == "word/document.xml") {
            if let Some(xml) = read_entry(&mut z, *idx) {
                c.text(&xml_to_text(&xml), &|l| format!("{prefix}Paragraph {l}"), false);
            }
        }
    } else {
        let mut slides: Vec<(usize, usize)> = names
            .iter()
            .filter_map(|(i, n)| Some((n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?, *i)))
            .collect();
        slides.sort();
        for (num, idx) in slides {
            if let Some(xml) = read_entry(&mut z, idx) {
                c.text(&xml_to_text(&xml), &|_| format!("{prefix}Slide {num}"), false);
            }
        }
    }
    Ok(())
}

fn scan_pdf(bytes: &[u8], prefix: &str, c: &mut Collector) -> Result<(), String> {
    // pdf-extract can panic on unusual files; never let one file take the app down.
    let pages = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text_from_mem_by_pages(bytes)))
        .map_err(|_| "pdf parser crashed".to_string())?
        .map_err(|e| e.to_string())?;
    for (i, page) in pages.iter().enumerate() {
        c.text(page, &|_| format!("{prefix}Page {}", i + 1), false);
    }
    Ok(())
}

fn scan_bytes(bytes: Vec<u8>, ext: &str, prefix: &str, c: &mut Collector) -> Result<(), String> {
    match kind_of(ext) {
        Some("text") => {
            c.text(&decode(&bytes), &|l| format!("{prefix}Line {l}"), matches!(ext, "csv" | "tsv"));
            Ok(())
        }
        Some("spreadsheet") => {
            let mut wb = open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| e.to_string())?;
            c.sheets(&mut wb, prefix);
            Ok(())
        }
        Some("document") => scan_ooxml_doc(&bytes, ext, prefix, c),
        Some("pdf") => scan_pdf(&bytes, prefix, c),
        _ => Ok(()),
    }
}

fn scan_zip(path: &Path, kinds: &[String], c: &mut Collector) -> Result<(), String> {
    let f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
    for i in 0..z.len().min(MAX_ZIP_ENTRIES) {
        if c.full() {
            break;
        }
        let Ok(mut entry) = z.by_index(i) else { continue };
        if entry.is_dir() || entry.size() > MAX_ZIP_ENTRY_BYTES {
            continue;
        }
        let Ok(name) = entry.name().map(|n| n.to_string()) else { continue };
        let ext = ext_of(&name);
        match kind_of(&ext) {
            Some(k) if k != "archive" && kinds.iter().any(|x| x == k) => {}
            _ => continue,
        }
        let mut buf = Vec::with_capacity(entry.size() as usize);
        if entry.read_to_end(&mut buf).is_err() {
            continue;
        }
        drop(entry);
        let _ = scan_bytes(buf, &ext, &format!("{name} › "), c);
    }
    Ok(())
}

/// Fast pre-filter for xlsx/xlsm: regex over the raw (decompressed) sheet XML.
/// Fully parsing a workbook is far more expensive, so we only do it when this says "maybe".
fn ooxml_may_match<R: Read + Seek>(reader: R, rx: &Regex) -> Result<bool, String> {
    let mut z = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    for i in 0..z.len() {
        let Ok(mut f) = z.by_index(i) else { continue };
        let Ok(name) = f.name().map(|n| n.to_string()) else { continue };
        if !(name == "xl/sharedStrings.xml" || (name.starts_with("xl/worksheets/") && name.ends_with(".xml"))) {
            continue;
        }
        buf.clear();
        if f.read_to_end(&mut buf).is_ok() && rx.is_match(&String::from_utf8_lossy(&buf)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The pre-filter never yields a false negative for literal queries without XML-escaped characters.
fn can_prefilter(o: &SearchOpts) -> bool {
    let bad = |s: &str| s.contains(['&', '<', '>', '"', '\'']);
    if o.is_bulk() { !o.terms.iter().any(|t| bad(t)) } else { !o.regex && !bad(&o.query) }
}

/// Everything a scan needs that is shared by all files.
pub struct Ctx {
    pub rx: Regex,
    pub opts: SearchOpts,
    /// bulk mode: normalized term → original term
    pub terms: Option<HashMap<String, String>>,
}

impl Ctx {
    pub fn new(opts: SearchOpts) -> Result<Self, String> {
        let rx = build_regex(&opts)?;
        let terms = opts
            .is_bulk()
            .then(|| clean_terms(&opts).into_iter().map(|t| (norm(&t, opts.case_sensitive), t)).collect());
        Ok(Self { rx, opts, terms })
    }

    fn collector(&self) -> Collector<'_> {
        match &self.terms {
            Some(t) => Collector::bulk(&self.rx, t, self.opts.case_sensitive),
            None => Collector::new(&self.rx, self.opts.max_hits_per_file),
        }
    }
}

/// Scan one file on disk. Returns the hits (possibly empty) and whether they were truncated.
pub fn scan_file(path: &Path, kind: &str, ctx: &Ctx) -> Result<(Vec<Hit>, bool), String> {
    let mut c = ctx.collector();
    let ext = ext_of(&path.to_string_lossy());
    match kind {
        "spreadsheet" => {
            if matches!(ext.as_str(), "xlsx" | "xlsm") && can_prefilter(&ctx.opts) {
                let f = fs::File::open(path).map_err(|e| e.to_string())?;
                if !ooxml_may_match(std::io::BufReader::new(f), &ctx.rx)? {
                    return Ok((Vec::new(), false));
                }
            }
            let mut wb = open_workbook_auto(path).map_err(|e| e.to_string())?;
            c.sheets(&mut wb, "");
        }
        "archive" => scan_zip(path, &ctx.opts.kinds, &mut c)?,
        _ => {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            scan_bytes(bytes, &ext, "", &mut c)?;
        }
    }
    Ok((c.hits, c.truncated))
}

// ================================================================ driver

struct Candidate {
    path: std::path::PathBuf,
    kind: &'static str,
    size: u64,
    modified: Option<u64>,
}

fn collect(o: &SearchOpts, cancel: &AtomicBool) -> Vec<Candidate> {
    let max_bytes = o.max_file_mb.max(1) * 1024 * 1024;
    let skip_dir = |name: &str| {
        DEFAULT_SKIP_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name)) || o.exclude_dirs.iter().any(|p| name_matches(p, name))
    };
    let includes: Vec<&String> = o.name_include.iter().filter(|p| !p.trim().is_empty()).collect();
    let excludes: Vec<&String> = o.name_exclude.iter().filter(|p| !p.trim().is_empty()).collect();
    WalkDir::new(&o.root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !(e.file_type().is_dir() && e.depth() > 0 && skip_dir(&e.file_name().to_string_lossy())))
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy();
            if name.starts_with("~$") {
                return None; // Office lock files
            }
            let kind = kind_of(&ext_of(&name))?;
            if !o.kinds.iter().any(|k| k == kind) {
                return None;
            }
            if !includes.is_empty() && !includes.iter().any(|p| name_matches(p, &name)) {
                return None;
            }
            if excludes.iter().any(|p| name_matches(p, &name)) {
                return None;
            }
            let md = e.metadata().ok()?;
            if md.len() > max_bytes {
                return None;
            }
            let modified = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
            if let (Some(after), Some(m)) = (o.modified_after, modified) {
                if m < after {
                    return None;
                }
            }
            if let (Some(before), Some(m)) = (o.modified_before, modified) {
                if m >= before {
                    return None;
                }
            }
            Some(Candidate { path: e.into_path(), kind, size: md.len(), modified })
        })
        .collect()
}

pub fn run(o: SearchOpts, cancel: Arc<AtomicBool>, emit: &(dyn Fn(SearchEvent) + Sync)) {
    let t0 = Instant::now();
    if !Path::new(&o.root).is_dir() {
        return emit(SearchEvent::Failed { message: format!("Folder not found: {}", o.root) });
    }
    let ctx = match Ctx::new(o) {
        Ok(c) => c,
        Err(message) => return emit(SearchEvent::Failed { message }),
    };

    let files = collect(&ctx.opts, &cancel);
    let total = files.len();
    emit(SearchEvent::Started { total, bulk: ctx.terms.is_some(), terms: ctx.terms.as_ref().map_or(0, |t| t.len()) });

    let scanned = AtomicUsize::new(0);
    let matched = AtomicUsize::new(0);
    let hit_count = AtomicUsize::new(0);
    let errors = AtomicUsize::new(0);

    // leave a couple of cores for the UI
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(2).max(2);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("thread pool");

    pool.install(|| {
        files.par_iter().for_each(|f| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            match scan_file(&f.path, f.kind, &ctx) {
                Ok((hits, truncated)) if !hits.is_empty() => {
                    matched.fetch_add(1, Ordering::Relaxed);
                    hit_count.fetch_add(hits.len(), Ordering::Relaxed);
                    emit(SearchEvent::Match {
                        file: FileResult {
                            path: f.path.to_string_lossy().into_owned(),
                            name: f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            dir: f.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                            kind: f.kind.to_string(),
                            size: f.size,
                            modified: f.modified,
                            hits,
                            truncated,
                        },
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    errors.fetch_add(1, Ordering::Relaxed);
                    emit(SearchEvent::Unreadable { path: f.path.to_string_lossy().into_owned(), reason: friendly_error(&e) });
                }
            }
            let n = scanned.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 20 == 0 || n == total {
                emit(SearchEvent::Progress { scanned: n, total });
            }
        });
    });

    emit(SearchEvent::Done {
        scanned: scanned.load(Ordering::Relaxed),
        matched: matched.load(Ordering::Relaxed),
        hits: hit_count.load(Ordering::Relaxed),
        elapsed_ms: t0.elapsed().as_millis() as u64,
        cancelled: cancel.load(Ordering::Relaxed),
        errors: errors.load(Ordering::Relaxed),
    });
}

// ================================================================ unit tests

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(q: &str) -> SearchOpts {
        SearchOpts { root: ".".into(), query: q.into(), kinds: vec!["text".into()], max_hits_per_file: 20, max_file_mb: 100, ..Default::default() }
    }

    #[test]
    fn finds_line_numbers_and_snippets() {
        let rx = build_regex(&opts("356938035643809")).unwrap();
        let mut c = Collector::new(&rx, 10);
        c.text("imei,unit\n356938035643809,DT-102\nfoo", &|l| format!("Line {l}"), false);
        assert_eq!(c.hits.len(), 1);
        assert_eq!(c.hits[0].loc, "Line 2");
        assert_eq!(c.hits[0].post, ",DT-102");
    }

    #[test]
    fn csv_lines_become_fields() {
        let rx = build_regex(&opts("DT-102")).unwrap();
        let mut c = Collector::new(&rx, 10);
        c.text("IMEI;Unit;Site\n356938035643809;DT-102;PIT B\n", &|l| format!("Line {l}"), true);
        let f = c.hits[0].fields.as_ref().unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(f[1], Field { name: "Unit".into(), value: "DT-102".into(), hit: true });
        assert_eq!(f[0].name, "IMEI");
    }

    #[test]
    fn whole_word_and_case() {
        let mut o = opts("kt 9012");
        o.whole_word = true;
        let rx = build_regex(&o).unwrap();
        assert!(rx.is_match("unit KT 9012 DM"));
        assert!(!rx.is_match("KT 90120"));
    }

    #[test]
    fn bulk_tags_terms_and_caps_per_term() {
        let mut o = opts("");
        o.terms = vec!["AAA".into(), "bbb".into(), " ".into(), "aaa".into()];
        let ctx = Ctx::new(o).unwrap();
        assert_eq!(ctx.terms.as_ref().unwrap().len(), 2, "blank and duplicate terms dropped");
        let mut c = ctx.collector();
        c.text("aaa\nAAA\naaa\naaa\nBBB\nccc", &|l| format!("Line {l}"), false);
        let a = c.hits.iter().filter(|h| h.term.as_deref() == Some("AAA")).count();
        let b = c.hits.iter().filter(|h| h.term.as_deref() == Some("bbb")).count();
        assert_eq!((a, b), (BULK_HITS_PER_TERM, 1));
    }

    #[test]
    fn wildcard_names() {
        assert!(name_matches("*timesheet*", "05. TIMESHEET GTRACK 2026.xlsx"));
        assert!(name_matches("*.xlsx", "a.XLSX"));
        assert!(name_matches("report_??.csv", "report_01.csv"));
        assert!(!name_matches("report_??.csv", "report_1.csv"));
        assert!(name_matches("backup", "old backup copy.xlsx"), "plain text = contains");
        assert!(!name_matches("*.pdf", "a.pdf.zip"));
    }

    #[test]
    fn header_detection() {
        let s = |v: &str| Some(v.to_string());
        let rows = vec![
            vec![s("LAPORAN BULANAN"), None, None],
            vec![s("No"), s("IMEI"), s("Unit")],
            vec![s("1"), s("356938035643809"), s("DT-102")],
        ];
        assert_eq!(detect_header(&rows), Some(1), "title row skipped, real header found");
    }

    #[test]
    fn decodes_utf16() {
        let s = "IMEI 123";
        let mut b = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode(&b), s);
    }

    #[test]
    fn xml_text_columns_and_cells() {
        assert_eq!(xml_to_text("<w:p><w:t>A &amp; B</w:t></w:p>"), "A & B\n");
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(cell_str(&Data::Float(356938035643809.0)).unwrap(), "356938035643809");
        assert_eq!(split_csv(r#"a,"b, c","d ""q""""#, ','), vec!["a", "b, c", r#"d "q""#]);
    }

    #[test]
    fn friendly_errors() {
        assert_eq!(friendly_error("Xlsx error: workbook is password protected"), "Password-protected");
        assert_eq!(friendly_error("The process cannot access the file because it is being used by another process. (os error 32)"), "File is locked by another program");
    }

    #[test]
    fn snippet_trimming() {
        assert_eq!(head_chars("abcdef", 3), "abc…");
        assert_eq!(tail_chars("abcdef", 3), "…def");
    }
}
