// French completeness of the page, in a real Chromium, against the real program.
//
//   cargo build -p memcp-server
//   cd crates/server/tests/page && npm install jsdom playwright-core --no-save
//   CHROMIUM=/usr/bin/chromium node french.test.js
//
// The page is loaded in French and walked through its main views and dialogs (home, administration, triage, a
// space and its tabs, the wizard for each assistant, the guide editor, the add-a-node window, the confirmation and
// the secret windows, empty states, toasts and errors). Two things fail the test:
//   1. an element that still holds one of the English sentences in ENGLISH below (the list to keep up to date: when a
//      new English sentence is added to the page, add its first words here and its French to FR in admin.html);
//   2. a visible text that the page's own translator leaves untouched and that looks English (stop words).
const { chromium } = require("playwright-core");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");

const BIN = process.env.MEMCP_BIN || path.join(__dirname, "..", "..", "..", "..", "target", "debug", "memcp");
const CHROMIUM = process.env.CHROMIUM || "/usr/bin/chromium";
const sleep = ms => new Promise(r => setTimeout(r, ms));
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };

// English sentences (or their openings) that must never be visible in French mode.
const ENGLISH = [
  "Once the assistant is connected", "You are connected to my memory", "Waiting for the assistant", "Nothing is connected",
  "Nothing waits here", "Nothing yet", "No other machine yet", "Looking for nodes", "Each person has a memory", "Waiting to be let in",
  "Tick what goes together", "Hold the red button", "Shown once", "Pick the assistant", "Give it a name", "Create the connection",
  "Pick an assistant first", "Nothing is ticked beforehand", "On the consent page", "Open claude.ai", "Add a custom connector",
  "In a terminal, run this command", "Told to every assistant", "no instructions yet", "Your heir is", "A recovery key exists",
  "You have no recovery key", "have not opened", "Attach a waiting account", "was refused in the last", "keys were refused",
  "The latest events", "Photos and documents", "What each space received", "The capture apps", "Not a decision to make",
  "An app that may propose", "Drag a space onto another", "Type the address", "Compare the three words", "has joined this memory",
  "What it captures is filed", "Check that the", "Welcome. Keep your memory", "Your memory is ready", "Start a memory",
  "Select all", "Send to", "Everything", "waiting for you", "never used", "Revoke", "Instructions for the space",
  "Have a code", "Use a code", "Cannot open", "It did not work", "could not be sent", "is not HTTPS", "Tailscale",
  "No space yet", "No note here", "Nothing here yet", "Nothing proposed", "No change yet", "No target yet", "None yet",
  "Add a space", "Connect an assistant", "Read-only", "My instructions", "my instructions", "Sort them", "Save the correction",
  "signed out", "select and copy by hand", "expired: get a new code", "QR code to join", "The microphone is not allowed",
];
// Words that make a text read as English; two of them in one text and the translator having left it alone is a miss.
const STOP = /\b(the|and|you|your|is|are|this|that|with|for|will|can|from|nothing|not|it|its|to|of|has|have|was|were|here|then|once|only|until)\b/gi;
// Things that are English on purpose: product names, commands, protocol words.
const ALLOWED = [/^MemCP\b/, /^Claude\b/, /^ChatGPT/, /^Kalios/, /^Home Assistant/, /^Tailscale/, /^OAuth/, /^MCP/, /^Authorization/, /^claude mcp/, /^https?:/];

async function node(prepare) {
  const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-fr-"));
  const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
  run("init");
  const claim = run("console-token", "owner").split("\n")[1].trim();
  prepare && prepare(run);
  const port = await freePort();
  const proc = spawn(BIN, ["--data", D, "serve", "--listen", `127.0.0.1:${port}`, "--public-url", "https://memory.example.net"], { stdio: "ignore" });
  await sleep(1200);
  return { base: `http://127.0.0.1:${port}`, token: claim, stop: () => { proc.kill(); fs.rmSync(D, { recursive: true, force: true }); } };
}

