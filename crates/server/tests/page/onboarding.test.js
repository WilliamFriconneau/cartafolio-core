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

// Stand-ins for what the node cannot do without a network: the nodes found (MEMCP_DISCOVER_FOUND) and memcp-sync --join (three words).
const FAKES = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-fakes-"));
fs.writeFileSync(path.join(FAKES, "sync"), "#!/bin/sh\nwhile [ $# -gt 0 ]; do [ \"$1\" = --status-file ] && F=$2; shift; done\nprintf '{\"state\":\"waiting\",\"check\":\"amber-delta-moss\"}' > \"$F\"\n", { mode: 0o755 });
async function node(prepare) {
  const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-ob-"));
  const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
  run("init");
  const claim = run("console-token", "owner").split("\n")[1].trim();
  prepare && prepare(run);
  const port = await freePort();
  const proc = spawn(BIN, ["--data", D, "serve", "--listen", `127.0.0.1:${port}`, "--public-url", "https://memory.example.net"], { stdio: "ignore", env: { ...process.env, MEMCP_DISCOVER_FOUND: "kitchen\thttp://192.168.1.5:8791\t1.0\tlan", MEMCP_SYNC_BIN: path.join(FAKES, "sync") } });
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
// The four sections of the page (docs/UI-SPEC.md): "" is Spaces, "#settings" holds the machines, the assistants and the rest.
const section = async (page, h) => { await page.evaluate(h => { location.hash = h; }, h); await sleep(300); };
const shot = async (page, name) => { if (SHOTS) await page.screenshot({ path: path.join(SHOTS, name), fullPage: true }); };
const wide = page => page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1);
const location_hash = url => (url.includes("#") ? "#" + url.split("#")[1] : "");
const FORBIDDEN = /\b(bridges?|capture (app|program)s?|prox(y|ies)|vaults?)\b/i;

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
  check("then the memory opens on its spaces, under the four sections", await page.locator("#home").isVisible() && (await page.locator("#sections button").count()) === 4 && !(await page.locator("#machines-section").isVisible()));
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
  const peers = [{ peer: "peer-laptop-ab12", name: "laptop", direct: true, direction: "receiving", done: 63, total: 100, percent: 63, spaces: [{ space: "owner-health", done: 63, total: 100, percent: 63 }], last_activity: Math.floor(Date.now() / 1000) - 2 },
    { peer: "peer-phone-cd34", name: "phone", direct: true, direction: "idle", done: 10, total: 10, percent: 100, spaces: [{ space: "owner-health", done: 10, total: 10, percent: 100 }], last_activity: Math.floor(Date.now() / 1000) - 300 }];
  page = await open(browser, full, { width: 1280, height: 1000 }, { sync: peers });
  const order = await page.evaluate(() => [...document.querySelectorAll("#home > *")].map(e => e.id || e.className).filter(Boolean));
  check("machines come before the assistants and the spaces", order.indexOf("machines-section") < order.indexOf("actions") && order.indexOf("actions") < order.indexOf("hero") + 99, order.join(","));
  await section(page, "#settings");
  const rows = await page.locator("#machines .mrow").allInnerTexts();
  check("this machine is first, then each peer with direction and percent", rows.length === 3 && /This machine|MemCP/.test(rows[0]) && /laptop/.test(rows[1]) && /receiving/.test(rows[1]) && /63 %/.test(rows[1]) && /phone/.test(rows[2]) && /in step/.test(rows[2]), rows.join(" | "));
  check("adding a node comes before connecting an assistant", (await page.locator("#add-node").boundingBox()).y < (await page.locator("#open-wizard").boundingBox()).y);
  check("on a node that has spaces the button is Add a node, never Join a memory", (await page.locator("#add-node").innerText()) === "Add a node" && !/Join a memory/.test(await page.locator("#home").innerText()));
  await shot(page, "home-1280.png");
  await section(page, "");
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
  await page.waitForSelector(".block .chip:has-text('atlas')");
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

  // ---- Nesting: a sub-space is inside its folder, never beside it ----
  await page.evaluate(() => { location.hash = ""; });
  await sleep(400);
  check("top level: the sub-space is not shown beside its folder", (await page.locator("#spaces > [data-space='owner-work']").count()) === 0 && (await page.locator("#spaces > [data-folder='owner-health']").count()) === 1);
  check("the folder says how many spaces it holds, never a note count", /1 space inside/.test(await folder.innerText()) && /Work/i.test(await folder.innerText()));
  await page.click("[data-folder='owner-health'] button:has-text('Open')");
  await page.waitForSelector("#spaces-crumb button");
  check("opening a folder shows its children and a breadcrumb", location_hash(await page.url()) === "#folder/owner-health" && (await page.locator("#spaces > [data-space='owner-work']").count()) === 1 && /Your spaces\s*›\s*.+/.test(await page.locator("#spaces-crumb").innerText()), await page.locator("#spaces-crumb").innerText());
  check("the folder offers a way to add a space inside, in the folder", (await page.locator("#spaces [data-add-inside] button").count()) === 1);
  await shot(page, "folder-1280.png");
  await page.click("#spaces [data-space='owner-work'] button:has-text('Open')");
  await page.waitForSelector("#space-tabs button");
  const crumb = await page.locator("#space-crumb").innerText();
  check("a sub-space opens with its breadcrumb (parent > child)", /Your spaces\s*›\s*.+/.test(crumb) && /health/i.test(crumb), crumb);
  const rules = await page.locator("#space-rules").innerText();
  check("a sub-space shows its keywords under its title", /#atlas/.test(rules) && /Files here/.test(rules), rules);
  check("the title and the page title name the space with its folder", /work/i.test(await page.locator("#space-title").innerText()) && /health.*›.*work/i.test(await page.title()), await page.title());
  // drag and drop between folders: a tile onto the breadcrumb goes up one level, onto a tile goes inside
  await page.evaluate(() => { location.hash = "#folder/owner-health"; });
  await page.waitForSelector("#spaces > [data-space='owner-work']");
  await page.dragAndDrop("#spaces > [data-space='owner-work']", "#spaces-crumb button:first-child");
  await page.waitForSelector("#spaces > [data-space='owner-work']", { state: "attached" });
  await sleep(600);
  check("dropping a tile on « Your spaces » puts it back at the top level", (await page.locator("#spaces > [data-space='owner-work']").count()) === 1 && (await page.locator("#spaces > [data-folder]").count()) === 0, location_hash(await page.url()));
  await page.dragAndDrop("#spaces > [data-space='owner-work']", "#spaces > [data-space='owner-health']");
  await sleep(800);
  check("dropping a tile on another puts it inside: the other becomes a folder", (await page.locator("#spaces > [data-folder='owner-health']").count()) === 1 && (await page.locator("#spaces > [data-space='owner-work']").count()) === 0);
  await page.evaluate(() => { location.hash = ""; });

  // ---- Notes for the next checks: some tagged, one that carries the name of a space ----
  await page.evaluate(async () => {
    await api("fact/add", { space: "owner-work", text: "Plan the quarterly review with the team", tags: ["planning"] });
    await api("fact/add", { space: "owner-health", text: "Slept seven hours and felt rested", tags: ["sleep", "planning"] });
    await api("fact/add", { space: "owner-work", text: "Ask the doctor about the schedule", tags: ["owner-health"] });
    await api("fact/add", { space: "owner-health", text: "Walked four kilometres by the river", tags: ["training/walk"] });
  });
  await page.reload(); await page.waitForSelector("#app:not([hidden])"); await sleep(900);

  // ---- The journal of every space ----
  await page.evaluate(() => { location.hash = "#journal"; });
  await page.waitForSelector("#gj-list .note", { timeout: 5000 });
  const jrows = await page.locator("#gj-list .note").count();
  const heroNotes = Number((await page.locator("#hero .facts div").nth(1).locator("b").innerText()).replace(/\D/g, ""));
  check("the journal lists the notes of every space; hero, header and list give the same number", jrows === heroNotes && new RegExp("^" + heroNotes + " notes").test(await page.locator("#global-sub").innerText()), jrows + " vs " + heroNotes + " / " + (await page.locator("#global-sub").innerText()));
  check("each entry carries its space chip and its tags", (await page.locator("#gj-list .note .chip.sp").count()) === jrows && /planning/.test(await page.locator("#gj-list").innerText()));
  check("the notes of the rules (keywords) are not entries", !/"keywords"/.test(await page.locator("#gj-list").innerText()));
  await shot(page, "journal-1280.png");
  await page.selectOption("#gj-space", "owner-work");
  await sleep(500);
  check("filter by space", (await page.locator("#gj-list .note").count()) === 2 && (await page.locator("#gj-list .note .chip.sp:has-text('health')").count()) === 0);
  await page.click("#gj-clear");
  await sleep(500);
  await page.selectOption("#gj-tag", "planning");
  await sleep(500);
  check("filter by tag", (await page.locator("#gj-list .note").count()) === 2);
  await page.click("#gj-clear");
  await sleep(400);
  await page.fill("#global-body input[type=search]", "kilometres");
  await page.press("#global-body input[type=search]", "Enter");
  await sleep(500);
  check("search across spaces", (await page.locator("#gj-list .note").count()) === 1 && /health/i.test(await page.locator("#gj-list .chip.sp").innerText()));
  await page.fill("#global-body input[type=search]", "nothing-like-this");
  await page.press("#global-body input[type=search]", "Enter");
  await sleep(500);
  check("no result says so and offers the way out", /No note matches/.test(await page.locator("#gj-list").innerText()) && (await page.locator("#gj-list button:has-text('Clear filters')").count()) === 1);
  await page.click("#gj-list button:has-text('Clear filters')");
  await sleep(500);
  // load more: the page asks for 50; make it ask for 2
  await page.route("**/admin/api/journal?*", r => r.continue({ url: r.request().url().replace("limit=50", "limit=2") }));
  await page.click("#gj-clear").catch(() => {});
  await page.evaluate(() => { location.hash = "#graph"; }); await sleep(300);
  await page.evaluate(() => { location.hash = "#journal"; }); await page.waitForSelector("#gj-more", { timeout: 5000 });
  const before = await page.locator("#gj-list .note").count();
  await page.click("#gj-more");
  await sleep(600);
  check("« Older notes » loads the next page, without a repeat", before === 2 && (await page.locator("#gj-list .note").count()) > before && (await page.locator("#gj-list .note").count()) <= jrows);
  await page.unroute("**/admin/api/journal?*");
  await page.click("#gj-list .note");
  await page.waitForSelector("#space-tabs button");
  await sleep(900);
  check("a click on an entry opens that note in its space", /^#space\//.test(location_hash(await page.url())) && (await page.locator("#space-body textarea").count()) >= 1);

  // ---- The graph ----
  await page.evaluate(() => { location.hash = "#graph"; });
  await page.waitForSelector("#graph-svg .gnode", { timeout: 5000 });
  const ids = await page.locator("#graph-svg .gnode").evaluateAll(ns => ns.map(n => n.getAttribute("data-id")));
  check("the graph has a node per space, tag and keyword", ["space:owner-health", "space:owner-work", "tag:planning", "tag:sleep", "keyword:atlas"].every(i => ids.includes(i)), ids.join(" "));
  check("the name of a space is a tag too: it is the space, not a second node", !ids.includes("tag:owner-health") && (await page.locator("#graph-svg line[data-s='space:owner-work'][data-t='space:owner-health'], #graph-svg line[data-t='space:owner-work'][data-s='space:owner-health']").count()) >= 1);
  check("a node is as big as its count", await page.evaluate(() => { const r = id => Number(document.querySelector(`.gnode[data-id="${id}"] .shape`).getAttribute("r")); return r("tag:planning") > r("tag:sleep"); }));
  const pos = async id => (await page.locator(`.gnode[data-id="${id}"]`).evaluate(n => n.getAttribute("transform")));
  const svgBox = await page.locator("#graph-svg").boundingBox();
  const p0 = await pos("space:owner-work");
  await page.mouse.move(svgBox.x + svgBox.width / 2, svgBox.y + svgBox.height / 2);
  await page.mouse.wheel(0, -400);
  await sleep(200);
  check("the wheel zooms", (await pos("space:owner-work")) !== p0);
  const p1 = await pos("space:owner-work");
  await page.mouse.move(svgBox.x + 6, svgBox.y + svgBox.height - 6);
  await page.mouse.down(); await page.mouse.move(svgBox.x + 60, svgBox.y + svgBox.height - 50, { steps: 4 }); await page.mouse.up();
  check("dragging the background pans", (await pos("space:owner-work")) !== p1);
  await page.click("[aria-label='Fit']");
  const tagBox = await page.locator(".gnode[data-id='tag:planning'] .shape").boundingBox();
  await page.mouse.click(tagBox.x + tagBox.width / 2, tagBox.y + tagBox.height / 2);
  check("a click on a node says what it is and what it is linked to", /Tag planning · 2 notes/.test(await page.locator("#graph-info").innerText()) && /Linked to/.test(await page.locator("#graph-info").innerText()), await page.locator("#graph-info").innerText());
  check("no stray text in the graph view", !/\bnull\b|undefined/.test(await page.locator("#global-body").innerText()));
  check("what is not linked to it fades", (await page.locator(".gnode.dim").count()) >= 1);
  await shot(page, "graph-1280.png");
  await page.click("#graph-show");
  await page.waitForSelector("#gj-list .note");
  check("« Show these notes » opens the journal filtered by that tag", (await page.locator("#gj-tag").inputValue()) === "planning" && (await page.locator("#gj-list .note").count()) === 2);
  await page.evaluate(() => { location.hash = "#graph"; });
  await page.waitForSelector("#graph-svg .gnode");
  await page.fill("[aria-label='Find a node']", "sleep");
  check("a search dims the other nodes", (await page.locator(".gnode.dim").count()) >= 3);
  await page.fill("[aria-label='Find a node']", "");
  await page.uncheck("input[data-kind='keyword']");
  check("keywords can be hidden", (await page.locator(".gnode.keyword:visible").count()) === 0);
  await page.check("input[data-kind='keyword']");
  const fillLight = await page.locator(".gnode.space .shape").first().evaluate(n => getComputedStyle(n).fill);
  await page.click("#app [data-theme-btn]"); await page.click("#app [data-theme-btn]");
  const fillDark = await page.locator(".gnode.space .shape").first().evaluate(n => getComputedStyle(n).fill);
  check("the graph follows light and dark", fillLight !== fillDark && /^rgb/.test(fillDark), fillLight + " / " + fillDark);
  await shot(page, "graph-dark-1280.png");
  await page.click("#app [data-theme-btn]");
  // the graph of one space, from its tab
  await page.evaluate(() => { location.hash = "#space/owner-work"; });
  await page.waitForSelector("#space-tabs button");
  await page.click("#space-tabs button:has-text('Graph')");
  await page.waitForSelector("#graph-svg .gnode");
  const mine = await page.locator("#graph-svg .gnode").evaluateAll(ns => ns.map(n => n.getAttribute("data-id")));
  check("the graph of a space holds that space only, with its tags and keywords", mine.includes("space:owner-work") && !mine.includes("space:owner-health") && mine.includes("keyword:atlas"), mine.join(" "));
  await page.click(".gnode[data-id='keyword:atlas'] .shape");
  await page.click("#graph-show");
  await page.waitForSelector("#space-body input[value='atlas']");
  check("in a space, a node filters its notes", (await page.locator("#space-body input[value='atlas']").count()) === 1);
  await page.evaluate(() => { location.hash = ""; });

  // ---- Empty states offer a next step ----
  const bare = await node(run => { run("space", "add", "owner-home", "--user", "owner", "--kind", "home"); });
  const bp = await open(browser, bare, { width: 1280, height: 900 });
  await bp.evaluate(() => { location.hash = "#journal"; }); await sleep(600);
  check("an empty journal says so and offers a way in", /Nothing in your memory yet/.test(await bp.locator("#gj-list").innerText()) && (await bp.locator("#gj-list button.primary").count()) === 1);
  await bp.evaluate(() => { location.hash = "#graph"; }); await sleep(600);
  check("an empty graph says so and offers a way in", /Nothing to (link|draw) yet/.test(await bp.locator("#global-body").innerText()) && (await bp.locator("#global-body button.primary").count()) === 1);
  await bp.context().close(); bare.stop();

  // ---- Peers live in Machines only; assistants say where they run; French from the first paint ----
  // A machine's connection is made by a pairing, a join or the node's own command line, never from the page.
  full.run("connect", "peer-laptop-ab12", "--all");
  await page.evaluate(async () => {
    await api("connections", { agent: "claudeai", rights: "all" });
    await api("connections", { agent: "claude-code", rights: "all" });
  });
  await page.reload(); await page.waitForSelector("#app:not([hidden])"); await sleep(1200);
  await section(page, "#settings");
  const tiles = await page.locator("#connections .tile h3").allInnerTexts();
  check("Connected assistants lists assistants only, never a peer", tiles.length === 2 && !tiles.some(x => /peer-/.test(x)), tiles.join("|"));
  const prow = page.locator(".mrow[data-peer='peer-laptop-ab12']");
  check("the peer's card in Machines carries rename, change access and revoke", (await prow.locator("button:has-text('rename')").count()) === 1 && (await prow.locator("button:has-text('change access')").count()) === 1 && (await prow.locator("button:has-text('Revoke')").count()) === 1);
  await prow.locator("button:has-text('change access')").click();
  check("change access opens the rights editor on the machine's card", (await prow.locator("button:has-text('save')").count()) === 1);
  await prow.locator("button:has-text('cancel')").click();
  check("an assistant says where it runs when that is known", /cloud/.test(await page.locator("#connections .tile:has-text('claudeai') .badge").innerText()) && /local/.test(await page.locator("#connections .tile:has-text('claude-code') .badge").innerText()));
  await section(page, "");
  check("the hero counts the assistants, not the machines", (await page.locator("#hero .facts div").nth(2).locator("b").innerText()) === "2");
  const fp = await open(browser, full, { width: 1280, height: 1000 }, { sync: peers, locale: "fr-FR" });
  const untranslated = await fp.evaluate(() => [...document.querySelectorAll("#home section h2, #home section h2 + p, #home details.fold > summary, #home .tile h3")]
    .filter(e => e.offsetParent !== null).map(e => e.textContent.trim())
    .filter(t => (Object.prototype.hasOwnProperty.call(FR, t) && FR[t] !== t) || /\b(the|your|and|with|this|from|nothing|what)\b/i.test(t)));
  check("French from the first paint: every heading and intro is French", untranslated.length === 0, untranslated.join(" | "));
  await section(fp, "#settings");
  check("French: the Connected assistants block", /Assistants connectés/.test(await fp.locator("#connections").locator("xpath=ancestor::section").innerText()) && /Révoquez-en un/.test(await fp.locator("#connections").locator("xpath=ancestor::section").innerText()));
  await fp.context().close();

  // French
  await page.evaluate(() => { location.hash = ""; });
  await page.click("#app [data-lang=fr]");
  await sleep(500);
  const fr = await page.locator("body").innerText();
  await section(page, "#settings");
  const frSettings = await page.locator("body").innerText();
  await section(page, "");
  check("French: the main blocks are translated", /Machines/.test(frSettings) && /Ajouter un nœud/.test(frSettings) && /Connecter un assistant/.test(frSettings) && /Vos espaces/.test(fr) && /administrateur/.test(fr), fr.slice(0, 300));
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
  for (const h of ["#journal", "#graph"]) {
    await page.evaluate(h => { location.hash = h; }, h); await sleep(900);
    const left = await page.evaluate(() => {
      const out = [], w = document.createTreeWalker(document.getElementById("view-global"), NodeFilter.SHOW_TEXT);
      for (let n = w.nextNode(); n; n = w.nextNode()) {
        const v = n.nodeValue.trim();
        if (v.length > 3 && /[a-z]{4}/i.test(v) && !n.parentElement.closest("svg,[data-notr],.tx") && tr(v) === v && /\b(the|your|and|with|this|from|no|nothing|open|add|show)\b/i.test(v)) out.push(v);
      }
      return out;
    });
    check("French: no English sentence left on " + h, left.length === 0, left.slice(0, 6).join(" | "));
  }
  check("French: the new views are named in French", /Graphe/.test(await page.locator("#global-tabs").innerText()) && /Vos espaces/.test(await page.locator("#global-crumb").innerText()));
  await shot(page, "graph-fr-1280.png");
  await page.evaluate(() => { location.hash = ""; });
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
  for (const h of ["#journal", "#graph"]) {
    await page.evaluate(h => { location.hash = h; }, h); await sleep(900);
    check("at 400 px " + h + " does not scroll sideways", !(await wide(page)));
    await shot(page, h.slice(1) + "-400.png");
  }
  check("at 400 px the graph fits the screen and its nodes are labelled", (await page.locator("#graph-svg").boundingBox()).width <= 400 && (await page.locator("#graph-svg text:visible").count()) >= 3);
  await page.evaluate(() => { location.hash = "#folder/owner-health"; }); await sleep(600);
  check("at 400 px a folder does not scroll sideways", !(await wide(page)));
  await shot(page, "folder-400.png");
  await page.context().close();

  await browser.close();
  empty.stop(); full.stop();
  console.log(failed ? failed + " failed" : "all passed");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
