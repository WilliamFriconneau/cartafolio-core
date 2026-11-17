// The golden paths of docs/UI-SPEC.md on the node page, against a real node: the program serves its own page and its
// own API, jsdom runs the page's script. Nothing is faked but the browser. MEMCP_BIN says where the program is
// (default: target/debug/memcp); it is not built here.
const { JSDOM, VirtualConsole } = require("jsdom");
const { execFileSync, spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
const os = require("os");
const net = require("net");

const BIN = process.env.MEMCP_BIN || path.join(__dirname, "..", "..", "..", "..", "target", "debug", "memcp");
if (!fs.existsSync(BIN)) { console.error("no program at " + BIN + ": build it (cargo build -p memcp-server) or set MEMCP_BIN"); process.exit(2); }
const D = fs.mkdtempSync(path.join(os.tmpdir(), "memcp-e2e-"));
const run = (...a) => execFileSync(BIN, ["--data", D, ...a], { encoding: "utf8" });
run("init");
run("space", "new", "Atelier", "--keywords", "alpha,project");
run("space", "new", "Jardin", "--keywords", "shopping");
const TOKEN = run("console-token", "owner").split("\n").map(l => l.trim()).find(l => l.startsWith("hcpc_"));

let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };
const sleep = ms => new Promise(r => setTimeout(r, ms));
const freePort = () => new Promise(r => { const s = net.createServer().listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => r(p)); }); });

let server = null, dom = null;
const finish = code => {
  try { if (dom) dom.window.close(); } catch (_) {}
  if (server) server.kill();
  fs.rmSync(D, { recursive: true, force: true });
  process.exit(code);
};

// What a person sees: an element is shown when no ancestor is hidden, folded away or not displayed.
function shown(w, n) {
  for (let e = n; e && e.nodeType === 1; e = e.parentElement) {
    if (e.hidden || e.classList.contains("not-here")) return false;
    if (w.getComputedStyle(e).display === "none") return false;
    const p = e.parentElement;
    if (p && p.tagName === "DETAILS" && !p.open && e.tagName !== "SUMMARY") return false;
  }
  return true;
}
function visibleText(w) {
  const out = [];
  const walk = w.document.createTreeWalker(w.document.body, 4 /* text */);
  for (let t = walk.nextNode(); t; t = walk.nextNode()) {
    const p = t.parentElement;
    if (!p || ["SCRIPT", "STYLE", "TEMPLATE"].includes(p.tagName) || p.closest("dialog:not([open])")) continue;
    if (t.nodeValue.trim() && shown(w, p)) out.push(t.nodeValue.trim());
  }
  return out.join(" ");
}
async function until(cond, ms = 5000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { try { if (cond()) return true; } catch (_) {} await sleep(50); }
  return false;
}
// The words the interface never says (docs/UI-SPEC.md), and no technical leftovers.
function clean(name, text) {
  check(name + ": no « null » or « undefined »", !/\b(null|undefined|NaN)\b/.test(text), (text.match(/.{0,40}\b(null|undefined|NaN)\b.{0,40}/) || [""])[0]);
  check(name + ": no forbidden word (bridge, capture program, capture app, vault)", !/\bbridge|capture (app|program)|\bvaults?\b/i.test(text), (text.match(/.{0,40}(bridge|capture (app|program)|vaults?).{0,40}/i) || [""])[0]);
  // Every node is equal: none is « the server » or « the administration », and the box of captures is the Inbox.
  const old = /à trier|a-trier|\bhemcp\b|administration|\b(server|serveur)\b(?! MCP)/i;
  check(name + ": no old word (À trier, administration, server, HemCP)", !old.test(text), (text.match(new RegExp(".{0,40}(" + old.source + ").{0,40}", "i")) || [""])[0]);
}

