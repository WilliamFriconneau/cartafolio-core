// The node page, in a real Chromium, against the real program: a node with no memory offers exactly two choices and a
// short guided path; a node with spaces shows its machines first (live peers), its folders, labels not technical names,
// the keywords and targets of a space, French and English, light and dark, and a phone width that does not scroll sideways.
//
//   cargo build -p memcp-server
//   cd crates/server/tests/page && npm install jsdom playwright-core --no-save
//   CHROMIUM=/usr/bin/chromium node onboarding.test.js        # SCREENSHOTS=dir keeps pictures at 1280 and 400 px
const { chromium } = require("playwright-core");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");
const BIN = process.env.MEMCP_BIN || path.join(__dirname, "..", "..", "..", "..", "target", "debug", "memcp");
const CHROMIUM = process.env.CHROMIUM || "/usr/bin/chromium";
const SHOTS = process.env.SCREENSHOTS || "";
const sleep = ms => new Promise(r => setTimeout(r, ms));
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };

// Stand-ins for the programs the node starts: memcp-bridge discover (one node found) and memcp-sync --join (three words).
const FAKES = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-fakes-"));
fs.writeFileSync(path.join(FAKES, "bridge"), "#!/bin/sh\nprintf 'kitchen\\thttp://192.168.1.5:8791\\t1.0\\tlan\\n'\n", { mode: 0o755 });
fs.writeFileSync(path.join(FAKES, "sync"), "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = --status-file ] && F=$2; shift; done\nprintf '{\"state\":\"waiting\",\"check\":\"amber-delta-moss\"}' > \"$F\"\n", { mode: 0o755 });
async function node(prepare) {
  const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-ob-"));
  const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
  run("init");
  const claim = run("console-token", "owner").split("\n")[1].trim();
  prepare && prepare(run);
  const port = await freePort();
  const proc = spawn(BIN, ["--data", D, "serve", "--listen", `127.0.0.1:${port}`, "--public-url", "https://memory.example.net"], { stdio: "ignore", env: { ...process.env, MEMCP_BRIDGE_BIN: path.join(FAKES, "bridge"), MEMCP_SYNC_BIN: path.join(FAKES, "sync") } });
  await sleep(1200);
  return { base: `http://127.0.0.1:${port}`, token: claim, stop: () => proc.kill(), run };
}
async function open(browser, n, viewport, opts = {}) {
  const ctx = await browser.newContext({ viewport, colorScheme: opts.scheme || "light", locale: opts.locale || "en-US" });
  const page = await ctx.newPage();
  page.problems = [];
  page.on("console", m => { if (["error", "warning"].includes(m.type()) && !/status of 40[134]/.test(m.text())) page.problems.push(m.text()); });
  page.on("pageerror", e => page.problems.push("pageerror: " + e.message));
  await page.addInitScript(() => { window.__csp = []; document.addEventListener("securitypolicyviolation", e => window.__csp.push(e.violatedDirective)); });
  if (opts.sync) await page.route("**/admin/api/sync/status", r => r.fulfill({ json: { peers: opts.sync } }));
  await page.goto(n.base + "/admin");
  await page.fill("#login-token", n.token);
  await page.click("#login-form button");
  await page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  await sleep(600);
  return page;
}
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: path.join(SHOTS, name), fullPage: true }); };
const wide = page => page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1);
const FORBIDDEN = /\b(bridges?|prox(y|ies)|vaults?)\b/i;

