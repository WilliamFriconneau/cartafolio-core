// The administrator's page of the manager, in a real browser, signed in the way Home Assistant's
// ingress signs people in (the proxy's headers). Not part of `cargo test`.
//
//   cargo build -p memcp-manager -p memcp-server
//   npm install playwright-core --no-save && node browser.test.js
//   (CHROMIUM=/usr/bin/chromium, SCREENSHOTS=some/dir, MEMCP_MANAGER and MEMCP_BIN to move the programs)
const { chromium } = require("playwright-core");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");
const root = path.join(__dirname, "..", "..", "..", "..", "target", "debug");
const MANAGER = process.env.MEMCP_MANAGER || path.join(root, "memcp-manager");
const ENGINE = process.env.MEMCP_BIN || path.join(root, "memcp");
const CHROMIUM = process.env.CHROMIUM || "/usr/bin/chromium";
const SHOTS = process.env.SCREENSHOTS || "";
const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-manager-ui-"));
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });
const sleep = ms => new Promise(r => setTimeout(r, ms));
let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };
let server = null;
(async () => {
  const [main, ingress] = [await freePort(), await freePort()];
  server = spawn(MANAGER, ["--data", D, "serve", "--listen", `127.0.0.1:${main}`, "--ingress-listen", `127.0.0.1:${ingress}`, "--trust-ingress", "127.0.0.1", "--public-url", "https://memory.example.net", "--new-people", "ask"], { stdio: "ignore" });
  await sleep(1200);
  const browser = await chromium.launch({ executablePath: CHROMIUM, args: ["--no-sandbox"], headless: true });
  const as = async (id, name) => {
    const ctx = await browser.newContext({ viewport: { width: 1200, height: 1500 }, extraHTTPHeaders: { "X-Remote-User-Id": id, "X-Remote-User-Name": name, "X-Remote-User-Display-Name": name } });
    const page = await ctx.newPage();
    const problems = [];
    page.on("console", m => { if (["error", "warning"].includes(m.type()) && !/status of (401|403)/.test(m.text())) problems.push(m.text()); });
    page.on("pageerror", e => problems.push("pageerror: " + e.message));
    await page.goto(`http://127.0.0.1:${ingress}/admin`);
    return { page, problems };
  };

  // The first Home Assistant account to open it administers the installation.
  const w = await as("ha-alice", "alice");
  await w.page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  const headings = await w.page.locator("main section:visible h2").allInnerTexts();
  check("the page opens on the people", headings[0] === "People", headings.join(" | "));
  check("the administrator is listed, with their connections counted", (await w.page.locator("#people tr").allInnerTexts()).some(r => r.includes("alice") && r.includes("administrator")));
  check("the form to add a person is right there, with its explanation", (await w.page.locator("#people-section form[data-act=users]").isVisible()) && (await w.page.locator("#people-section").innerText()).includes("Home Assistant login"));

  check("the memory is not shown beside the people", !(await w.page.locator("#view-memory").isVisible()));
  // Picking their own line opens their memory, on the line of the whole memory's address.
  await w.page.click("#people button:has-text('Open my memory')");
  await w.page.waitForSelector("#view-memory:not([hidden])");
  const mem = await w.page.locator("main section:visible h2").allInnerTexts();
  check("their memory opens on how to connect an assistant", mem[0] === "Connect an assistant", mem.join(" | "));
  check("with the button, and no address for the whole memory", (await w.page.locator("#mcp-url").count()) === 0 && (await w.page.locator("#open-wizard").isVisible()));
  if (SHOTS) await w.page.screenshot({ path: path.join(SHOTS, "manager-memory.png"), fullPage: true });
  await w.page.click("#back-people");
  await w.page.waitForSelector("#view-people:not([hidden])");

  // The administrator adds a person by the name of their Home Assistant login.
  await w.page.fill("#people-section input[name=name]", "lou");
  await w.page.fill("#people-section input[name=display]", "Lou");
  await w.page.click("#people-section form[data-act=users] button");
  await w.page.waitForFunction(() => [...document.querySelectorAll("#people tr")].some(r => r.innerText.includes("lou")));
  const lou = (await w.page.locator("#people tr", { hasText: "lou" }).innerText());
  check("the new person is listed, not linked to an account yet", lou.includes("not linked"), lou);
  if (SHOTS) await w.page.screenshot({ path: path.join(SHOTS, "manager-people.png"), fullPage: true });

  // A stranger waits; the administrator sees them and lets them in.
  const eve = await as("ha-eve", "eve");
  check("a stranger is told to wait", (await eve.page.locator("#login-err").innerText()).includes("waiting"), await eve.page.locator("#login-err").innerText());
  await w.page.reload();
  await w.page.waitForSelector("#requests-section:not([hidden])");
  check("the administrator sees who waits", (await w.page.locator("#requests").innerText()).includes("eve"));

  // The person added by name is linked the first time they open it.
  const l = await as("ha-lou", "lou");
  await l.page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  check("a person added by name is signed in without waiting", (await l.page.locator("#who").innerText()).includes("Lou"));
  check("a member never sees the people", !(await l.page.locator("#people-section").isVisible()));
  check("and starts on an empty memory that says what to do", (await l.page.locator("#spaces").innerText()).includes("No space yet"));
  // Lou makes a recovery key, then loses the account: a new one takes the memory back with the key.
  await l.page.locator("#safety summary").click();
  check("a person has the safety panel, with an heir to pick", (await l.page.locator("#heir-select option").count()) >= 2);
  await l.page.click("#make-key");
  await l.page.waitForSelector("#secret[open]");
  const key = (await l.page.locator("#secret-body pre").first().innerText()).trim();
  check("the recovery key is shown, in groups of four", /^([A-Z2-9]{4}-){4}[A-Z2-9]{4}$/.test(key), key);
  const l2 = await as("ha-lou-again", "lou");
  await l2.page.waitForSelector("#recover:not([hidden])", { timeout: 5000 });
  await l2.page.fill("#recover-key", key);
  await l2.page.click("#recover-form button");
  await l2.page.waitForSelector("#app:not([hidden])", { timeout: 5000 });
  check("the new account is back in Lou's memory", (await l2.page.locator("#who").innerText()).includes("Lou"));
  await w.page.reload();
  await w.page.waitForSelector("#people tr");
  const lou2 = await w.page.locator("#people tr", { hasText: "lou" }).innerText();
  check("the administrator now sees them linked", lou2.includes("linked") && !lou2.includes("not linked"), lou2);
  check("no error or warning in the console", w.problems.length === 0 && l.problems.length === 0 && l2.problems.length === 0, [...w.problems, ...l.problems, ...l2.problems].join(" | "));
  await browser.close();
  server.kill();
  fs.rmSync(D, { recursive: true, force: true });
  console.log(failed ? `\n${failed} FAILED` : "\nthe manager's page works in a real browser");
  process.exit(failed ? 1 : 0);
})().catch(e => { console.error("ERROR", e.message); if (server) server.kill(); fs.rmSync(D, { recursive: true, force: true }); process.exit(2); });
