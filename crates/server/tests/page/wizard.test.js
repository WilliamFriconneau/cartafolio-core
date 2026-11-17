const { JSDOM } = require("jsdom");
const fs = require("fs");
const html = fs.readFileSync(require("path").join(__dirname, "..", "..", "src", "admin.html"), "utf8");

const spaces = [
  { name: "health", kind: "health", members: [], mine: "rw", can_guide: true, guide: true, stats: { facts: 3, files: 0, last_write: null } },
  { name: "work", members: [], mine: "rw", can_guide: true, guide: false, stats: { facts: 1, files: 0, last_write: null } },
  { name: "shared", members: [], mine: "r", can_guide: false, guide: false, stats: null },
];
const world = { connections: [], activity: [], attention: { idle: [], refused_24h: 0 }, app_guides: [],
  triage: { space: "sam-a-trier", notes: [{ id: "n1", text: "<b>rappeler</b> le plombier", tags: ["inbox"], created_at: "2026-11-03T08:00:00Z" }], files: [] } };
const guides = { "space:health": "Compare with my usual range." };
const state = () => ({
  me: { id: "u1", name: "sam", display: "Sam", admin: true }, my_guide: false, app_guides: world.app_guides, activity: world.activity, attention: world.attention, public_url: "https://memory.example.net",
  users: [{ id: "u1", name: "sam", display: "Sam", admin: true, disabled: false, spaces: [] }],
  spaces, triage: world.triage, groups: [], connections: world.connections, devices: [], files: [], requests: [],
});
const calls = [];
const mockFetch = async (url, opts = {}) => {
  if (String(url).endsWith("/pair")) {
    const b = JSON.parse(opts.body);
    world.pairCall = b;
    if (b.code !== "ABCDEFGH") return { ok: false, status: 400, json: async () => ({ error: "this code is unknown, used or expired" }) };
    return { ok: true, status: 200, json: async () => ({ agent: "this-page-1", key: "hcp_paired_key", mcp_url: "https://memory.example.net/mcp/this-page-1", vaults: ["sam-a-trier"] }) };
  }
  const path = String(url).split("admin/api/")[1];
  calls.push([opts.method || "GET", path, opts.body]);
  const ok = body => ({ ok: true, status: 200, json: async () => body });
  if (path === "state") return ok(state());
  if (path.startsWith("guide?scope=")) {
    const scope = decodeURIComponent(path.split("=")[1]);
    return ok({ scope, max_chars: 16384, versions: guides[scope] ? 2 : 0, current: guides[scope] ? { body: guides[scope], author: "u1", written_at: "x" } : null });
  }
  if (path === "guides/set") {
    const b = JSON.parse(opts.body);
    if (b.body.includes("FAIL")) return { ok: false, status: 400, json: async () => ({ error: "the guide is empty: clear it instead" }) };
    guides[b.scope] = b.body; return ok({ scope: b.scope });
  }
  if (path === "connections/person-guide") {
    const b = JSON.parse(opts.body);
    const c = world.connections.find(x => x.agent === b.agent);
    if (c) c.person_guide = b.told;
    return ok({ ok: true });
  }
  if (path === "connections/revoke" || path === "app-guides/approve" || path === "app-guides/withdraw") return ok({ ok: true });
  if (path === "export") {
    if (world.exportFails) return { ok: false, status: 400, json: async () => ({ error: "sam owns no vault to export" }) };
    return { ok: true, status: 200, headers: { get: h => h === "Content-Disposition" ? 'attachment; filename="memcp-export-sam-20261101.tar"' : "" }, blob: async () => new w.Blob(["tar bytes"]) };
  }
  if (path === "pairing") {
    world.paired = JSON.parse(opts.body);
    return ok({ code: "ABCD-EFGH", server: "https://memory.example.net", expires_in: 600, triage: "sam-a-trier" });
  }
  if (path === "triage/send-many") {
    const b = JSON.parse(opts.body);
    world.sent = b;
    world.triage.notes = world.triage.notes.filter(n => !b.ids.includes(n.id));
    return ok({ moved: b.ids.length, failed: [] });
  }
  if (path === "guides/clear") { delete guides[JSON.parse(opts.body).scope]; return ok({ cleared: true }); }
  if (path === "connections") {
    const b = JSON.parse(opts.body);
    world.connections.push({ agent: b.agent, user: "u1", rights: b.rights, upstream: "none", read_only: false, revoked: false, created_at: "x", last_used_at: null, expires_at: "t1" });
    return ok({ token: "hcp_secret_token", mcp_url: "x/mcp", claude_code: "x" });
  }
  return { ok: false, status: 404, json: async () => ({ error: "no" }) };
};
const dom = new JSDOM(html, {
  runScripts: "dangerously", url: "https://memory.example.net/admin", pretendToBeVisual: true,
  beforeParse(w) {
    w.fetch = mockFetch;
    w.HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
    w.HTMLDialogElement.prototype.close = function () { this.removeAttribute("open"); this.dispatchEvent(new w.Event("close")); };
    w.sessionStorage.setItem("memcp-console", "x");
  },
});
const w = dom.window, d = w.document;
const $ = s => d.querySelector(s), $$ = s => [...d.querySelectorAll(s)];
const sleep = ms => new Promise(r => setTimeout(r, ms));
const text = s => ($(s) ? $(s).textContent : "");
const click = el => el.dispatchEvent(new w.MouseEvent("click", { bubbles: true }));
let failed = 0;
const check = (name, cond, extra = "") => { console.log((cond ? "ok   " : "FAIL ") + name + (cond ? "" : "  " + extra)); if (!cond) failed++; };