(async () => {
  const browser = await chromium.launch({ executablePath: CHROMIUM, args: ["--no-sandbox"], headless: true });

  // ---- A node with no memory: two choices, a guided path ----
  let empty = await node();
  let page = await open(browser, empty, { width: 1280, height: 900 });
  check("an empty node shows the start screen, not a blank page", await page.locator("#onboard").isVisible() && !(await page.locator("#home").isVisible()));
  const choices = await page.locator("#ob-body .choice").allInnerTexts();
  check("exactly two choices: Start a memory, Join a memory", choices.length === 2 && /Start a memory/.test(choices[0]) && /Join a memory/.test(choices[1]), choices.join("|"));
  check("a progress indicator of four steps is visible", (await page.locator("#ob-progress li").count()) === 4 && /Step 1 of 4/.test(await page.locator("#ob-step").innerText()));
  await shot(page, "start-1280.png");
  await page.click("#ob-start");
  check("step 2 asks for the name", /Step 2 of 4/.test(await page.locator("#ob-step").innerText()) && await page.locator("#ob-name").isVisible());
  await page.fill("#ob-name", "Sam");
  await page.click("#ob-make");
  await page.waitForSelector("#ob-connect", { timeout: 5000 });
  check("step 3 offers to connect an assistant, and the spaces now exist", /Step 3 of 4/.test(await page.locator("#ob-step").innerText()));
  await page.click("#ob-connect");
  await page.waitForSelector("#ob-url", { timeout: 5000 });
  const url = await page.locator("#ob-url").innerText();
  check("the single address carries the key", /^https:\/\/memory\.example\.net\/mcp\/my-assistant\?key=hcp_/.test(url), url);
  await shot(page, "start-3-1280.png");
  await page.click("#ob-next");
  check("step 4 says it is ready, with the default spaces", /Your memory is ready/.test(await page.locator("#ob-body").innerText()) && /6 spaces/.test(await page.locator("#ob-body").innerText()));
  await page.click("#ob-open");
  check("then the memory opens on its machines", await page.locator("#machines-section").isVisible() && await page.locator("#home").isVisible());
  const names = await page.locator("#spaces .tile h3").allInnerTexts();
  check("six spaces are made, shown by label", names.length === 6 && !names.some(n => /owner-/.test(n)), names.join("|"));
  check("no technical step left: nothing logged", page.problems.length === 0, page.problems.join(" | "));
  await page.context().close();

  // the join path, on a fresh empty node
  page = await open(browser, empty = await node(), { width: 400, height: 800 });
  check("at phone width the start screen does not scroll sideways", !(await wide(page)));
  await shot(page, "start-400.png");
  await page.click("#ob-join");
  check("join step 2 asks for the address", await page.locator("#ob-address").isVisible() && /Step 2 of 4/.test(await page.locator("#ob-step").innerText()));
  await page.waitForSelector("[data-found]", { timeout: 8000 });
  check("the nodes found on the network are listed, one click to ask", /kitchen/.test(await page.locator("[data-found]").innerText()));
  await shot(page, "join-400.png");
  await page.click("[data-found] button");
  await page.waitForSelector("#ob-words", { timeout: 8000 });
  check("then the three words are shown, at step 3", (await page.locator("#ob-words").innerText()) === "amber-delta-moss" && /Step 3 of 4/.test(await page.locator("#ob-step").innerText()));
  await page.click("#ob-body button:has-text('Back')");
  await page.fill("#ob-address", "not an address");
  await page.click("#ob-ask");
  await sleep(500);
  check("a bad address is said, not swallowed", (await page.locator("#ob-body .err").innerText()).length > 0);
  await page.context().close();

  // ---- A node with spaces: machines first, folders, labels, words, languages, themes ----
  const full = await node(run => { run("space", "add", "owner-health", "--user", "owner", "--kind", "health"); run("space", "add", "owner-work", "--user", "owner", "--kind", "work"); });
  const peers = [{ peer: "peer-laptop-ab12", name: "laptop", direction: "receiving", spaces: [{ space: "owner-health", done: 63, total: 100, percent: 63 }], last_activity: Math.floor(Date.now() / 1000) - 2 },
    { peer: "peer-phone-cd34", name: "phone", direction: "idle", spaces: [{ space: "owner-health", done: 10, total: 10, percent: 100 }], last_activity: Math.floor(Date.now() / 1000) - 300 }];
  page = await open(browser, full, { width: 1280, height: 1000 }, { sync: peers });
  const order = await page.evaluate(() => [...document.querySelectorAll("#home > *")].map(e => e.id || e.className).filter(Boolean));
  check("machines come before the assistants and the spaces", order.indexOf("machines-section") < order.indexOf("actions") && order.indexOf("actions") < order.indexOf("hero") + 99, order.join(","));
  const rows = await page.locator("#machines .mrow").allInnerTexts();
  check("this machine is first, then each peer with direction and percent", rows.length === 3 && /This machine/.test(rows[0]) && /laptop/.test(rows[1]) && /receiving/.test(rows[1]) && /63 %/.test(rows[1]) && /phone/.test(rows[2]) && /in step/.test(rows[2]), rows.join(" | "));
  check("adding a node comes before connecting an assistant", (await page.locator("#add-node").boundingBox()).y < (await page.locator("#open-wizard").boundingBox()).y);
  check("on a node that has spaces the button is Add a node, never Join a memory", (await page.locator("#add-node").innerText()) === "Add a node" && !/Join a memory/.test(await page.locator("#home").innerText()));
  await shot(page, "home-1280.png");
  check("spaces show their labels, not technical names", !(await page.locator("#spaces").innerText()).match(/owner-/));
  // a folder: put work inside health through the layout
  await page.evaluate(async () => { await api("layout/set", { space: "owner-work", parent: "owner-health" }); });
  await page.reload(); await page.waitForSelector("#app:not([hidden])"); await sleep(800);
  const folder = page.locator("#spaces [data-folder]");
  check("a space with children is a folder: no count, nothing to connect", (await folder.count()) === 1 && !/\d+ (facts?|notes?)/.test(await folder.innerText()) && !/reach/.test(await folder.innerText()) && (await folder.locator("button:has-text('Connect')").count()) === 0);
  check("the text of the page holds none of the forbidden words", !FORBIDDEN.test(await page.locator("body").innerText()), (await page.locator("body").innerText()).match(FORBIDDEN) + "");
  // the keywords and targets of a leaf space
  await page.evaluate(() => { location.hash = "#space/owner-work"; });
  await page.waitForSelector("#space-tabs button");
  await page.click("#space-tabs button:has-text('Keywords and targets')");
  await page.waitForSelector("#kw-add");
  await page.fill("#kw-add", "atlas");
  await page.press("#kw-add", "Enter");
  await page.waitForSelector(".chip:has-text('atlas')");
  const blocks = await page.locator(".block h3").allInnerTexts();
  check("the blocks: keywords, targets, janitor, changes (and tags)", ["Keywords", "Targets", "Recently added by the janitor", "What changed"].every(b => blocks.includes(b)), blocks.join("|"));
  check("each block says in a sentence what it does", (await page.locator(".block .says").allInnerTexts()).every(t => t.length > 20));
  check("a word only this space has is marked", (await page.locator(".chip.unique:has-text('atlas')").count()) === 1);
  await page.fill("#tgt-add", "walk 4 per week");
  await page.press("#tgt-add", "Enter");
  await page.waitForSelector(".tgt");
  check("a target with a figure shows a progress bar", (await page.locator(".tgt .meter").count()) === 1 && /0 of 4 this week/.test(await page.locator(".tgt").innerText()));
  await page.waitForSelector("text=No change yet.", { state: "detached", timeout: 3000 });
  check("the change of the keywords shows in the history", /atlas/.test(await page.locator(".hist").first().innerText()));
  await shot(page, "words-1280.png");
  // what the janitor added: shown with its date, one click removes a word (the page is fed a history made by the janitor)
  await page.route("**/admin/api/space?space=owner-work", async r => {
    const res = await r.fetch(); const j = await res.json();
    j.rules.keywords = [...new Set([...(j.rules.keywords || []), "devis"])];
    j.rules_history = [{ by: "janitor", at: "2026-11-16T22:00:00Z", janitor: true, keywords_added: ["devis"], keywords_removed: [], tags_added: [], tags_removed: [] }, ...(j.rules_history || [])];
    r.fulfill({ response: res, json: j });
  });
  await page.evaluate(() => loadSpace("owner-work")); await sleep(500);
  await page.click("#space-tabs button:has-text('Keywords and targets')");
  await page.waitForSelector("[data-janitor] .chip:has-text('devis')");
  check("the words the janitor added are listed with their date", /2026/.test(await page.locator("[data-janitor]").first().innerText()));
  const posted = page.waitForRequest(r => r.url().includes("space/rules") && r.method() === "POST");
  await page.click("[data-janitor] button[title='Remove this word']");
  const body = JSON.parse((await posted).postData());
  check("one click removes the word through the rules route", Array.isArray(body.keywords) && !body.keywords.includes("devis") && body.keywords.includes("atlas"), JSON.stringify(body));
  await page.unroute("**/admin/api/space?space=owner-work");
  await sleep(800); page.problems.length = 0; // the page was fed a word the node does not hold: its 400 is the test's doing

  // French
  await page.evaluate(() => { location.hash = ""; });
  await page.click("#app [data-lang=fr]");
  await sleep(500);
  const fr = await page.locator("body").innerText();
  check("French: the main blocks are translated", /Machines/.test(fr) && /Ajouter un nœud/.test(fr) && /Connecter un assistant/.test(fr) && /Vos espaces/.test(fr), fr.slice(0, 300));
  const leftovers = await page.evaluate(() => {
    const out = [], w = document.createTreeWalker(document.getElementById("home"), NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      const v = n.nodeValue.trim();
      if (v.length > 3 && /[a-z]{4}/i.test(v) && !n.parentElement.closest("code,pre,[data-notr],#kind-hint") && tr(v) === v && /\b(the|your|and|with|this|from|no|nothing|open|add)\b/i.test(v)) out.push(v);
    }
    return out;
  });
  check("French: no English sentence left on the home page", leftovers.length === 0, leftovers.slice(0, 6).join(" | "));
  await shot(page, "home-fr-1280.png");
  await page.click("#app [data-lang=en]");

  // Dark and light
  await page.click("#app [data-theme-btn]"); await page.click("#app [data-theme-btn]");
  check("the theme switch sets dark explicitly", (await page.evaluate(() => document.documentElement.dataset.theme)) === "dark" && (await page.evaluate(() => getComputedStyle(document.body).backgroundColor)) === "rgb(19, 22, 20)");
  await shot(page, "home-dark-1280.png");
  check("nothing broke the content security policy or logged", (await page.evaluate(() => window.__csp.length)) === 0 && page.problems.length === 0, page.problems.join(" | "));
  await page.context().close();

  // phone width on the full node
  page = await open(browser, full, { width: 400, height: 900 }, { sync: peers, scheme: "dark" });
  check("dark follows the device when no choice was made", (await page.evaluate(() => getComputedStyle(document.body).backgroundColor)) === "rgb(19, 22, 20)");
  check("at 400 px the home page does not scroll sideways", !(await wide(page)));
  await shot(page, "home-400.png");
  await page.evaluate(() => { location.hash = "#space/owner-work"; });
  await page.waitForSelector("#space-tabs button");
  await page.click("#space-tabs button:has-text('Keywords and targets')");
  await sleep(600);
  check("at 400 px the keywords page does not scroll sideways", !(await wide(page)));
  await shot(page, "words-400.png");
  await page.context().close();

  await browser.close();
  empty.stop(); full.stop();
  console.log(failed ? failed + " failed" : "all passed");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
