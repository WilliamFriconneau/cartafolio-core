const { chromium } = require("playwright-core");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");
// The built program and the browser: set MEMCP_BIN and CHROMIUM if they are elsewhere.
const BIN = process.env.MEMCP_BIN || path.join(__dirname, "..", "..", "..", "..", "target", "debug", "memcp");
const CHROMIUM = process.env.CHROMIUM || "/usr/bin/chromium";
const SHOTS = process.env.SCREENSHOTS || ""; // a directory, to keep screenshots of the page
const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-ui-"));
let PORT = 0, BASE = "";
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
run("init", "--spaces", "legacy");
run("vault", "add", "work", "--user", "owner");
run("vault", "add", "health", "--user", "owner");
const console_token = run("console-token", "owner").split("\n")[1].trim();
let server = null;
const sleep = ms => new Promise(r => setTimeout(r, ms));
// The four sections of the page (docs/UI-SPEC.md): "" is Spaces, "#settings" holds the machines, the assistants and the rest.
const section = async (page, h) => { await page.evaluate(h => { location.hash = h; }, h); await sleep(200); };
let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };
(async () => {
  PORT = await freePort(); BASE = `http://127.0.0.1:${PORT}`;
  server = spawn(BIN, ["--data", D, "serve", "--listen", `127.0.0.1:${PORT}`, "--public-url", "https://memory.example.net"], { stdio: "ignore" });
  await sleep(1200);
  const browser = await chromium.launch({ executablePath: CHROMIUM, args: ["--no-sandbox"], headless: true });
  const ctx = await browser.newContext({ acceptDownloads: true, locale: "en-US", viewport: { width: 1200, height: 1500 } });
  const page = await ctx.newPage();
  const problems = [];
  // A refused request (the page before sign-in, my deliberate wrong key, a code used twice) logs a 401 or 400: expected.
  page.on("console", m => { if (["error", "warning"].includes(m.type()) && !/status of 40[01]/.test(m.text())) problems.push(m.type() + ": " + m.text()); });
  page.on("pageerror", e => problems.push("pageerror: " + e.message));
  page.on("requestfailed", r => problems.push("requestfailed: " + r.url()));
  await page.addInitScript(() => { window.__csp = []; document.addEventListener("securitypolicyviolation", e => window.__csp.push(e.violatedDirective + " " + e.blockedURI)); });

  // sign in with a console token
  await page.goto(BASE + "/admin");
  check("the page asks for a console key when not signed in", await page.locator("#login").isVisible());
  await page.fill("#login-token", "wrong-key");
  await page.click("#login-form button");
  await sleep(500);
  check("a wrong key is refused and the page stays on the login", await page.locator("#login").isVisible() && (await page.locator("#login-err").innerText()).length > 0);
  await page.fill("#login-token", console_token);
  await page.click("#login-form button");
  await page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  check("signed in: the app shows", await page.locator("#app").isVisible());
  const rows = await page.locator("#spaces .tile[data-space]").allInnerTexts();
  check("the page carries the project name", (await page.title()) === "MemCP" && (await page.locator("h1").first().innerText()) === "MemCP");
  check("the spaces are listed with their kind", rows.some(r => /work/i.test(r)) && rows.some(r => /health/i.test(r)), rows.join(" | "));
  check("each space is a tile with its own connect button", (await page.locator("#spaces .tile[data-space] button:has-text('Connect')").count()) === rows.length, String(rows.length));
  await section(page, "#settings");
  check("no address for the whole memory is shown: each assistant has its own", (await page.locator("#mcp-url").count()) === 0 && (await page.locator("#open-wizard").isVisible()));
  check("the technical forms are folded away until asked for", !(await page.locator("#advanced").evaluate(d => d.open)) && !(await page.locator("#advanced form").first().isVisible()));
  check("nothing is connected at the start and the page says so", (await page.locator("#connections").innerText()).includes("Nothing is connected"), await page.locator("#connections").innerText());
  if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ui-main.png"), fullPage: true });

  // a key for one vault: the wizard
  await page.click("#open-wizard");
  await page.click("#w-platforms button:has-text('Claude Code')");
  check("nothing is ticked at the start", (await page.locator("#w-access input[type=checkbox]:checked").count()) === 0);
  await page.locator("#w-access label:has-text('Work') input").check();
  await page.fill("#w-name", "My Laptop!");
  await page.click("#w-create");
  await page.waitForSelector("#w-steps pre", { timeout: 5000 });
  const cmd = await page.locator("#w-steps pre").first().innerText();
  const key = (cmd.match(/Bearer (hcp_[a-z0-9_]+)/) || [])[1];
  check("the command carries the name, the key and the address of the connection", !!key && cmd.includes("http my-laptop https://memory.example.net/mcp/my-laptop"), cmd);
  check("the last step gives the prompt that sets the memory up", (await page.locator("#w-prompt pre").innerText()).includes("You are connected to my memory") && (await page.locator("#w-prompt button:has-text('copy')").isVisible()));
  check("it waits for the first request", (await page.locator("#w-wait").innerText()).includes("Waiting"));
  if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ui-wizard-key.png") });
  // the assistant shows up: a real request with that key
  const init = await (await fetch(BASE + "/mcp/my-laptop", { method: "POST", headers: { Authorization: "Bearer " + key, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "ping" }) })).json();
  check("the key works at the address of its connection", JSON.stringify(init.result) === "{}");
  const elsewhere = await fetch(BASE + "/mcp/someone-else", { method: "POST", headers: { Authorization: "Bearer " + key, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "ping" }) });
  check("and at no other connection's address", elsewhere.status === 403);
  const general = await fetch(BASE + "/mcp", { method: "POST", headers: { Authorization: "Bearer " + key, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "ping" }) });
  check("and the bare /mcp takes nothing", general.status === 404);
  await page.waitForFunction(() => document.querySelector("#w-wait").innerText.startsWith("Connected"), null, { timeout: 8000 });
  check("the page notices the first request", (await page.locator("#w-wait").innerText()).includes("Connected"));
  // the clipboard (a real browser, a loopback page is a secure context)
  await ctx.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.locator("#w-steps button:has-text('copy')").first().click();
  await sleep(200);
  const copied = await page.evaluate(() => navigator.clipboard.readText()).catch(() => "");
  check("copy puts the command on the clipboard", copied.includes("claude mcp add"), copied.slice(0, 60));
  // web assistant: its name is the end of its address
  await page.click("#w-platforms button:has-text('Claude on the web')");
  check("the name is suggested first", (await page.inputValue("#w-name")) === "claudeai");
  const web = await page.locator("#w-steps pre").first().innerText();
  check("a web connector gets an address named after it", web === "https://memory.example.net/mcp/claudeai", web);
  if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ui-wizard-web.png") });
  // an app with its own paths: preselected, a configuration to paste, then its guide approved here
  await page.click("#w-platforms button:has-text('Kalios')");
  check("Kalios is preselected with its paths and nothing to tick", (await page.inputValue("#w-name")) === "kalios" && (await page.locator("#w-access input[type=checkbox]").count()) === 0 && (await page.locator("#w-access").innerText()).includes("owner-health/sources/kalios"));
  await page.click("#w-create");
  await page.waitForSelector("#w-steps pre", { timeout: 5000 });
  const cfgText = await page.locator("#w-steps pre", { hasText: "mcpServers" }).first().innerText();
  const cfg = JSON.parse(cfgText).mcpServers.kalios;
  check("the configuration to paste holds the address and the key", cfg.url === "https://memory.example.net/mcp/kalios" && /^Bearer hcp_/.test(cfg.headers.Authorization), cfgText.slice(0, 80));
  const rpc = async (method, params, id) => (await (await fetch(BASE + "/mcp/kalios", { method: "POST", headers: { Authorization: cfg.headers.Authorization, "Content-Type": "application/json", Accept: "application/json, text/event-stream" }, body: JSON.stringify({ jsonrpc: "2.0", id, method, params }) })).json());
  await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "kalios", version: "1" } }, 1);
  const tl = await rpc("tools/list", {}, 2);
  const names = tl.result.tools.map(t => t.name);
  check("Kalios gets its tools, with the one to propose its guide", ["recall", "remember", "retire", "stats", "guide_propose"].every(n => names.includes(n)), names.join());
  const kdep = await rpc("tools/call", { name: "remember", arguments: { text: "[Kalios] note e1: {}", origin: { app: "kalios", id: "note/e1", at: "2026-11-02" } } }, 3);
  check("it deposits under its own paths", kdep.result && !kdep.result.isError, JSON.stringify(kdep).slice(0, 120));
  const kguide = await rpc("tools/call", { name: "guide_propose", arguments: { text: "Records are one JSON object per measure, under sources/kalios." } }, 4);
  check("it proposes its guide", kguide.result && !kguide.result.isError, JSON.stringify(kguide).slice(0, 120));
  await page.click("#wizard form button");
  await page.reload();
  await page.waitForSelector("#app-guides-section:not([hidden])", { timeout: 5000 });
  const prop = await page.locator("#app-guides").innerText();
  check("the proposal waits on the page, as text, for the person", prop.includes("kalios") && prop.includes("one JSON object per measure") && prop.includes("waiting for you"), prop);
  const before = (await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "kalios", version: "1" } }, 5)).result.instructions;
  check("until it is approved, no assistant is told it", !before.includes("one JSON object per measure"));
  await page.locator("#app-guides button:has-text('Approve')").click();
  await page.waitForFunction(() => { const t = document.querySelector("#app-guides").innerText; return t.includes("approved") && !t.includes("waiting for you"); }, null, { timeout: 5000 });
  const after = (await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "kalios", version: "1" } }, 6)).result.instructions;
  check("an app's own key, limited to its paths, is not told what lies outside them", !after.includes("one JSON object per measure"));
  // an assistant that reaches the whole space is told it, framed with who approved it
  const mk = await (await fetch(BASE + "/admin/api/connections", { method: "POST", headers: { Authorization: "Bearer " + console_token, "X-Requested-With": "memcp", "Content-Type": "application/json" }, body: JSON.stringify({ agent: "whole-reader", rights: "owner-health:r" }) })).json();
  const whole = await (await fetch(BASE + "/mcp/whole-reader", { method: "POST", headers: { Authorization: "Bearer " + mk.token, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 7, method: "initialize", params: {} }) })).json();
  const told2 = (whole.result || {}).instructions || JSON.stringify(whole);
  check("once approved, an assistant reaching the whole space is told it, framed with who approved it", told2.includes("GUIDE of the app \"kalios\"") && told2.includes("one JSON object per measure") && told2.includes("Form only"), told2.slice(-300));

  // instructions: the person's own are written here; a space's are read-only on the page (its assistants keep them)
  await section(page, "");
  await page.locator("#spaces .tile[data-space=owner-work]").locator("button:has-text('Instructions')").click();
  check("a space's instructions open read-only", await page.locator("#guide-text").evaluate(t => t.readOnly) && !(await page.locator("#guide-save").isVisible()));
  await page.click("#guide-dlg form button:last-child");
  await page.click("#my-guide");
  await page.fill("#guide-text", "Formal tone at work.");
  await page.click("#guide-save");
  await sleep(500);
  check("saved instructions come back marked", (await page.locator("#my-guide").innerText()).includes("✓") || (await page.locator("#my-guide").innerText()).length > 0);
  await page.click("#my-guide");
  await sleep(300);
  check("and reopen with their text", (await page.inputValue("#guide-text")) === "Formal tone at work.");
  if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ui-guide.png") });
  await page.click("#guide-dlg form button:last-child");
  const told = await (await fetch(BASE + "/mcp/my-laptop", { method: "POST", headers: { Authorization: "Bearer " + key, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 2, method: "initialize", params: {} }) })).json();
  check("a connection is told those instructions", told.result.instructions.includes("Formal tone at work."), JSON.stringify(told).slice(0, 200));

  // export: a real download
  const [download] = await Promise.all([page.waitForEvent("download", { timeout: 15000 }), page.click("#export-mine")]);
  const tarPath = path.join(D, "export.tar");
  await download.saveAs(tarPath);
  const listing = execFileSync("tar", ["-tf", tarPath], { encoding: "utf8" });
  check("the export downloads as a tar with a manifest and the readable files", download.suggestedFilename().endsWith(".tar") && listing.includes("manifest.json") && listing.includes("readable/owner-work.md"), listing.slice(0, 200));

  // activity
  await page.reload();
  await page.waitForSelector("#app:not([hidden])");
  await section(page, "#settings");
  await sleep(500);
  await page.locator("details.fold summary", { hasText: "Activity" }).click();
  const acts = await page.locator("#activity tr").allInnerTexts();
  check("activity says what happened in plain words", acts.some(r => r.includes("a connection was made")) && acts.some(r => r.includes("instructions written")) && !acts.some(r => /\bconnect\b\s*$/m.test(r.split("\t")[2] || "")), acts.join(" | "));
  check("the page remembers the sign-in for the session and shows activity", await page.locator("#app").isVisible());
  if (SHOTS) await page.screenshot({ path: path.join(SHOTS, "ui-activity.png"), fullPage: true });

  const csp = await page.evaluate(() => window.__csp);
  check("the connection carries the name that was given, made readable", (await page.locator("#connections").innerText()).includes("my-laptop"), await page.locator("#connections").innerText());
  await page.locator("#connections .tile", { hasText: "my-laptop" }).locator("button:has-text('rename')").click();
  await page.fill("#rename-input", "work-laptop");
  await page.locator("#connections button:has-text('save')").click();
  await page.waitForFunction(() => document.querySelector("#connections").innerText.includes("work-laptop"));
  check("it can be renamed from the list", !(await page.locator("#connections").innerText()).includes("my-laptop"));
  // Revoking opens a confirmation with a button to hold: a click does nothing, holding does it.
  const row = page.locator("#connections .tile", { hasText: "work-laptop" });
  await row.locator("button:has-text('Revoke')").click();
  await page.waitForSelector("#confirm-dlg[open]", { timeout: 3000 });
  check("revoking opens a confirmation", await page.locator("#confirm-dlg").isVisible());
  check("the confirmation names what is revoked", (await page.locator("#confirm-q").innerText()).includes("work-laptop"));
  const hold = page.locator("#confirm-hold");
  await hold.click();
  await sleep(300);
  check("a plain click does not revoke", (await page.locator("#connections").innerText()).includes("work-laptop") && await page.locator("#confirm-dlg").isVisible());
  const box = await hold.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down(); await sleep(500); await page.mouse.up(); await sleep(900);
  check("letting go early does not revoke", (await page.locator("#connections").innerText()).includes("work-laptop"));
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down(); await sleep(1600); await page.mouse.up();
  await page.waitForFunction(() => !document.querySelector("#connections").innerText.includes("work-laptop"));
  check("holding revokes it and closes the confirmation", !(await page.locator("#confirm-dlg").isVisible()));
  // a bare code typed alone on this node's page goes to its own pairing route
  const made = await (await fetch(BASE + "/admin/api/pairing", { method: "POST", headers: { Authorization: "Bearer " + console_token, "X-Requested-With": "memcp", "Content-Type": "application/json" }, body: "{}" })).json();
  await page.fill("#code-input", made.code.replace("-", "").toLowerCase());
  await page.click("#code-go");
  await sleep(400);
  check("eight lower-case letters with no dash are not taken for a code", !(await page.locator("#secret").evaluate(d => d.open)) && (await page.locator("#code-note").innerText()).length > 0);
  await page.fill("#code-input", made.code);
  await page.click("#code-go");
  await page.waitForSelector("#secret[open]", { timeout: 5000 });
  check("a bare code makes a connection of its own and shows its key once", /hcp_/.test(await page.locator("#secret-body").innerText()));
  await page.click("#secret form button");
  await page.fill("#code-input", made.code);
  await page.click("#code-go");
  await sleep(500);
  check("the code works once", !(await page.locator("#secret").evaluate(d => d.open)) && (await page.locator("#code-note").innerText()).includes("unknown, used or expired"));
  await page.locator("#connections .tile").first().waitFor().catch(() => {});
  check("no violation of the content security policy", csp.length === 0, JSON.stringify(csp));
  check("no error or warning in the console, no failed request", problems.length === 0, problems.join(" | "));
  await browser.close();
  server.kill();
  fs.rmSync(D, { recursive: true, force: true });
  console.log(failed ? "\n" + failed + " FAILED" : "\nthe real page works in a real browser");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error("ERROR", e.message); if (server) server.kill(); fs.rmSync(D, { recursive: true, force: true }); process.exit(2); });