(async () => {
  await sleep(300);
  check("the page loaded the vaults", $$("#spaces .tile[data-space]").length === 3, String($$("#spaces .tile[data-space]").length));
  check("there is no address for the whole memory", $("#mcp-url") === null);
  check("a vault row offers to connect an assistant", $$("#spaces button").some(b => b.textContent === "Connect"));

  // --- Connect a bridge: no vault to choose, everything goes to À trier
  check("no vault is offered to a bridge", $("#pair-all") === null && $("#pair-vaults") === null);
  $("#pair-form").dispatchEvent(new w.Event("submit", { cancelable: true }));
  await sleep(100);
  check("one code, asked for no vault", JSON.stringify(world.paired) === "{}" && $("#pair-dlg").hasAttribute("open"), JSON.stringify(world.paired));
  check("the code is shown big with the address", text("#pair-code") === "ABCD-EFGH" && text("#pair-server") === "https://memory.example.net");
  check("it says everything goes to À trier", text("#pair-into") === "À trier");
  check("a countdown runs", /^\d+:\d\d$/.test(text("#pair-left")), text("#pair-left"));
  $("#pair-dlg").removeAttribute("open");

  // --- À trier: what waits, shown as text, sent to the vault the person chooses
  check("À trier shows what waits on the home page", !$("#triage-tile").hidden && text("#triage-count") === "1");
  w.location.hash = "#triage";
  await sleep(50);
  check("the À trier view lists what waits", !$("#view-triage").hidden && $$("#tr-list .trrow").length === 1);
  check("a capture is text, never markup", $("#tr-list b") === null && text("#tr-list").includes("<b>rappeler</b>"));
  check("À trier is not among the spaces", !$$("#spaces .tile h3").some(h => h.textContent.includes("a-trier")));
  check("nothing picked: send is disabled", $("#tr-send").disabled);
  click($("#tr-list .trrow"));
  check("a row picked, still no space chosen: send stays disabled", $("#tr-send").disabled && text("#tr-count").includes("1 selected"));
  $("#tr-space").value = "work";
  $("#tr-space").dispatchEvent(new w.Event("change"));
  check("a row and a space: send is enabled", !$("#tr-send").disabled && $("#tr-send").textContent === "Send 1");
  click($("#tr-send"));
  await sleep(150);
  check("sent to the chosen space", world.sent && world.sent.ids.join() === "n1" && world.sent.space === "work", JSON.stringify(world.sent));
  check("nothing left, the list says so and the tile is hidden", !$("#tr-empty").hidden && $("#triage-tile").hidden);
  w.location.hash = "";
  await sleep(50);

  // --- a bare code typed alone, on a node that already has people: the node's own pairing route
  const typeCode = v => { $("#code-input").value = v; $("#code-form").dispatchEvent(new w.Event("submit", { cancelable: true })); };
  typeCode("memory.example.net");
  await sleep(50);
  check("an address is not a code: said, and nothing is called", world.pairCall === undefined && text("#code-note").includes("eight characters"), text("#code-note"));
  typeCode("internet");
  await sleep(50);
  check("eight lower-case letters are a host name, not a code", world.pairCall === undefined);
  typeCode("WRONG-123");
  await sleep(50);
  check("a wrong length is refused before any call", world.pairCall === undefined);
  typeCode("ZZZZ-ZZZZ");
  await sleep(100);
  check("an unknown code is refused with the server's words", world.pairCall && world.pairCall.code === "ZZZZZZZZ" && text("#code-note").includes("unknown, used or expired"), text("#code-note"));
  typeCode("abcd-efgh");
  await sleep(150);
  check("a code typed in lower case with its dash is still a code", world.pairCall.code === "ABCDEFGH");
  $("#secret").removeAttribute("open");
  typeCode("ABCD-EFGH");
  await sleep(150);
  check("the bare code goes to the pairing route with its eight characters", world.pairCall.code === "ABCDEFGH" && world.pairCall.device.length > 0, JSON.stringify(world.pairCall));
  check("the connection it made is shown once, with its address and key", $("#secret").hasAttribute("open") && text("#secret-title").includes("this-page-1") && text("#secret-body").includes("hcp_paired_key") && $("#code-input").value === "", text("#secret-body"));
  check("no error is left on the page", text("#code-note") === "");
  $("#secret").removeAttribute("open");

  // --- OAuth platform: nothing to create, the steps show the address, then it waits
  click($("#open-wizard"));
  click($$("#w-platforms button").find(b => b.textContent === "Claude Code"));
  check("opened from the button, nothing is ticked at all", $$("#w-access input[type=checkbox]").filter(i => i.id !== "w-all").every(b => !b.checked), $$("#w-access input[type=checkbox]").filter(i => i.id !== "w-all").map(b => b.checked).join());
  click($$("#w-platforms button").find(b => b.textContent === "Claude on the web or phone"));
  click($("#wizard form button"));
  click($("#open-wizard"));
  check("the wizard opens", $("#wizard").hasAttribute("open"));
  check("nothing chosen yet", text("#w-steps").includes("…") && text("#w-wait").includes("…"));
  click($$("#w-platforms button").find(b => b.textContent.startsWith("Claude on the web")));
  check("the name is asked first, with a suggestion", $("#w-name") && $("#w-name").value === "claudeai" && text("#w-namebox").includes("end of its address"), $("#w-name") && $("#w-name").value);
  check("the address is the name's, in the steps", text("#w-steps").includes("https://memory.example.net/mcp/claudeai") && text("#w-namebox").includes("/mcp/claudeai"), text("#w-steps"));
  check("the access step says the consent page asks", text("#w-access").includes("consent page"));
  check("no vault box for an OAuth platform", $$("#w-access input[type=checkbox]").filter(i => i.id !== "w-all").length === 0);
  check("it waits", text("#w-wait").includes("Waiting"));
  check("the internet warning shows", text("#w-steps").includes("from the internet"));
  // the consent happens elsewhere: a new oauth connection appears
  world.connections.push({ agent: "claudeai", user: "u1", rights: "work:rw", upstream: "none", read_only: false, revoked: false, created_at: "x", last_used_at: null, expires_at: "t2" });
  await sleep(3400);
  check("it notices the connection", text("#w-wait").includes("Connected: claudeai") && text("#w-wait").includes("work:rw"), text("#w-wait"));
  click($("#wizard form button"));

  // --- keep my instructions from a connection
  await sleep(100);
  const keep = $$("#connections button").find(b => b.textContent === "keep my instructions from it");
  check("each connection can be kept from my instructions", !!keep);
  click(keep);
  await sleep(200);
  check("it shows that it is not told them", text("#connections").includes("not told your instructions") && world.connections.find(c => c.agent === "claudeai").person_guide === false, text("#connections"));
  click($$("#connections button").find(b => b.textContent === "tell my instructions"));
  await sleep(200);
  check("and it can be told them again", world.connections.find(c => c.agent === "claudeai").person_guide === true);

  // --- key platform: tick vaults, create, steps with the key, wait for the first call
  click($$("#spaces button").find(b => b.textContent === "Read-only"));
  check("opened on a vault, read only", $("#wizard").hasAttribute("open") && $("#w-ro") === null);
  click($$("#w-platforms button").find(b => b.textContent === "Claude Code"));
  const boxes = $$("#w-access input[type=checkbox]").filter(i => i.id !== "w-all");
  const vaultBoxes = boxes.filter(b => b.id !== "w-ro");
  check("the readable vaults are listed and only the one asked is ticked", vaultBoxes.length === 3 && vaultBoxes.filter(b => b.checked).length === 1 && vaultBoxes.find(b => b.checked).closest("label").textContent.trim().startsWith("health"), vaultBoxes.map(b => b.checked).join());
  check("read only is ticked because it was asked", $("#w-ro").checked);
  check("steps wait for the connection", text("#w-steps").includes("Create the connection"));
  check("the name is asked first, with a suggestion", $("#w-name").value === "claude-code");
  const rename = v => { $("#w-name").value = v; $("#w-name").dispatchEvent(new w.Event("input")); };
  rename("health");
  check("the name of a space is refused", text("#w-name-note").includes("name of a space") && $("#w-create").disabled === true, text("#w-name-note"));
  rename("");
  check("an empty name is refused", text("#w-name-note").includes("Give it a name") && text("#w-steps").includes("Give it a name"));
  rename("My Laptop!");
  check("the name is made into one an address can hold", $("#w-name-note").textContent.includes("/mcp/my-laptop"), text("#w-name-note"));
  const create = $("#w-create");
  check("create is enabled with a vault ticked", create && !create.disabled);
  const ticked = vaultBoxes.find(b => b.checked); ticked.checked = false; ticked.dispatchEvent(new w.Event("change"));
  check("create is disabled with nothing ticked", $("#w-create").disabled);
  // tick health and work, not read-only
  const byName = n => $$("#w-access label").find(l => l.textContent.trim().startsWith(n)).querySelector("input");
  for (const n of ["health", "work"]) { const i = byName(n); i.checked = true; i.dispatchEvent(new w.Event("change")); }
  const ro = $("#w-ro"); ro.checked = false; ro.dispatchEvent(new w.Event("change"));
  click($("#w-create"));
  await sleep(300);
  const post = calls.filter(c => c[1] === "connections").pop();
  const body = post && JSON.parse(post[2]);
  check("it created one connection with exactly what was ticked", body && body.agent === "my-laptop" && body.rights.split(",").sort().join() === "health:rw,work:rw" && body.upstream === undefined, JSON.stringify(body));
  check("the command carries the name, the key and the address of the connection", text("#w-steps").includes("http my-laptop https://memory.example.net/mcp/my-laptop") && text("#w-steps").includes("Authorization: Bearer hcp_secret_token"), text("#w-steps"));
  check("the access step summarises it", text("#w-access").includes("health") && text("#w-access").includes("work"));
  check("it waits for the first call", text("#w-wait").includes("Waiting"));
  world.connections.find(c => c.agent === body.agent).last_used_at = "2026-11-01T12:00:00+00:00";
  await sleep(3400);
  check("it notices the first request", text("#w-wait").includes("Connected: " + body.agent), text("#w-wait"));
  // a read-only key
  click($("#wizard form button"));
  click($$("#spaces button").find(b => b.textContent === "Read-only"));
  click($$("#w-platforms button").find(b => b.textContent === "Another assistant"));
  click($("#w-create"));
  await sleep(300);
  const post2 = calls.filter(c => c[1] === "connections").pop();
  check("read only gives :r", JSON.parse(post2[2]).rights === "health:r", post2[2]);
  check("other assistants get the header form", text("#w-steps").includes("Authorization: Bearer hcp_secret_token") && text("#w-steps").includes("OAuth"));

  // --- instructions (the guide)
  const guideBtn = n => $$("#spaces .tile[data-space]").find(r => r.textContent.startsWith(n)) && [...$$("#spaces .tile[data-space]").find(r => r.textContent.startsWith(n)).querySelectorAll("button")].find(b => b.textContent.startsWith("Instructions"));
  check("the owner sees the instructions button, ticked when there are some", guideBtn("health") && guideBtn("health").textContent === "Instructions ✓", guideBtn("health") && guideBtn("health").textContent);
  check("a vault without any shows it plainly", guideBtn("work") && guideBtn("work").textContent === "Instructions");
  check("the person has a button of their own", $("#my-guide") && $("#my-guide").textContent === "my instructions");
  click(guideBtn("health"));
  await sleep(200);
  check("the editor opens with the text in force", $("#guide-dlg").hasAttribute("open") && $("#guide-text").value === "Compare with my usual range.", $("#guide-text").value);
  check("it says who it is for and that it is read here, not edited", text("#guide-lead").includes("only on it") && text("#guide-lead").includes("read here, not edited"));
  check("it counts and shows the versions kept", text("#guide-count") === "28" && text("#guide-versions").includes("2 versions kept, none erased"), text("#guide-count") + "/" + text("#guide-versions"));
  $("#guide-text").value = "Compare with my usual range, and say when it is missing.";
  $("#guide-text").dispatchEvent(new w.Event("input"));
  check("the count follows what is typed", text("#guide-count") === "56", text("#guide-count"));
  click($("#guide-save"));
  await sleep(300);
  const saved = calls.filter(c => c[1] === "guides/set").pop();
  check("saving sends the scope and the text", saved && JSON.parse(saved[2]).scope === "space:health" && JSON.parse(saved[2]).body.startsWith("Compare with my usual range, and"), saved && saved[2]);
  check("the editor closes after a save", !$("#guide-dlg").hasAttribute("open"));
  // a refusal stays on screen
  click(guideBtn("work"));
  await sleep(200);
  check("an empty vault opens an empty editor", $("#guide-text").value === "" && text("#guide-versions").includes("no instructions yet"));
  $("#guide-text").value = "FAIL please";
  click($("#guide-save"));
  await sleep(300);
  check("a refusal is shown and the editor stays open", $("#guide-dlg").hasAttribute("open") && text("#guide-err").includes("empty"), text("#guide-err"));
  click($("#guide-dlg form button:last-child"));
  // clearing, and the person's own
  click(guideBtn("health"));
  await sleep(200);
  click($("#guide-clear"));
  await sleep(300);
  const cleared = calls.filter(c => c[1] === "guides/clear").pop();
  check("removing sends the scope", cleared && JSON.parse(cleared[2]).scope === "space:health", cleared && cleared[2]);
  click($("#my-guide"));
  await sleep(200);
  const mine = calls.filter(c => String(c[1]).startsWith("guide?scope=")).pop();
  check("my instructions asks for the person's own", mine && mine[1] === "guide?scope=person" && text("#guide-title") === "My instructions" && text("#guide-lead").includes("whatever it reaches"), mine && mine[1]);

  // --- activity and what deserves a look
  check("with nothing to show, the page says so and hides the attention section", text("#activity").includes("Nothing yet.") && $("#attention-section").hidden);
  world.activity = [
    { at: "2026-11-01T12:00:00+00:00", who: "claude-code-ab12", ip: "10.0.0.5", event: "revoke", detail: "" },
    { at: "2026-11-01T11:00:00+00:00", who: "", ip: "203.0.113.9", event: "mcp_refused", detail: "" },
    { at: "2026-11-01T10:00:00+00:00", who: "sam", ip: "", event: "guide_set", detail: "space:<b>x</b>" },
  ];
  world.attention = { idle: [{ agent: "forgotten-key", created_at: "2026-10-01T00:00:00+00:00", last_used_at: null, why: "never used" }], refused_24h: 3 };
  await w.load();
  const rows = $$("#activity tr");
  check("the events are listed with plain words", rows.length === 4 && rows[1].textContent.includes("revoked") && rows[2].textContent.includes("a key was refused") && rows[3].textContent.includes("instructions written"), rows.map(r => r.textContent).join(" | "));
  check("an event with no name shows a dash, and its address", rows[2].textContent.includes("—") && rows[2].textContent.includes("203.0.113.9"));
  check("a detail is shown as text, never as markup", rows[3].querySelector("b") === null && rows[3].textContent.includes("<b>x</b>"));
  check("the attention section appears with the refused keys and the forgotten connection", !$("#attention-section").hidden && text("#attention").includes("3 keys were refused") && text("#attention").includes("forgotten-key") && text("#attention").includes("never used"), text("#attention"));
  const revokeBtn = $$("#attention button").find(b => b.textContent === "revoke");
  click(revokeBtn);
  check("revoking opens a confirmation that names it, and does nothing yet", $("#confirm-dlg").hasAttribute("open") && text("#confirm-q").includes("forgotten-key") && !calls.some(c => c[1] === "connections/revoke"), text("#confirm-q"));
  const hold = $("#confirm-hold");
  hold.setPointerCapture = () => {};
  const press = name => hold.dispatchEvent(new w.Event(name, { bubbles: true }));
  click(hold);
  check("a plain click does not do it", !calls.some(c => c[1] === "connections/revoke") && $("#confirm-dlg").hasAttribute("open"));
  press("pointerdown"); await sleep(300); press("pointerup"); await sleep(1200);
  check("letting go early does not do it", !calls.some(c => c[1] === "connections/revoke") && $("#confirm-dlg").hasAttribute("open"));
  check("the button fills while held, and empties when let go", !hold.classList.contains("holding"));
  press("pointerdown"); check("it fills while held", hold.classList.contains("holding"));
  await sleep(1500);
  const revoked = calls.filter(c => c[1] === "connections/revoke").pop();
  check("holding it revokes that connection and closes the confirmation", revoked && JSON.parse(revoked[2]).agent === "forgotten-key" && !$("#confirm-dlg").hasAttribute("open"), revoked && revoked[2]);

  // --- what apps proposed as their own instructions
  check("nothing proposed: the section is hidden", $("#app-guides-section").hidden);
  world.app_guides.push({ space: "health", app: "kalios", text: "Write <b>one</b> record per measure.", hash: "h1", standing: "waiting", updated_at: "x", approved_at: null });
  await w.load();
  check("a proposal is shown, as text and never as markup", !$("#app-guides-section").hidden && text("#app-guides").includes("<b>one</b>") && $("#app-guides b") === null);
  check("it says it is waiting for the person", text("#app-guides").includes("waiting for you"));
  click($$("#app-guides button").find(b => b.textContent === "Approve"));
  await sleep(100);
  const approved = calls.filter(c => c[1] === "app-guides/approve").pop();
  check("approving sends what was read, by its digest", approved && JSON.parse(approved[2]).hash === "h1" && JSON.parse(approved[2]).app === "kalios", approved && approved[2]);
  world.app_guides[0].standing = "changed";
  await w.load();
  check("a changed text asks again and cannot be withdrawn blindly", text("#app-guides").includes("changed since you approved") && $$("#app-guides button").some(b => b.textContent === "Approve the new text"));
  world.app_guides.length = 0;
  await w.load();

  // --- export of one's own memory
  const downloads = [];
  w.URL.createObjectURL = () => "blob:test";
  w.URL.revokeObjectURL = () => {};
  w.HTMLAnchorElement.prototype.click = function () { downloads.push(this.getAttribute("download")); };
  click($("#export-mine"));
  await sleep(250);
  check("the export button asks the server for the person's own archive", calls.some(c => c[0] === "GET" && c[1] === "export"));
  check("and hands it over under the name the server gave", downloads.length === 1 && downloads[0] === "memcp-export-sam-20261101.tar", downloads.join());
  world.exportFails = true;
  click($("#export-mine"));
  await sleep(250);
  check("a refusal is said plainly and nothing is downloaded", text("#err").includes("owns no vault") && downloads.length === 1, text("#err"));

  // --- the name is the address; a name in use is refused, a revoked one is free again
  click($$("#spaces button").find(b => b.textContent === "Connect"));
  click($$("#w-platforms button").find(b => b.textContent.startsWith("Claude on the web")));
  check("an OAuth connector has no vault to pick: the consent page asks", $("#w-access select") === null && text("#w-access").includes("consent page"));
  const rn = v => { $("#w-name").value = v; $("#w-name").dispatchEvent(new w.Event("input")); };
  rn("my-laptop");
  check("a name in use is refused", text("#w-name-note").includes("already has this name"), text("#w-name-note"));
  world.connections.find(c => c.agent === "my-laptop").revoked = true;
  await w.load();
  rn("my-laptop");
  check("the name of a revoked connection is free again", text("#w-name-note").includes("/mcp/my-laptop") && !text("#w-name-note").includes("already"), text("#w-name-note"));
  click($("#wizard form button"));

  // --- an app with its own paths, preselected, and a configuration to paste
  click($("#open-wizard"));
  click($$("#w-platforms button").find(b => b.textContent.startsWith("Kalios")));
  check("Kalios is a platform of its own, named kalios", $("#w-name").value === "kalios");
  check("nothing to tick: its paths are preselected", $$("#w-access input[type=checkbox]").length === 0 && text("#w-access").includes("health/sources/kalios") && text("#w-access").includes("health/dossier/kalios") && text("#w-access").includes("health/proposals/kalios") && text("#w-access").includes("health/guide"), text("#w-access"));
  click($("#w-create"));
  await sleep(300);
  const kal = calls.filter(c => c[1] === "connections").pop();
  const kb = kal && JSON.parse(kal[2]);
  check("it creates exactly those paths, and nothing else", kb && kb.agent === "kalios" && kb.rights.split(",").sort().join() === "health/dossier/kalios:rw,health/guide:rw,health/proposals/kalios:rw,health/sources/kalios:rw", kal && kal[2]);
  const cfg = $$("#w-steps pre").map(p => p.textContent).find(t => t.includes("mcpServers"));
  const parsed = cfg && JSON.parse(cfg);
  check("it gives a configuration to paste: one server, its address, its key", parsed && Object.keys(parsed.mcpServers).join() === "kalios" && parsed.mcpServers.kalios.url === "https://memory.example.net/mcp/kalios" && parsed.mcpServers.kalios.headers.Authorization === "Bearer hcp_secret_token", cfg);
  click($("#wizard form button"));
  console.log(failed ? "\n" + failed + " FAILED" : "\nall wizard checks passed");
  process.exit(failed ? 1 : 0);
})();
