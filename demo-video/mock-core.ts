// Browser-only stand-in for @tauri-apps/api/core, used to record the demo video.
// It replays a real event stream captured from the Rust engine (examples/cli.rs with VQ_JSON=1).
import bulkRaw from "./bulk.jsonl?raw";
import regexRaw from "./regex.jsonl?raw";

const DEMO_ROOT = "D:\\Fleet Reports";

function parse(raw: string) {
  // show a neutral folder instead of the local checkout path
  const text = raw.split("C:\\\\Users\\\\HP\\\\vaultquery\\\\demo").join(DEMO_ROOT.replace(/\\/g, "\\\\"));
  return text
    .split(/\r?\n/)
    .map((l) => l.replace(/^\uFEFF/, "").trim())
    .filter((l) => l.startsWith("{"))
    .map((l) => JSON.parse(l));
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export class Channel<T> {
  onmessage: (m: T) => void = () => {};
}

export async function invoke(cmd: string, args?: any): Promise<any> {
  if (cmd !== "search") return;
  const events = parse(args.opts.terms?.length ? bulkRaw : regexRaw);
  const ch = args.onEvent as Channel<any>;
  const started = events.find((e) => e.type === "started");
  const matches = events.filter((e) => e.type === "match");
  const done = events.find((e) => e.type === "done");
  ch.onmessage(started);
  for (let i = 0; i < matches.length; i++) {
    await sleep(120); // stream results in so the video shows it
    ch.onmessage(matches[i]);
    ch.onmessage({ type: "progress", scanned: Math.round(((i + 1) / matches.length) * started.total), total: started.total });
  }
  await sleep(150);
  ch.onmessage(done);
}
