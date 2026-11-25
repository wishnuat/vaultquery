# VaultQuery

[![CI](https://github.com/wishnuat/vaultquery/actions/workflows/ci.yml/badge.svg)](https://github.com/wishnuat/vaultquery/actions/workflows/ci.yml)

**Search inside Excel, CSV, Word, PowerPoint, PDF and zip files, or look up a whole list of IDs at once. It shows
the exact sheet, cell and row for every match, and needs no index.**

<!-- screenshot: docs/screenshot.png  ·  demo gif: docs/bulk-lookup.gif -->

Windows search finds file *names*. To find which of 4,000 downloaded reports mention a particular device ID, phone
number or customer, you either keep a content index running in the background (on one machine that index grew to
**20 GB**) or open the files one by one. VaultQuery scans file contents on demand, in parallel, and streams results
while it works.

## Features

### Two ways to search
- **Search**: one query, literal or **regex**, with match case and whole word
- **Bulk lookup**: paste hundreds of IDs, phone numbers or plates. VaultQuery searches them all **in a single pass**
  (Aho-Corasick via `regex`) and reports, for each term, the files and cells it appears in. It also lists the terms
  that appear **nowhere**, which helps with reconciliation work (e.g. "which of these 300 devices are missing from
  every report?"). You can copy the not-found list or export everything to CSV

### Results you don't have to open
- Spreadsheet hits are reported as `Sheet ! Cell` (e.g. `Timesheet ! D8`) together with **the whole row, labelled
  with its column headers**: `Unit ID: GR-372 · IMEI: 3538… · Site: PIT NORTH · Status: Active`. The header row is
  detected automatically, skipping title rows above it
- CSV lines are split into header/value pairs the same way, with `,` `;` tab and `|` delimiters auto-detected
- Documents report `Paragraph n`, `Slide n` and `Page n`
- **Unreadable files are listed with a reason** (password-protected, locked by another program, damaged) instead of
  being skipped silently

### Formats
| Kind | Extensions |
|---|---|
| Spreadsheets | `xlsx` `xlsm` `xlsb` `xls` `ods` |
| Text & data | `csv` `tsv` `txt` `json` `log` `xml` `kml` `geojson` `html` `sql` … (UTF-8 / UTF-16 auto-detected) |
| Documents | `docx` `pptx` `pdf` |
| Archives | `zip`: every format above is searched **inside the zip**, without extracting it |

### Filters
Date modified (today, 7/30/90 days, this year, custom range), file-name include and exclude patterns (`*timesheet*`,
`backup`), folders to skip, and maximum file size.

### Small and native
About a 3 MB installer built with Tauri (uses the system WebView2), light and dark themes, and keyboard shortcuts:
`Enter`/`Ctrl+Enter` to search, `Esc` to stop, `Ctrl+K` to focus the search box.

## Try it with the demo data

`demo/` contains a **fictional** fleet-telematics document pile: monthly timesheets (xlsx), weekly speeding reports
(legacy xls), installation reports (docx), a dismantle report (pdf), a review deck (pptx) and raw GPS logs inside a
zip. All IDs are random.

1. Open VaultQuery and choose the `demo` folder
2. **Bulk lookup**: paste the contents of `demo-bulk-list.txt` (10 IMEIs). 8 are found across timesheets, PDFs,
   decks and zipped CSV logs, and 2 are reported as *not found*
3. **Search**: try `GR-3`, or a regex such as `DT-3\d\d`

## Benchmark

A real-world folder on a laptop (i7-1265U, NVMe SSD): **4,334 files** (2,978 spreadsheets, 2,204 of them legacy
`.xls`, 995 text/CSV, 141 Office documents, 107 PDF, 113 zip).

| Version | One query |
|---|---|
| Python prototype (zip + regex, multiprocessing) | 26 s |
| Rust engine, full workbook parse | 62 s |
| **Rust engine + raw-XML pre-filter for xlsx** | **≈ 20 s** |

Bulk lookup costs about the same as a single query, because all terms are matched in one pass. Most of the remaining
time is spent in legacy `.xls` files, where numbers are stored as binary doubles and can't be pre-filtered as text.

## How it works

```
React UI ──invoke("search", opts, Channel)──▶ Rust command ──spawn_blocking──▶ engine::run
   ▲                                                                          │
   └──── Channel<SearchEvent>: started · progress · match · unreadable · done ┘
```

1. **Walk** the folder (`walkdir`), applying type, name, folder, size and date filters
2. **Scan in parallel** (`rayon`), keeping 2 cores free so the UI stays responsive. Each format has its own scanner:
   - xlsx/xlsm: regex over the raw `sharedStrings.xml` and sheet XML first. Only when that may match is the workbook
     parsed with `calamine` to find the exact cells and the header row
   - xls/xlsb/ods: parsed with `calamine`
   - docx/pptx: unzipped and converted from XML to text
   - pdf: `pdf-extract`, page by page, inside `catch_unwind` so a malformed PDF can't crash the app
   - zip: entries are read in memory and passed to the scanners above
3. **Stream** each matching file through a Tauri `Channel`. The UI batches incoming results before rendering them
4. **Cancel** through a shared `AtomicBool`. Starting a new search cancels the previous one

Bulk mode compiles all terms into one alternation, longest first. Each hit is mapped back to its term, with a
per-term cap per file so that one huge file can't hide other terms.

## Development

Requirements: Rust (MSVC toolchain), Node 18+, WebView2 (bundled with Windows 10/11).

```bash
npm install
npm run tauri dev        # run the app
npm run tauri build      # installer → src-tauri/target/release/bundle/nsis/

cd src-tauri
cargo test               # unit tests + one fixture per format (tests/formats.rs)
cargo run --release --example cli -- ../demo GR-3                         # headless search
cargo run --release --example cli -- ../demo --terms ../demo-bulk-list.txt  # headless bulk lookup
```

Fixtures and the demo dataset are generated by `python scripts/make_samples.py` (needs openpyxl, xlwt, python-docx,
python-pptx, fpdf2).

CI runs type-checking and all engine tests on every push. Pushing a `v*` tag builds the installer and attaches it to
a draft GitHub Release.

## Roadmap

- [ ] "Search in results", AND / OR / NOT across a file, saved searches
- [ ] Explorer right-click: "Search with VaultQuery"
- [ ] 7z / rar archives
- [ ] Use the Everything index for instant file listing when it's installed
- [ ] OCR for scanned PDFs
- [ ] Code signing (SignPath for OSS)

## Related tools

[dnGrep](https://github.com/dnGrep/dnGrep) is a mature Windows grep with Office plugins, and
[xlsxgrep](https://github.com/zazuum/xlsxgrep) is a CLI for spreadsheets. VaultQuery focuses on bulk lookup with a
not-found report, row context labelled by header, searching inside zips, and a fast parallel Rust engine in a small
native app.

## License

MIT