// The state the real node answers, with what a fresh node does not have yet: waiting captures, people to administer,
// an app's proposal, something that needs a look, a few events, a recovery state.
function enrich(j) {
  if (!j.me) return j;
  const me = j.me;
  j.triage = { space: "owner-a-trier", notes: [{ id: "n1", text: "call the plumber", tags: ["inbox"], created_at: "2026-11-03T08:00:00Z" }], files: [{ id: "f1", name: "scan.pdf", note: "", created_at: "2026-11-03T09:00:00Z" }] };
  j.app_guides = [{ space: "owner-health", app: "kalios", text: "One record per measure.", hash: "h1", standing: "waiting", updated_at: "x", approved_at: null }];
  j.attention = { idle: [{ agent: "forgotten-key", created_at: "2026-10-01T00:00:00+00:00", last_used_at: null, why: "never used" }], refused_24h: 3 };
  j.activity = [{ at: "2026-11-01T12:00:00+00:00", who: "claude-code", ip: "10.0.0.5", event: "revoke", detail: "" }, { at: "2026-11-01T11:00:00+00:00", who: "", ip: "203.0.113.9", event: "mcp_refused", detail: "" }];
  j.safety = { heir: null, candidates: [{ id: "u2", name: "sam", display: "Sam" }], wait_days: 30, has_key: false, heir_of: [{ id: "u3", name: "alex", display: "Alex", idle_days: 40, eligible: true }], waiting: [] };
  j.installation = { me: me.id, admin: true, requests: [{ ha_user: "ha1", name: "kim", display: "Kim", first_seen: "2026-11-02T10:00:00Z", status: "pending" }],
    users: [{ id: me.id, name: me.name, display: me.display, admin: true, disabled: false, vaults: 3, connections: 1, vault_names: ["owner-health"] },
      { id: "u2", name: "sam", display: "Sam", admin: false, disabled: false, home_assistant: true, vaults: 1, connections: 0, vault_names: [] }] };
  return j;
}

// Every visible English leftover in a part of the page: the listed sentences, and the stop-word heuristic.
async function leftovers(page, rootSel) {
  return page.evaluate(({ rootSel, ENGLISH, STOP, ALLOWED }) => {
    const stop = new RegExp(STOP, "gi"), allowed = ALLOWED.map(a => new RegExp(a));
    const out = [], root = document.querySelector(rootSel) || document.body;
    const visible = e => { for (let x = e; x && x !== document.documentElement; x = x.parentElement) { if (x.hidden || (x.tagName === "DIALOG" && !x.open)) return false; const cs = getComputedStyle(x); if (cs.display === "none" || cs.visibility === "hidden") return false; } return true; };
    const seen = new Set();
    const consider = (text, where) => {
      const v = text.replace(/\s+/g, " ").trim();
      if (v.length < 4 || seen.has(v) || allowed.some(a => a.test(v))) return;
      seen.add(v);
      const listed = ENGLISH.find(s => v.includes(s));
      if (tr(v) !== v) { out.push(where + " [the translator knows it but it was not applied]: " + v.slice(0, 90)); return; }
      const words = (v.match(stop) || []).length;
      if (listed && tr(v) === v) out.push(where + " [" + listed + "]: " + v.slice(0, 90));
      else if (words >= 2 && tr(v) === v) out.push(where + " [stop words]: " + v.slice(0, 90));
    };
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      const p = n.parentElement;
      if (!p || /^(SCRIPT|STYLE|CODE|PRE|TEXTAREA)$/.test(p.tagName) || p.closest("pre,code,[data-notr]") || !visible(p)) continue;
      consider(n.nodeValue, p.tagName.toLowerCase());
    }
    for (const e of root.querySelectorAll("[placeholder],[title],[aria-label],[alt]")) {
      if (!visible(e) || e.closest("pre,code")) continue;
      for (const a of ["placeholder", "title", "aria-label", "alt"]) if (e.getAttribute(a)) consider(e.getAttribute(a), "@" + a);
    }
    return out;
  }, { rootSel, ENGLISH, STOP: STOP.source, ALLOWED: ALLOWED.map(a => a.source) });
}

