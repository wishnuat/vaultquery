"""Generate test fixtures and a fictional demo dataset for VaultQuery.

    python scripts/make_samples.py

Writes:
  src-tauri/tests/fixtures/   one small file per supported format, each containing known needles
  demo/                       a realistic but entirely FICTIONAL fleet-telematics document pile

All IDs are random (fixed seed). Company names, units, plates and phone numbers are made up.
Requires: openpyxl, xlwt, python-docx, python-pptx, fpdf2
"""
import csv
import io
import random
import zipfile
from datetime import date, timedelta
from pathlib import Path

import openpyxl
import xlwt
from docx import Document
from fpdf import FPDF
from pptx import Presentation
from pptx.util import Inches, Pt

ROOT = Path(__file__).resolve().parent.parent
FIX = ROOT / "src-tauri" / "tests" / "fixtures"
DEMO = ROOT / "demo"

NEEDLE = "VQ-NEEDLE-4242"
FIX_IMEI = "356938035643809"  # well-known sample IMEI used in documentation

rng = random.Random(20261125)


# ------------------------------------------------------------------ id generators

def luhn_digit(body: str) -> str:
    total = 0
    for i, ch in enumerate(reversed(body)):
        d = int(ch)
        if i % 2 == 0:
            d *= 2
            d -= 9 if d > 9 else 0
        total += d
    return str((10 - total % 10) % 10)


def fake_imei() -> str:
    body = "35" + "".join(rng.choice("0123456789") for _ in range(12))
    return body + luhn_digit(body)


def fake_gsm() -> str:
    return "0812" + "0000" + "".join(rng.choice("0123456789") for _ in range(4))  # 08120000xxxx: obviously dummy


def fake_plate() -> str:
    return f"KT {rng.randint(9000, 9999)} {rng.choice(['DM', 'DX', 'DZ'])}"


# ------------------------------------------------------------------ writers

def write_xlsx(path: Path, title: str, header: list, rows: list, sheet="Data", extra_sheets=None):
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = sheet
    ws.append([title])
    ws.append([])
    ws.append(header)
    for r in rows:
        ws.append(r)
    for name, (h, rs) in (extra_sheets or {}).items():
        s = wb.create_sheet(name)
        s.append(h)
        for r in rs:
            s.append(r)
    path.parent.mkdir(parents=True, exist_ok=True)
    wb.save(path)


def write_xls(path: Path, header: list, rows: list):
    path.parent.mkdir(parents=True, exist_ok=True)
    wb = xlwt.Workbook()
    ws = wb.add_sheet("Sheet1")
    for c, h in enumerate(header):
        ws.write(0, c, h)
    for r, row in enumerate(rows, 1):
        for c, v in enumerate(row):
            ws.write(r, c, v)
    wb.save(str(path))


def write_docx(path: Path, title: str, paragraphs: list):
    path.parent.mkdir(parents=True, exist_ok=True)
    d = Document()
    d.add_heading(title, 1)
    for p in paragraphs:
        d.add_paragraph(p)
    d.save(path)


def write_pptx(path: Path, slides: list):
    prs = Presentation()
    for title, lines in slides:
        s = prs.slides.add_slide(prs.slide_layouts[5])
        s.shapes.title.text = title
        tb = s.shapes.add_textbox(Inches(0.8), Inches(1.8), Inches(8.5), Inches(4)).text_frame
        for i, line in enumerate(lines):
            p = tb.paragraphs[0] if i == 0 else tb.add_paragraph()
            p.text = line
            p.font.size = Pt(16)
    prs.save(path)


def write_pdf(path: Path, pages: list):
    pdf = FPDF()
    pdf.set_font("Helvetica", size=11)
    for lines in pages:
        pdf.add_page()
        for line in lines:
            pdf.cell(0, 7, line, new_x="LMARGIN", new_y="NEXT")
    pdf.output(str(path))


def csv_bytes(header, rows, delim=",") -> bytes:
    buf = io.StringIO()
    w = csv.writer(buf, delimiter=delim, lineterminator="\n")
    w.writerow(header)
    w.writerows(rows)
    return buf.getvalue().encode("utf-8")


