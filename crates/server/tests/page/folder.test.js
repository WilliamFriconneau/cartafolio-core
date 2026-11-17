// The "Folder on this node" block of a space on the node page, in a real Chromium against the real program: the default
// (no mirror), choosing a folder with its preview held to confirm, the cloud advice, a pass on demand, and back to the
// default. The folder is this node's own setting: the page says so.
const { chromium } = require("playwright-core");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");
const BIN = process.env.MEMCP_BIN || path.join(__dirname, "..", "..", "..", "..", "target", "debug", "memcp");
const CHROMIUM = process.env.CHROMIUM || "/usr/bin/chromium";
const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-folder-ui-"));
const W = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-folder-ui-dir-"));
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
run("init", "--spaces", "legacy");
run("vault", "add", "work", "--user", "owner");
const token = run("console-token", "owner").split("\n")[1].trim();
const sleep = ms => new Promise(r => setTimeout(r, ms));
let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };
(async () => {
  const PORT = await freePort(), BASE = `http://127.0.0.1:${PORT}`;
  const server = spawn(BIN, ["--data", D, "serve", "--listen", `127.0.0.1:${PORT}`, "--public-url", "https://memory.example.net"], { stdio: "ignore" });
  await sleep(1200);
  const browser = await chromium.launch({ executablePath: CHROMIUM, args: ["--no-sandbox"], headless: true });
  const ctx = await browser.newContext({ locale: "en-US", viewport: { width: 1300, height: 1800 } });
  const page = await ctx.newPage();
  const problems = [];
  const native = [];
  page.on("dialog", d => { native.push(d.message()); d.dismiss(); });
  page.on("console", m => { if (["error", "warning"].includes(m.type()) && !/status of (40[0-9]|423)/.test(m.text())) problems.push(m.type() + ": " + m.text()); });
  page.on("pageerror", e => problems.push("pageerror: " + e.message));
  const api = async (p, body) => (await fetch(BASE + "/admin/api/" + p, { method: body ? "POST" : "GET", headers: { Authorization: "Bearer " + token, "X-Requested-With": "memcp", "Content-Type": "application/json" }, body: body ? JSON.stringify(body) : undefined })).json();
  const hold = async () => {
    await page.waitForSelector("#confirm-dlg[open]", { timeout: 5000 });
    const box = await page.locator("#confirm-hold").boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down(); await sleep(1700); await page.mouse.up();
    await sleep(600);
  };
  const confirmText = async () => { await page.waitForSelector("#confirm-dlg[open]", { timeout: 5000 }); return page.locator("#confirm-q").innerText(); };

  await api("fact/add", { space: "owner-work", text: "Pay the garage on Friday" });
  await page.goto(BASE + "/admin");
  await page.fill("#login-token", token);
  await page.click("#login-form button");
  await page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  await page.evaluate(() => document.querySelectorAll("details").forEach(d => { d.open = true; }));
  await page.locator("#ftree .frow", { hasText: "work" }).click();
  await page.waitForSelector("#side-folder [data-folder-path]");

  const block = () => page.locator("#side-folder").innerText();
  const t0 = await block();
  check("the block is there, in the words of the page", /Folder on this node/.test(t0) && /stays on this node/.test(t0), t0);
  check("by default it is the node's own folder, nothing to sync", /storage folder/.test(t0) && (await page.locator("#folder-sync").count()) === 0, t0);

  // Choose a folder held by a drive: the preview says it is this node's, then the advice shows.
  const cloud = path.join(W, "CloudStorage", "GoogleDrive-me@example.org", "notes");
  await page.fill("#folder-path", cloud);
  await page.click("#folder-set");
  const q = await confirmText();
  check("the preview says what happens and that it is not shared", /not shared with the other nodes/.test(q) && q.includes(cloud), q);
  check("nothing was made before the confirmation", !fs.existsSync(cloud));
  await hold();
  await page.waitForSelector("#side-folder [data-cloud]");
  check("a cloud folder is allowed and flagged with the advice", /Keep it on the device/.test(await block()));
  check("the folder is the one chosen", (await page.locator("#side-folder [data-folder-path]").getAttribute("data-folder-path")).endsWith("notes"));

  // A pass on demand writes the note into the folder, and the page says when.
  await page.click("#folder-sync");
  await page.waitForFunction(() => /Last scan/.test(document.querySelector("#folder-scan").innerText), null, { timeout: 8000 });
  const files = fs.readdirSync(cloud).filter(f => f.endsWith(".md"));
  check("the note is a Markdown file of the folder", files.length === 1 && /Pay the garage/.test(fs.readFileSync(path.join(cloud, files[0]), "utf8")));
  const info = await api("space/folder?space=owner-work");
  check("the node says the same", info.cloud === "google-drive" && info.mode === "readwrite" && info.files === 1, JSON.stringify(info));

  // Back to the default: the folder stays as it is.
  await page.click("#folder-default");
  check("the preview says the folder is left as it is", /left as it is/.test(await confirmText()));
  await hold();
  await page.waitForFunction(() => /storage folder/.test(document.querySelector("#side-folder").innerText));
  check("the files are still there", fs.readdirSync(cloud).some(f => f.endsWith(".md")));

  check("not one browser dialog was used", native.length === 0, native.join(" | "));
  check("no error or warning in the console", problems.length === 0, problems.join(" | "));
  await browser.close();
  server.kill();
  fs.rmSync(D, { recursive: true, force: true });
  fs.rmSync(W, { recursive: true, force: true });
  console.log(failed ? "\n" + failed + " FAILED" : "\nthe folder block works in a real browser");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error(e); try { fs.rmSync(D, { recursive: true, force: true }); fs.rmSync(W, { recursive: true, force: true }); } catch (_) {} process.exit(1); });