(async () => {
  const port = await freePort();
  const BASE = `http://127.0.0.1:${port}`;
  server = spawn(BIN, ["--data", D, "serve", "--listen", `127.0.0.1:${port}`], { stdio: "ignore" });
  let up = false;
  for (let i = 0; i < 100 && !up; i++) { try { up = (await fetch(BASE + "/admin")).ok; } catch (_) { await sleep(100); } }
  if (!up) { console.error("the node did not start"); return finish(1); }

  const errors = [];
  const vc = new VirtualConsole();
  vc.on("jsdomError", e => { if (!/Not implemented/.test(e.message)) errors.push(e.message); });
  vc.on("error", m => errors.push(String(m)));
  dom = await JSDOM.fromURL(BASE + "/admin", {
    runScripts: "dangerously", pretendToBeVisual: true, virtualConsole: vc,
    beforeParse(w) {
      // jsdom has no fetch: the page's requests go to the real node through Node's own.
      w.fetch = (u, o = {}) => fetch(new URL(String(u), w.location.href), { method: o.method, headers: o.headers, body: o.body });
      w.localStorage.setItem("memcp-lang", "fr");
      w.HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
      w.HTMLDialogElement.prototype.close = function () { this.removeAttribute("open"); this.dispatchEvent(new w.Event("close")); };
      w.HTMLFormElement.prototype.requestSubmit = function () { this.dispatchEvent(new w.Event("submit", { bubbles: true, cancelable: true })); };
    },
  });
  const w = dom.window, d = w.document;
  const $ = s => d.querySelector(s), $$ = s => [...d.querySelectorAll(s)];
  const go = async h => { w.location.hash = h; await sleep(50); };

  // Sign in with the console token.
  check("not signed in: the page asks for a console token", await until(() => shown(w, $("#login"))));
  $("#login-token").value = TOKEN;
  $("#login-form").dispatchEvent(new w.Event("submit", { bubbles: true, cancelable: true }));
  check("signed in: the memory opens", await until(() => shown(w, $("#app")) && !shown(w, $("#login"))), $("#login-err").textContent);

  // The four sections, in order, in French.
  await until(() => $$("#sections button").length === 4);
  const labels = $$("#sections button").map(b => b.textContent.trim());
  check("four sections in order, in French", JSON.stringify(labels) === JSON.stringify(["Tracer ou déposer", "Journal", "Espaces", "Réglages"]), labels.join(" | "));

  // Spaces (the empty hash): the tiles, then a space opens on its boards.
  check("Spaces is the section shown first", $("#sections button.on") && $("#sections button.on").textContent.trim() === "Espaces");
  check("Spaces shows a tile per space", await until(() => $$("#spaces .tile[data-space]").filter(t => shown(w, t)).length >= 2),
    $$("#spaces .tile").map(t => t.textContent).join(" | "));
  const tiles = $$("#spaces .tile[data-space]").map(t => t.textContent);
  check("the tiles carry the spaces' labels", tiles.some(t => /Atelier/.test(t)) && tiles.some(t => /Jardin/.test(t)), tiles.join(" | "));
  check("Spaces hides the settings", !shown(w, $("#machines-section")) && !shown(w, $("#open-wizard")));
  clean("Spaces", visibleText(w));
  const work = $$("#spaces .tile[data-space]").find(t => /Atelier/.test(t.textContent));
  const workName = work.getAttribute("data-space");
  await go("#space/" + workName);
  check("a space opens", await until(() => shown(w, $("#view-space")) && $("#space-title").textContent === "Atelier"), $("#space-title").textContent);
  check("a space has its Boards tab", await until(() => $$("#space-tabs button").some(b => b.textContent.trim() === "Tableaux" && shown(w, b))),
    $$("#space-tabs button").map(b => b.textContent).join(" | "));
  $$("#space-tabs button").find(b => b.textContent.trim() === "Tableaux").click();
  await sleep(500);
  clean("Boards of a space", visibleText(w));

  // Trace or drop: the Inbox in its simple view; a note written there is in the list.
  await go("#note");
  check("Trace or drop opens its own view", await until(() => shown(w, $("#view-space")) && $("#space-title").textContent === "Tracer ou déposer"), $("#space-title").textContent);
  check("the browser tab names Trace or drop", /^MemCP · (Tracer ou déposer|Trace or drop)$/.test(d.title), d.title);
  check("Trace or drop hides the tabs of a space", !shown(w, $("#space-tabs")));
  const buttons = () => $$("#space-body button").filter(b => shown(w, b)).map(b => b.textContent.trim());
  check("Trace or drop has its three buttons", await until(() => ["Ajouter au journal", "Joindre des fichiers", "● Enregistrer"].every(t => buttons().includes(t))), buttons().join(" | "));
  const box = $("#space-body textarea");
  check("Trace or drop has a box to write", !!box && shown(w, box));
  box.value = "Une idée pour le projet alpha, à creuser";
  $$("#space-body button").find(b => b.textContent.trim() === "Ajouter au journal").click();
  check("the note written appears in the list", await until(() => $$("#space-body .note").some(n => /projet alpha, à creuser/.test(n.textContent))),
    $("#space-body").textContent.slice(0, 300) + " " + $("#err").textContent);
  check("the box is emptied once the note is kept", box.isConnected ? box.value === "" : $("#space-body textarea").value === "");
  clean("Trace or drop", visibleText(w));

  // A note written straight into each space, to see the journal gather every space. (The journal reads the spaces,
  // not the sorting box: what waits in Inbox is shown in Trace or drop.)
  const gardenName = $$("#spaces .tile[data-space]").find(t => /Jardin/.test(t.textContent)).getAttribute("data-space");
  run("note", "add", workName, "Réunion projet alpha : envoyer le devis");
  run("note", "add", gardenName, "Tailler les rosiers avant l'hiver");

  // Journal: every note of every space, each with its space.
  await go("#journal");
  check("the journal opens", await until(() => shown(w, $("#view-global")) && $("#sections button.on").textContent.trim() === "Journal"));
  check("the journal lists the notes of every space", await until(() => {
    const t = $("#gj-list").textContent;
    return /envoyer le devis/.test(t) && /Tailler les rosiers/.test(t);
  }, 8000), $("#gj-list") ? $("#gj-list").textContent.slice(0, 400) : "no list");
  const rows = $$("#gj-list .note");
  const chip = n => (n && n.querySelector(".chip.sp") ? n.querySelector(".chip.sp").textContent.trim() : "");
  const devis = rows.find(n => /envoyer le devis/.test(n.textContent));
  const roses = rows.find(n => /Tailler les rosiers/.test(n.textContent));
  check("each note shows its own space", chip(devis) === "Atelier" && chip(roses) === "Jardin", chip(devis) + " / " + chip(roses));
  check("every note of the journal has its space", rows.length >= 2 && rows.every(n => n.querySelector(".chip.sp")), String(rows.length));
  clean("Journal", visibleText(w));

  // Settings: the machines and connecting an assistant; no tiles of spaces.
  await go("#settings");
  check("Settings is the section shown", await until(() => $("#sections button.on").textContent.trim() === "Réglages"));
  check("Settings shows the machines", shown(w, $("#machines-section")) && /Machines/.test($("#machines-section h2").textContent));
  check("the Machines block is this node (with its id), the nodes and one field to add a node: the codes are folded away",
    await until(() => /^[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}$/.test(($("#this-id") || {}).textContent || "")) && shown(w, $("#show-link")) && shown(w, $("#add-link")) && $$("#machines-section form").length === 1);
  check("the older codes are under Advanced", !!$("#advanced #pair-form") && !!$("#advanced #code-form"));
  check("Settings offers to connect an assistant", shown(w, $("#open-wizard")) && $("#open-wizard").textContent.trim() === "Connecter un assistant");
  check("Settings hides the spaces' tiles", $$("#spaces .tile").every(t => !shown(w, t)) && !shown(w, $("#layout-section")));
  const empty = ["#attention-section", "#app-guides-section", "#revoked-section", "#internal-section"].filter(s => shown(w, $(s)));
  check("Settings shows no empty section (nothing to look at, nothing proposed, nothing revoked)", empty.length === 0, empty.join(" "));
  check("Settings hides Trace or drop and the journal", !shown(w, $("#view-space")) && !shown(w, $("#view-global")));
  for (const s of $$("#view-memory details.fold")) s.open = true;
  await sleep(200);
  clean("Settings, every fold open", visibleText(w));

  check("the page logged no error", errors.length === 0, errors.join(" / "));
  console.log(failed ? `\n${failed} FAILED` : "\nall passed");
  finish(failed ? 1 : 0);
})().catch(e => { console.error(e); finish(1); });