# ------------------------------------------------------------------ fixtures

def make_fixtures():
    FIX.mkdir(parents=True, exist_ok=True)
    header = ["No", "IMEI", "Unit", "Site", "Note"]
    rows = [[1, 356938035643700, "DT-101", "PIT A", "ok"],
            [2, int(FIX_IMEI), "DT-102", "PIT B", NEEDLE],
            [3, 356938035643718, "DT-103", "PIT A", "ok"]]
    write_xlsx(FIX / "sheet.xlsx", "MONTHLY DEVICE REPORT", header, rows)
    write_xls(FIX / "legacy.xls", header, rows)
    (FIX / "data.csv").write_bytes(csv_bytes(header, rows, ";"))
    (FIX / "utf16.txt").write_bytes(("first line\nsecond " + NEEDLE + " line\n").encode("utf-16"))
    write_docx(FIX / "doc.docx", "Installation report", ["Intro paragraph.", f"Device {FIX_IMEI} installed.", NEEDLE])
    write_pptx(FIX / "deck.pptx", [("Weekly review", ["nothing here"]), ("Findings", [f"Needle: {NEEDLE}"])])
    write_pdf(FIX / "report.pdf", [["Page one, no match here."], [f"Page two has {NEEDLE} inside."]])
    with zipfile.ZipFile(FIX / "archive.zip", "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("inner/logs.csv", csv_bytes(["ts", "imei", "event"], [["2026-01-01 08:00", FIX_IMEI, "ignition on"]]))
        buf = io.BytesIO()
        wb = openpyxl.Workbook()
        wb.active.append(["Code", "Value"])
        wb.active.append(["X", NEEDLE])
        wb.save(buf)
        z.writestr("inner/nested.xlsx", buf.getvalue())
    (FIX / "corrupt.xlsx").write_bytes(b"this is not really a spreadsheet")
    print(f"fixtures -> {FIX}")


# ------------------------------------------------------------------ demo dataset

SITES = ["PIT NORTH", "PIT SOUTH", "HAULING KM 12", "PORT JETTY", "WORKSHOP"]
TYPES = ["Dump Truck", "Excavator", "Light Vehicle", "Water Truck", "Grader", "Fuel Truck"]
CUSTOMERS = ["PT Demo Tambang Nusantara", "PT Contoh Energi Borneo", "PT Fiktif Logistik Raya"]
STATUS = ["Active", "Active", "Active", "Active", "No Update", "Dismantled"]


def make_demo():
    DEMO.mkdir(parents=True, exist_ok=True)
    units = []
    for i in range(120):
        prefix = rng.choice(["DT", "EX", "LV", "WT", "GR", "FT"])
        units.append({
            "unit": f"{prefix}-{300 + i}",
            "type": TYPES[["DT", "EX", "LV", "WT", "GR", "FT"].index(prefix)],
            "imei": fake_imei(),
            "gsm": fake_gsm(),
            "plate": fake_plate() if prefix in ("LV", "FT") else "-",
            "site": rng.choice(SITES),
            "customer": rng.choice(CUSTOMERS),
        })

    # 1. Master device list (csv)
    (DEMO / "Master Device List.csv").write_bytes(csv_bytes(
        ["Unit ID", "Type", "IMEI", "GSM", "Plate", "Site", "Customer"],
        [[u["unit"], u["type"], u["imei"], u["gsm"], u["plate"], u["site"], u["customer"]] for u in units]))

    # 2. Monthly timesheets (xlsx), Jan-Oct 2026; a few devices get swapped mid-year
    for m in range(1, 11):
        month = date(2026, m, 1).strftime("%B %Y")
        if m == 6:  # device swap: two units get new IMEIs from June
            for u in rng.sample(units, 2):
                u["old_imei"], u["imei"] = u["imei"], fake_imei()
        rows = []
        for n, u in enumerate(rng.sample(units, 90), 1):
            hours = round(rng.uniform(80, 420), 1)
            rows.append([n, u["unit"], u["type"], int(u["imei"]), u["gsm"], u["site"], hours, rng.choice(STATUS)])
        write_xlsx(DEMO / "Timesheet" / f"{m:02d}. Timesheet GPS {month}.xlsx", f"TIMESHEET GPS - {month.upper()}",
                   ["No", "Unit ID", "Type", "IMEI", "GSM", "Site", "Engine Hours", "Status"], rows, sheet="Timesheet")

    # 3. Speeding reports (legacy xls)
    for w in range(1, 5):
        rows = []
        for _ in range(60):
            u = rng.choice(units)
            rows.append([u["unit"], u["imei"], u["site"], rng.randint(45, 72), f"2026-09-{w * 7 - rng.randint(0, 6):02d}"])
        write_xls(DEMO / "Speeding" / f"Speeding Report Week {w} Sep 2026.xls", ["Unit", "IMEI", "Zone", "Max Speed (km/h)", "Date"], rows)

    # 4. Installation reports (docx)
    for k, u in enumerate(rng.sample(units, 6), 1):
        write_docx(DEMO / "Installation" / f"BA Installation {u['unit']}.docx", f"Installation Report No. {k:03d}/INS/2026", [
            f"Customer: {u['customer']}",
            f"Unit: {u['unit']} ({u['type']}), site {u['site']}.",
            f"Installed GPS tracker IMEI {u['imei']} with SIM {u['gsm']}.",
            "Power, ignition and GPS fix verified. Device reporting every 30 seconds.",
            "Technician: TECH-07. Witness: Site Supervisor.",
        ])

    # 5. Dismantle report (pdf)
    dismantled = rng.sample(units, 8)
    write_pdf(DEMO / "Dismantle Report Q3 2026.pdf", [
        ["DISMANTLE REPORT - Q3 2026", "PT Demo Tambang Nusantara (fictional)", ""] +
        [f"{u['unit']:8}  IMEI {u['imei']}  GSM {u['gsm']}  reason: unit off-hire" for u in dismantled[:4]],
        ["DISMANTLE REPORT - Q3 2026 (cont.)", ""] +
        [f"{u['unit']:8}  IMEI {u['imei']}  GSM {u['gsm']}  reason: unit sold" for u in dismantled[4:]],
    ])

    # 6. Weekly review deck (pptx)
    nu = [u for u in units if True][:5]
    write_pptx(DEMO / "Weekly Review W40.pptx", [
        ("Weekly Fleet Review - W40", ["Fleet availability 92%", "Devices reporting: 108 / 120"]),
        ("No-update devices", [f"{u['unit']} - IMEI {u['imei']} - last seen 3 days ago" for u in nu]),
        ("Device swaps", [f"{u['unit']}: {u.get('old_imei')} -> {u['imei']}" for u in units if 'old_imei' in u]),
    ])

    # 7. Raw daily logs inside a zip
    with zipfile.ZipFile(DEMO / "Raw GPS Logs Oct 2026.zip", "w", zipfile.ZIP_DEFLATED) as z:
        for d in range(1, 11):
            day = date(2026, 10, d)
            rows = []
            for _ in range(300):
                u = rng.choice(units)
                rows.append([f"{day} {rng.randint(0, 23):02d}:{rng.randint(0, 59):02d}", u["imei"],
                             round(-0.5 + rng.random(), 6), round(117 + rng.random(), 6), rng.randint(0, 60)])
            z.writestr(f"logs/{day}.csv", csv_bytes(["timestamp", "imei", "lat", "lon", "speed"], rows))

    # 8. A list to try bulk search with: 8 known IMEIs + 2 that exist nowhere.
    #    Kept OUTSIDE demo/ so the list doesn't find itself.
    check = [u["imei"] for u in rng.sample(units, 8)] + [fake_imei(), fake_imei()]
    (ROOT / "demo-bulk-list.txt").write_text("\n".join(check) + "\n", encoding="utf-8")
    print(f"demo     -> {DEMO}")


if __name__ == "__main__":
    make_fixtures()
    make_demo()
