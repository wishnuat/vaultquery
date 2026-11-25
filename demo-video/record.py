"""Record the VaultQuery demo video headlessly (nothing is shown on screen).

    npx vite --config demo-video/vite.demo.config.ts   # in another terminal
    python -I demo-video/record.py
"""
import shutil
import subprocess
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"
TERMS = (HERE.parent / "demo-bulk-list.txt").read_text(encoding="utf-8")

CAPTION_JS = """(t) => {
  let el = document.getElementById('vq-cap');
  if (!el) {
    el = document.createElement('div'); el.id = 'vq-cap';
    el.style.cssText = 'position:fixed;left:50%;bottom:26px;transform:translateX(-50%);z-index:9999;'
      + 'background:rgba(20,24,32,.92);color:#fff;padding:10px 18px;border-radius:10px;'
      + 'font:600 16px Segoe UI,system-ui,sans-serif;box-shadow:0 8px 30px rgba(0,0,0,.35);transition:opacity .3s';
    document.body.appendChild(el);
  }
  el.style.opacity = t ? '1' : '0'; if (t) el.textContent = t;
}"""


def main():
    OUT.mkdir(exist_ok=True)
    for f in OUT.glob("*.webm"):
        f.unlink()
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="chrome", headless=True)
        ctx = browser.new_context(viewport={"width": 1280, "height": 800}, color_scheme="dark",
                                  record_video_dir=str(OUT), record_video_size={"width": 1280, "height": 800})
        page = ctx.new_page()
        page.goto("http://localhost:1430")
        page.evaluate("localStorage.clear()")
        page.reload()
        page.wait_for_selector("text=VaultQuery")
        cap = lambda t: page.evaluate(CAPTION_JS, t)
        page.wait_for_timeout(900)

        # ---- scene 1: bulk lookup
        cap("Bulk lookup: paste a list of device IDs")
        page.click("button[role=tab]:has-text('Bulk lookup')")
        page.wait_for_timeout(500)
        page.click(".folderPath")
        page.keyboard.type("D:\\Fleet Reports", delay=35)
        page.wait_for_timeout(300)
        page.click(".bulkbox textarea")
        page.fill(".bulkbox textarea", TERMS)  # a paste
        page.wait_for_timeout(1400)
        cap("One pass over every Excel, CSV, PDF, PowerPoint and zip file")
        page.click("button:has-text('Look up all')")
        page.wait_for_selector("text=terms found", timeout=20000)
        page.wait_for_timeout(1500)
        cap("…and it tells you which IDs appear nowhere")
        page.click(".bulktabs button:has-text('Not found')")
        page.wait_for_timeout(2200)
        page.click(".bulktabs button:has-text('All')")
        cap("Every hit shows the sheet, cell and the whole row, labelled by header")
        page.click(".term.found .trow >> nth=0")
        page.wait_for_timeout(1200)
        page.mouse.move(640, 500)
        page.mouse.wheel(0, 260)
        page.wait_for_timeout(2600)

        # ---- scene 2: regex search
        cap("Regular search works too, including regex")
        page.click("button[role=tab]:has-text('Search')")
        page.wait_for_timeout(400)
        page.click(".query")
        page.keyboard.type("DT-3\\d\\d", delay=70)
        page.click("button.tog[title='Regular expression']")
        page.wait_for_timeout(500)
        page.click("button.go:has-text('Search')")
        page.wait_for_selector("article.file", timeout=20000)
        page.wait_for_selector(".statusbar >> text=files scanned", timeout=20000)
        page.wait_for_timeout(1000)
        cap("Results stream in: PDF pages, slides, zipped CSV logs…")
        for _ in range(6):
            page.mouse.wheel(0, 320)
            page.wait_for_timeout(450)
        page.wait_for_timeout(800)
        cap("VaultQuery: no index, about a 3 MB installer")
        page.mouse.wheel(0, -5000)
        page.wait_for_timeout(2400)
        cap("")
        page.wait_for_timeout(500)
        video = page.video.path()
        ctx.close()
        browser.close()

    webm = OUT / "raw.webm"
    shutil.move(video, webm)
    mp4 = OUT / "vaultquery-demo.mp4"
    gif = OUT / "vaultquery-demo.gif"
    ff = ["ffmpeg", "-y", "-loglevel", "error"]
    subprocess.run(ff + ["-i", str(webm), "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "22", "-preset", "fast",
                         "-movflags", "+faststart", str(mp4)], check=True)
    subprocess.run(ff + ["-i", str(webm), "-vf",
                         "fps=10,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=96[p];[b][p]paletteuse=dither=bayer:bayer_scale=4",
                         str(gif)], check=True)
    for f in (mp4, gif):
        print(f"{f}  {f.stat().st_size / 1e6:.1f} MB")


if __name__ == "__main__":
    sys.exit(main())