(async () => {
  const n = await node(run => { run("space", "add", "owner-health", "--user", "owner", "--kind", "health"); run("space", "add", "owner-work", "--user", "owner", "--kind", "work"); });
  const browser = await chromium.launch({ executablePath: CHROMIUM, args: ["--no-sandbox"], headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 1600 }, locale: "fr-FR" });
  const page = await ctx.newPage();
  const problems = [];
  page.on("console", m => { if (["error", "warning"].includes(m.type()) && !/status of 40[0134]/.test(m.text())) problems.push(m.text()); });
  page.on("pageerror", e => problems.push("pageerror: " + e.message));
  await page.route("**/admin/api/state", async r => { const res = await r.fetch(); r.fulfill({ response: res, json: enrich(await res.json()), status: res.status() }); });
  await page.addInitScript(() => { try { localStorage.setItem("memcp-lang", "fr"); } catch (_) {} });
  await page.goto(n.base + "/admin");
  await page.fill("#login-token", n.token);
  await page.click("#login-form button");
  await page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  await sleep(800);
  const bodyText = await page.locator("body").innerText();
  check("the page is in French", (await page.evaluate(() => document.documentElement.lang)) === "fr" && /Vos espaces/.test(bodyText), bodyText.slice(0, 200));

  const all = [];
  const scan = async (name, sel) => { const l = await leftovers(page, sel || "#app"); check("French: no English left in " + name, l.length === 0, "\n    " + l.join("\n    ")); all.push(...l); };

  await scan("the home page");
  await page.evaluate(() => { location.hash = "#settings"; }); await sleep(300);
  await page.locator("#home details.fold > summary").evaluateAll(s => s.forEach(x => x.parentElement.open = true));
  await sleep(200);
  await scan("the settings with every fold open");
  await page.evaluate(() => { location.hash = "#note"; }); await page.waitForSelector("#space-body textarea"); await sleep(400);
  await scan("trace or drop");
  await page.evaluate(() => { location.hash = "#journal"; }); await sleep(600);
  await scan("the journal");
  await page.evaluate(() => { location.hash = "#triage"; }); await sleep(300);
  await scan("the triage view");
  await page.fill("#tr-filter", "zzzz"); await sleep(150);
  await scan("the triage view with nothing matching");
  await page.fill("#tr-filter", "");
  await page.evaluate(() => { location.hash = "#admin"; }); await sleep(400);
  await scan("the administration view");
  await page.evaluate(() => { location.hash = "#space/owner-work"; }); await page.waitForSelector("#space-tabs button"); await sleep(400);
  for (const tab of await page.locator("#space-tabs button").allInnerTexts()) {
    await page.locator("#space-tabs button", { hasText: tab }).first().click(); await sleep(500);
    await scan("the space tab " + tab, "#view-space");
  }
  await page.evaluate(() => { location.hash = ""; }); await sleep(300);

  // dialogs
  await page.evaluate(() => { location.hash = "#settings"; }); await sleep(300);
  await page.click("#open-wizard");
  const platforms = await page.locator("#w-platforms button").allInnerTexts();
  for (const [i, b] of platforms.entries()) {
    await page.locator("#w-platforms button", { hasText: b }).first().click(); await sleep(200);
    await scan("the wizard, " + b, "#wizard");
    if (!(await page.locator("#w-create").count())) continue;
    if (!/Kalios/.test(b)) await page.fill("#w-name", "fr-test-" + i);
    const box = page.locator("#w-access input[type=checkbox]:not(#w-all):not(#w-ro)").first();
    if (await box.count()) await box.check();
    if (await page.locator("#w-create").isDisabled()) continue;
    await page.click("#w-create"); await sleep(600);
    await scan("the wizard once connected, " + b, "#wizard");
  }
  await page.click("#wizard form button"); await page.click("#open-wizard");
  await page.locator("#w-platforms button").nth(2).click();
  await page.fill("#w-name", "health"); await sleep(100);
  await scan("the wizard with a name that is a space", "#wizard");
  await page.click("#wizard form button");
  await page.evaluate(() => { location.hash = ""; }); await sleep(300);
  await page.locator("#spaces .tile[data-space] button", { hasText: "Instructions" }).first().click().catch(() => {});
  await page.locator("#spaces .tile[data-space] button", { hasText: "Consignes" }).first().click(); await sleep(400);
  await scan("the instructions of a space", "#guide-dlg");
  await page.click("#guide-dlg form button:last-child");
  await page.click("#my-guide"); await sleep(400);
  await scan("my instructions", "#guide-dlg");
  await page.click("#guide-dlg form button:last-child");
  await page.evaluate(() => { location.hash = "#settings"; }); await sleep(300);
  await page.click("#show-link"); await page.waitForSelector("#link-dlg[open]"); await sleep(300);
  await scan("the link of this node", "#link-dlg");
  check("the link of this node carries its id and its address", /^memcp:\/\/node\?id=[A-Z2-7]{4}-/.test(await page.locator("#link-text").innerText()));
  await page.click("#link-ticket"); await sleep(500);
  await scan("the link made without words", "#link-dlg");
  check("a link made without words carries a ticket", /&t=/.test(await page.locator("#link-text").innerText()));
  await page.click("#link-dlg form button.primary");
  await page.fill("#add-link", "hello there"); await page.click("#add-node"); await sleep(500);
  await scan("an add-a-node error", "#add-state");
  check("a link that is not one is refused in words", (await page.locator("#add-state").innerText()).length > 0);
  await page.evaluate(() => { document.querySelector("#advanced").open = true; });
  await page.click("#show-code"); await page.waitForSelector("#pair-dlg[open]"); await sleep(300);
  await scan("the code window", "#pair-dlg");
  await page.click("#pair-dlg form button");
  await page.locator("#connections .tile button", { hasText: "Révoquer" }).first().click().catch(() => {});
  await page.locator("#attention button").first().click().catch(() => {});
  await sleep(200);
  await scan("the confirmation window", "#confirm-dlg");
  await page.keyboard.press("Escape");
  await page.click("#make-key").catch(() => {});
  await page.waitForSelector("#secret[open]", { timeout: 3000 }).catch(() => {});
  await scan("the secret window", "#secret");
  await page.evaluate(() => { const d = document.querySelector("#secret"); d.open && d.close(); });
  // an error shown by the page: a name already used by a connection
  await page.evaluate(() => { fail(new Error("storage failed")); }); await sleep(100);
  await scan("an error shown by the page");

  // the onboarding of a node with no spaces, in French
  const empty = await node();
  const p2 = await (await browser.newContext({ viewport: { width: 1280, height: 1200 }, locale: "fr-FR" })).newPage();
  await p2.addInitScript(() => { try { localStorage.setItem("memcp-lang", "fr"); } catch (_) {} });
  await p2.goto(empty.base + "/admin"); await p2.fill("#login-token", empty.token); await p2.click("#login-form button");
  await p2.waitForSelector("#onboard:not([hidden])", { timeout: 5000 }); await sleep(500);
  const scan2 = async name => { const l = await leftovers(p2, "#onboard"); check("French: no English left in " + name, l.length === 0, "\n    " + l.join("\n    ")); };
  await scan2("the start screen");
  await p2.click("#ob-join"); await sleep(600);
  await scan2("the join step");
  await p2.fill("#ob-address", "not an address"); await p2.click("#ob-ask"); await sleep(600);
  await scan2("a join error");
  await p2.click("#ob-body button:has-text('Retour')");
  await p2.click("#ob-start"); await sleep(200);
  await scan2("the name step");
  await p2.click("#ob-make"); await p2.waitForSelector("#ob-connect"); await p2.click("#ob-connect"); await p2.waitForSelector("#ob-url"); await sleep(200);
  await scan2("the connect step");
  await p2.click("#ob-next"); await sleep(200);
  await scan2("the ready step");

  check("nothing logged an error", problems.length === 0, problems.join(" | "));
  await browser.close();
  n.stop(); empty.stop();
  console.log(failed ? "\n" + failed + " FAILED, " + all.length + " English leftovers on the main views" : "\nthe page is complete in French");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error("ERROR", e); process.exit(2); });
