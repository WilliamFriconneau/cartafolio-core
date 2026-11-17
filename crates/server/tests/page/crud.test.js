// The new create / read / update / delete buttons of the page, in a real Chromium against the real program: delete a
// space with a choice, archive and restore, new key, revoke and remove a connection, pause and revoke a machine,
// bring a removed note back, remove a vocabulary, internal tools kept apart. Every destructive button shows its
// preview first and is held to confirm (no browser dialog).
const { chromium } = require("playwright-core");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");
const BIN = process.env.MEMCP_BIN || path.join(__dirname, "..", "..", "..", "..", "target", "debug", "memcp");
const CHROMIUM = process.env.CHROMIUM || "/usr/bin/chromium";
const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-crud-ui-"));
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
run("init", "--spaces", "legacy");
run("vault", "add", "work", "--user", "owner");
run("vault", "add", "health", "--user", "owner");
run("connect", "tool-a", "--all");
run("connect", "peer-laptop-ab12", "--all");
run("connect", "janitor", "--all", "--internal");
fs.writeFileSync(path.join(D, "signs.txt"), "fever\ncough\n");
run("vocabulary", "import", path.join(D, "signs.txt"), "--space", "owner-health", "--name", "signs");
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
  const openAll = () => page.evaluate(() => document.querySelectorAll("details").forEach(d => { d.open = true; }));

  await page.goto(BASE + "/admin");
  await page.fill("#login-token", token);
  await page.click("#login-form button");
  await page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  await openAll();

  // ---- a space: create with a kind and keywords, then delete with a choice
  await page.fill("#layout-add input[name=label]", "Cars");
  await page.fill("#layout-add input[name=keywords]", "garage, tyres");
  await page.click("#layout-add button.primary");
  await page.waitForFunction(() => document.querySelector("#ftree").innerText.includes("Cars"));
  check("a new space shows in the tree", true);
  await page.locator("#ftree .frow", { hasText: "Cars" }).click();
  await page.waitForSelector("#fside #delete-space");
  check("its keywords are chips of the side panel", (await page.locator("#fside").innerText()).includes("#garage"));
  await page.fill("#fside input[placeholder^='add a keyword']", "winter");
  await page.press("#fside input[placeholder^='add a keyword']", "Enter");
  await page.waitForFunction(() => document.querySelector("#fside").innerText.includes("#winter"));
  check("a keyword is added from the page", true);
  await page.click("#delete-space");
  await page.waitForSelector("#del-dlg[open]");
  check("deleting asks for a choice: archive or move into another space", (await page.locator("#del-how option").allInnerTexts()).length === 2);
  await page.click("#del-go");
  const q1 = await confirmText();
  check("the preview says what will happen before anything does", /archived/.test(q1) && /restored/.test(q1), q1);
  check("the space is still there before the confirmation", (await api("layout")).spaces.some(s => s.label === "Cars"));
  await hold();
  await page.waitForFunction(() => !document.querySelector("#ftree").innerText.includes("Cars"));
  check("holding archives it: it leaves the tree", true);
  await page.waitForSelector("#archived-section:not([hidden])");
  check("the archive is listed with a restore button", (await page.locator("#archived").innerText()).includes("Cars"));
  await page.locator("#archived button:has-text('restore')").click();
  check("restoring shows its preview too", /comes back/.test(await confirmText()));
  await hold();
  await page.waitForFunction(() => document.querySelector("#ftree").innerText.includes("Cars"));
  check("the restored space is back in the tree", true);
  // delete into another space
  await page.locator("#ftree .frow", { hasText: "Cars" }).click();
  await page.click("#delete-space");
  await page.waitForSelector("#del-dlg[open]");
  await page.selectOption("#del-how", "into");
  await page.click("#del-go");
  check("the 'into' preview names notes and files", /go into/.test(await confirmText()));
  await page.click("#confirm-dlg button:not(#confirm-hold)");

  // ---- a connection: new key, revoke, remove from the list
  await page.evaluate(() => { location.hash = "#settings"; }); // the connections and the machines are in Settings
  await sleep(200);
  await openAll();
  await page.waitForSelector("#connections .tile");
  const tile = () => page.locator("#connections .tile", { hasText: "tool-a" });
  await tile().locator("button:has-text('new key')").click();
  check("a new key shows its preview", /new key at the same address/.test(await confirmText()));
  await hold();
  await page.waitForSelector("#secret[open]", { timeout: 5000 });
  check("the new key is shown once", /hcp_/.test(await page.locator("#secret-body").innerText()));
  await page.click("#secret form button");
  await tile().locator("button:has-text('Revoke')").click();
  check("revoking shows its preview", /stops at once/.test(await confirmText()));
  await hold();
  await page.waitForSelector("#revoked-section:not([hidden])");
  check("the revoked connection moves to its own list", (await page.locator("#revoked").innerText()).includes("tool-a"));
  await page.locator("#revoked [data-agent=tool-a] button").click();
  check("removing it shows its preview", /removed from the list/.test(await confirmText()));
  await hold();
  await page.waitForFunction(() => document.querySelector("#revoked-section").hidden);
  check("it is gone from the list", true);

  // ---- internal tools stay out of the assistants
  check("the janitor is not in the connected assistants", !(await page.locator("#connections").innerText()).includes("janitor"));
  await page.waitForSelector("#internal-section:not([hidden])");
  check("it is in the Internal tools section", (await page.locator("#internal").innerText()).includes("janitor"));

  // ---- a machine: pause, then revoke
  await page.waitForSelector("#machines [data-peer='peer-laptop-ab12']", { timeout: 8000 });
  const mrow = () => page.locator("#machines [data-peer='peer-laptop-ab12']");
  check("the machine is listed with its controls", (await mrow().innerText()).includes("laptop") && await mrow().locator("button:has-text('pause')").count() === 1);
  await mrow().locator("button:has-text('pause')").click();
  check("pausing shows its preview", /stops exchanging/.test(await confirmText()));
  await hold();
  await page.waitForFunction(() => document.querySelector("#machines [data-peer='peer-laptop-ab12']")?.innerText.includes("paused"), null, { timeout: 8000 });
  check("the machine shows as paused, with a resume button", await mrow().locator("button:has-text('resume')").count() === 1);
  await mrow().locator("button:has-text('revoke')").click();
  await hold();
  await page.waitForFunction(() => document.querySelector("#machines [data-peer='peer-laptop-ab12']")?.innerText.includes("revoked"), null, { timeout: 8000 });
  check("a revoked machine stays in the list, marked revoked, and can be forgotten", await mrow().locator("button:has-text('forget')").count() === 1);
  await mrow().locator("button:has-text('forget')").click();
  await hold();
  await page.waitForFunction(() => !document.querySelector("#machines [data-peer='peer-laptop-ab12']"), null, { timeout: 8000 });
  check("a forgotten machine leaves the list", true);

  // ---- a removed note comes back
  const f = await api("fact/add", { space: "owner-work", text: "Pay the garage on Friday" });
  await api("fact/retire", { space: "owner-work", id: f.fact.id, reason: "typo" });
  await page.evaluate(() => { location.hash = "#space/owner-work"; });
  await page.waitForSelector("#space-tabs button");
  await page.locator("#space-tabs button", { hasText: "Notes" }).click();
  await page.locator("#space-body button:has-text('Show removed notes')").click();
  await page.locator("#space-body", { hasText: "Pay the garage" }).waitFor();
  await page.locator("#space-body .bubble", { hasText: "Pay the garage" }).first().click();
  await page.locator("#space-body button:has-text('Bring back')").click();
  await sleep(800);
  const again = await api("facts?space=owner-work");
  check("a removed note comes back as a new active note", again.facts.some(x => x.text.includes("Pay the garage")));

  // ---- a vocabulary is removed from the side panel (people are on the administration page, covered by the HTTP tests)
  await page.evaluate(() => { location.hash = ""; });
  await openAll();
  await page.locator("#ftree .frow", { hasText: "health" }).click();
  await page.waitForSelector("#side-vocab [data-vocabulary=signs]");
  await page.locator("#side-vocab [data-vocabulary=signs] button:has-text('remove')").click();
  check("removing a vocabulary shows its preview", /signs/.test(await confirmText()));
  await hold();
  await page.waitForFunction(() => !document.querySelector("#side-vocab [data-vocabulary=signs]"));
  check("the vocabulary is gone", true);

  check("not one browser dialog was used", native.length === 0, native.join(" | "));
  check("no error or warning in the console", problems.length === 0, problems.join(" | "));
  await browser.close();
  server.kill();
  fs.rmSync(D, { recursive: true, force: true });
  console.log(failed ? "\n" + failed + " FAILED" : "\nthe new buttons work in a real browser");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error(e); try { fs.rmSync(D, { recursive: true, force: true }); } catch (_) {} process.exit(1); });
