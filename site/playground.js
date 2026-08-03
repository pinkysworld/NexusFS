// Browser driver for the NexusFS WebAssembly core.
//
// The module is plain wasm with no imports — see crates/wasm. Everything here is
// buffer marshalling plus UI; all filesystem behaviour happens inside the same Rust
// code the native binary runs.

// --- wasm bridge -----------------------------------------------------------

const CORE = (() => {
  let wasm = null;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  async function boot() {
    const response = await fetch("./nexusfs.wasm");
    if (!response.ok) throw new Error(`could not load nexusfs.wasm (HTTP ${response.status})`);

    let instance;
    const type = response.headers.get("content-type") || "";
    if (WebAssembly.instantiateStreaming && type.includes("wasm")) {
      ({ instance } = await WebAssembly.instantiateStreaming(response, {}));
    } else {
      // Some static hosts serve .wasm with the wrong content type; fall back.
      ({ instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {}));
    }
    wasm = instance.exports;

    const seed = new Uint8Array(32);
    crypto.getRandomValues(seed);
    const ptr = wasm.nx_alloc(32);
    new Uint8Array(wasm.memory.buffer, ptr, 32).set(seed);
    wasm.nx_seed(ptr);
    wasm.nx_dealloc(ptr, 32);
  }

  function call(request) {
    if (!wasm) throw new Error("core not loaded");
    const bytes = encoder.encode(JSON.stringify(request));
    const ptr = wasm.nx_alloc(bytes.length);
    new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);

    const len = wasm.nx_dispatch(ptr, bytes.length);
    // Re-read: allocation inside dispatch may have grown memory and detached any
    // view taken before the call.
    const out = new Uint8Array(wasm.memory.buffer, wasm.nx_response_ptr(), len);
    const parsed = JSON.parse(decoder.decode(out));
    if (!parsed.ok) throw new Error(parsed.error);
    return parsed.data;
  }

  return { boot, call };
})();

// --- shared state ----------------------------------------------------------

const DEVICES = [
  { index: 0, name: "Laptop" },
  { index: 1, name: "Phone" },
];

const el = (id) => document.getElementById(id);
const now = () => Date.now();

// Fixed timestamps keep the walkthrough reproducible: the same clicks always
// produce the same conflict outcome, so the prose can state what will happen.
let clock = 1000;
const tick = () => (clock += 100);

const offline = [false, false];

// --- primitives ------------------------------------------------------------

function fsCall(op) {
  return CORE.call(op);
}

function state(i) {
  return fsCall({ op: "state", replica: i });
}

function tree(i) {
  return fsCall({ op: "tree", replica: i });
}

function reset() {
  fsCall({ op: "reset", names: DEVICES.map((d) => d.name) });
  clock = 1000;
  offline[0] = false;
  offline[1] = false;
}

function sync(from, to) {
  const payload = fsCall({ op: "export", replica: from });
  const result = fsCall({ op: "import", replica: to, payload });
  log(
    `${DEVICES[from].name} → ${DEVICES[to].name}: ${payload.ops.length} records, ` +
      `${payload.blobs.length} objects, ${result.applied} newly applied`,
    "sync",
  );
  return result;
}

// --- logging ---------------------------------------------------------------

function log(message, kind = "info") {
  const box = el("log");
  if (!box) return;
  box.querySelector(".log-empty")?.remove();

  const line = document.createElement("div");
  line.className = `log-line log-${kind}`;
  const time = document.createElement("span");
  time.className = "log-time";
  time.textContent = new Date().toLocaleTimeString([], { hour12: false });
  line.append(time, document.createTextNode(message));
  box.prepend(line);
  while (box.childElementCount > 60) box.lastElementChild.remove();
}

function clearLog() {
  const box = el("log");
  if (!box) return;
  box.textContent = "";
  const empty = document.createElement("div");
  empty.className = "log-empty";
  empty.textContent = "Nothing yet.";
  box.appendChild(empty);
}

// --- rendering -------------------------------------------------------------

function describe(entries) {
  const files = entries.filter((e) => e.kind === "file").length;
  const dirs = entries.filter((e) => e.kind === "dir").length;
  if (!files && !dirs) return "nothing stored yet";
  const parts = [];
  if (dirs) parts.push(`${dirs} folder${dirs === 1 ? "" : "s"}`);
  if (files) parts.push(`${files} file${files === 1 ? "" : "s"}`);
  return parts.join(", ");
}

/// Conflict names embed a 16-digit device id, which is unreadable in a listing.
/// Elide the middle for display only — the row's tooltip carries the real name.
function displayName(name) {
  const parts = name.match(/^(.*)~conflict-0*([0-9a-f]{1,4})-(\d+)$/);
  return parts ? `${parts[1]}~conflict-${parts[2]}-${parts[3]}` : name;
}

function renderTree(container, entries) {
  container.textContent = "";
  if (!entries.length) {
    const empty = document.createElement("div");
    empty.className = "tree-empty";
    empty.textContent = "(no files)";
    container.appendChild(empty);
    return;
  }

  for (const entry of entries) {
    const row = document.createElement("div");
    row.className = "tree-row";
    row.style.paddingLeft = `${0.2 + entry.depth * 0.9}rem`;
    row.title = entry.path;

    const shown = displayName(entry.name);
    const name = document.createElement("span");
    name.className = entry.kind === "dir" ? "tree-dir" : "tree-file";
    name.textContent = entry.kind === "dir" ? `${shown}/` : shown;
    row.appendChild(name);

    if (entry.name.includes("~conflict-")) {
      const tag = document.createElement("span");
      tag.className = "tree-tag";
      tag.textContent = "kept separately";
      row.appendChild(tag);
    }
    container.appendChild(row);
  }
}

function render() {
  const roots = [];

  for (const d of DEVICES) {
    const i = d.index;
    const s = state(i);
    const entries = tree(i);
    roots.push(s.state_root);

    renderTree(el(`tree-${i}`), entries);
    el(`summary-${i}`).textContent = describe(entries);
    el(`fp-${i}`).textContent = s.state_root.slice(0, 10);

    const conn = el(`conn-${i}`);
    conn.textContent = offline[i] ? "offline" : "online";
    conn.className = `device-state ${offline[i] ? "is-offline" : ""}`;
    el(`device-${i}`).dataset.offline = String(offline[i]);
  }

  const match = roots[0] === roots[1];
  const verdict = el("fp-verdict");
  verdict.textContent = match ? "identical" : "different";
  verdict.className = `fp-verdict ${match ? "is-match" : "is-diff"}`;
  document.querySelectorAll(".fp code").forEach((c) => {
    c.className = match ? "is-match" : "is-diff";
  });

  const anyOffline = offline[0] || offline[1];
  const badge = el("link-badge");
  badge.textContent = anyOffline ? "disconnected" : match ? "in sync" : "out of sync";
  badge.className = `link-badge ${anyOffline ? "is-off" : match ? "is-match" : "is-diff"}`;

  renderDetails();
}

function renderDetails() {
  const grid = el("detail-grid");
  if (!grid || !grid.closest("details").open) return;

  grid.textContent = "";
  for (const d of DEVICES) {
    const s = state(d.index);
    const card = document.createElement("div");
    card.className = "detail-card";
    card.innerHTML = `
      <h4>${d.name}</h4>
      <dl>
        <div><dt>State root</dt><dd><code>${s.state_root.slice(0, 24)}…</code></dd></div>
        <div><dt>Operations</dt><dd>${s.ops}</dd></div>
        <div><dt>Applied</dt><dd>${s.applied}</dd></div>
        <div><dt>Waiting on dependencies</dt><dd>${s.pending}</dd></div>
        <div><dt>Stored objects</dt><dd>${s.blob_count}</dd></div>
        <div><dt>Device id</dt><dd><code>${s.device_id}</code></dd></div>
      </dl>`;
    grid.appendChild(card);
  }
}

// --- tutorial --------------------------------------------------------------

const say = (html) => html;

const STEPS = [
  {
    title: "Two devices, both empty",
    body: say(`
      <p>Below are a laptop and a phone. Each one stores its own copy of the same
      filesystem — there is no server in the middle holding the real version.</p>
      <p>Right now both are empty, so their <strong>fingerprints match</strong>. A
      fingerprint is a short summary of everything a device is storing. If two
      fingerprints are identical, the devices hold identical files.</p>`),
    action: {
      label: "Start",
      run() {},
    },
    after: say(`<p>Both devices agree, because both hold nothing. Let's change that.</p>`),
  },

  {
    title: "Write a file on the laptop",
    body: say(`
      <p>We'll create a folder called <code>/notes</code> on the laptop and save a file
      inside it.</p>
      <p>Watch the phone while this happens: it will not change. The two devices are not
      connected yet, so the phone has no idea any of this occurred.</p>`),
    action: {
      label: "Create /notes/todo.txt on the laptop",
      run() {
        fsCall({ op: "mkdir", replica: 0, path: "/notes", now: tick() });
        fsCall({ op: "put", replica: 0, path: "/notes/todo.txt", content: "buy milk", now: tick() });
        log("Laptop created /notes/todo.txt", "ok");
      },
    },
    after: say(`
      <p>The laptop now has a file and the phone still has nothing, so their fingerprints
      no longer match — the panel between them reads <strong>out of sync</strong>.</p>
      <p>Nothing is wrong here. This is just what two devices look like before they talk.</p>`),
  },

  {
    title: "Connect them",
    body: say(`
      <p>Now let the laptop send what it has to the phone.</p>
      <p>What actually travels is not the folder as you see it. The laptop sends a list of
      <em>what it did</em> — "created a folder called notes", "saved this file" — along with
      the file contents those records refer to. The phone replays those actions itself.</p>`),
    action: {
      label: "Sync laptop → phone",
      run() {
        sync(0, 1);
      },
    },
    after: say(`
      <p>The phone replayed the laptop's actions and now holds the same file. The
      fingerprints match again.</p>
      <p>Sending actions rather than files is what makes the next part work.</p>`),
  },

  {
    title: "Now take both devices offline",
    body: say(`
      <p>Here is the situation that breaks naive file sync: you are on a train, both
      devices are disconnected, and you edit on both.</p>
      <p>Neither device can ask the other what to do. Neither can ask a server. They each
      have to accept the change and sort it out later.</p>`),
    action: {
      label: "Disconnect both",
      run() {
        offline[0] = true;
        offline[1] = true;
        log("both devices went offline", "warn");
      },
    },
    after: say(`<p>Both are offline. Any edit now happens in isolation.</p>`),
  },

  {
    title: "Edit the same file on both devices",
    body: say(`
      <p>We'll change <code>/notes/todo.txt</code> on the laptop and on the phone, to
      different text, while neither can see the other.</p>
      <p>This is the case that has no automatically "correct" answer. Two people edited the
      same thing. Something has to give.</p>`),
    action: {
      label: "Edit on both devices",
      run() {
        fsCall({ op: "put", replica: 0, path: "/notes/todo.txt", content: "buy milk and eggs", now: 5000 });
        fsCall({ op: "put", replica: 1, path: "/notes/todo.txt", content: "buy milk and bread", now: 5000 });
        log("Laptop wrote 'buy milk and eggs'", "ok");
        log("Phone wrote 'buy milk and bread'", "ok");
      },
    },
    after: say(`
      <p>Both edits succeeded locally — neither device refused the write or made you wait.
      Their fingerprints are different again.</p>`),
  },

  {
    title: "Reconnect and reconcile",
    body: say(`
      <p>Now bring them back together and sync in both directions.</p>
      <p>For a file edited in two places, NexusFS has to pick one version. It uses a rule
      both devices can apply independently: newest edit wins, and if the timestamps tie —
      as they do here — the higher device identifier breaks it. Neither device asks the
      other. They just both follow the same rule and land on the same answer.</p>`),
    action: {
      label: "Reconnect and sync both ways",
      run() {
        offline[0] = false;
        offline[1] = false;
        sync(0, 1);
        sync(1, 0);
      },
    },
    after: say(`
      <p>Matching fingerprints: both devices agree on which version of the file survived,
      without negotiating.</p>
      <p>Losing an edit is unavoidable when two people overwrite the same file. What matters
      is that both devices lose the <em>same</em> one, so they never drift apart. The
      discarded version is still in the history.</p>`),
  },

  {
    title: "A conflict where nothing is lost",
    body: say(`
      <p>Overwriting one file is the harsh case. Folders are different — there is no reason
      to throw either one away.</p>
      <p>We'll disconnect again and create a folder with the <em>same name</em> on both
      devices, each holding a different file.</p>`),
    action: {
      label: "Create /trip on both devices",
      run() {
        offline[0] = true;
        offline[1] = true;
        fsCall({ op: "mkdir", replica: 0, path: "/trip", now: 7000 });
        fsCall({ op: "put", replica: 0, path: "/trip/flights.txt", content: "BA 2490", now: 7100 });
        fsCall({ op: "mkdir", replica: 1, path: "/trip", now: 7200 });
        fsCall({ op: "put", replica: 1, path: "/trip/hotel.txt", content: "Pension Marta", now: 7300 });
        log("both devices created /trip while offline", "warn");
      },
    },
    after: say(`
      <p>Two different folders, same name, neither aware of the other.</p>`),
  },

  {
    title: "Watch both folders survive",
    body: say(`
      <p>Sync them together and look at what happens to <code>/trip</code>.</p>`),
    action: {
      label: "Sync both ways",
      run() {
        offline[0] = false;
        offline[1] = false;
        sync(0, 1);
        sync(1, 0);
      },
    },
    after: say(`
      <p>Both folders are still there. One kept the name <code>/trip</code>; the other was
      renamed and marked <em>kept separately</em>, so you can see it and decide what to do.
      Your flight details and your hotel details both survived.</p>
      <p>The new name is not random. Both devices calculated it from the same inputs and
      arrived at the same string independently — which is why the fingerprints still
      match.</p>`),
  },

  {
    title: "That's the whole idea",
    body: say(`
      <p>You just watched a filesystem accept writes on two disconnected devices and
      reconcile them with no server and no negotiation, arriving at byte-identical state on
      both sides.</p>
      <p>Try breaking it. Take a device offline, make changes, sync in one direction only,
      sync repeatedly, sync in the wrong order. The two fingerprints should always end up
      identical once both devices have seen the same set of changes.</p>`),
    action: {
      label: "Open free play",
      run() {},
    },
  },
];

const tutorial = { step: 0, acted: false };

function renderProgress() {
  const bar = el("progress");
  bar.textContent = "";
  STEPS.forEach((_, i) => {
    const dot = document.createElement("span");
    dot.className = "dot" + (i < tutorial.step ? " is-done" : i === tutorial.step ? " is-current" : "");
    bar.appendChild(dot);
  });
}

function renderStep() {
  const step = STEPS[tutorial.step];

  el("step-count").textContent = `Step ${tutorial.step + 1} of ${STEPS.length}`;
  el("step-title").textContent = step.title;
  el("step-body").innerHTML = step.body;

  const result = el("step-result");
  const action = el("step-action");
  const next = el("step-next");
  const back = el("step-back");

  back.hidden = tutorial.step === 0;

  if (!tutorial.acted) {
    result.hidden = true;
    action.hidden = false;
    action.textContent = step.action.label;
    next.hidden = true;
  } else {
    action.hidden = true;
    if (step.after) {
      result.innerHTML = step.after;
      result.hidden = false;
    } else {
      result.hidden = true;
    }
    const last = tutorial.step === STEPS.length - 1;
    next.hidden = last;
    next.textContent = "Next →";
  }

  renderProgress();
}

function runStep() {
  const step = STEPS[tutorial.step];
  try {
    step.action.run();
  } catch (err) {
    log(String(err.message || err), "error");
  }
  tutorial.acted = true;

  if (tutorial.step === STEPS.length - 1) {
    enterFreePlay();
    return;
  }
  render();
  renderStep();
}

function nextStep() {
  tutorial.step = Math.min(tutorial.step + 1, STEPS.length - 1);
  tutorial.acted = false;
  renderStep();
}

function prevStep() {
  tutorial.step = Math.max(tutorial.step - 1, 0);
  tutorial.acted = false;
  renderStep();
}

function enterFreePlay() {
  el("tutorial").hidden = true;
  el("freeplay").hidden = false;
  el("closing").hidden = false;
  el("closing").style.opacity = "1";
  el("closing").style.transform = "none";
  render();
  el("freeplay").scrollIntoView({ block: "start", behavior: "smooth" });
}

// --- free play -------------------------------------------------------------

function guard(fn) {
  try {
    fn();
  } catch (err) {
    log(String(err.message || err), "error");
  }
  render();
}

function wireFreePlay() {
  for (const d of DEVICES) {
    const i = d.index;
    const path = () => el(`path-${i}`).value.trim();

    el(`mkdir-${i}`).addEventListener("click", () =>
      guard(() => {
        fsCall({ op: "mkdir", replica: i, path: path(), now: now() });
        log(`${d.name} created folder ${path()}`, "ok");
      }),
    );

    el(`put-${i}`).addEventListener("click", () =>
      guard(() => {
        const content = el(`content-${i}`).value;
        fsCall({ op: "put", replica: i, path: path(), content, now: now() });
        log(`${d.name} saved ${path()} (${content.length} bytes)`, "ok");
      }),
    );

    el(`cat-${i}`).addEventListener("click", () =>
      guard(() => {
        const body = fsCall({ op: "cat", replica: i, path: path() });
        log(`${d.name} read ${path()} → ${JSON.stringify(body)}`, "ok");
      }),
    );

    el(`rm-${i}`).addEventListener("click", () =>
      guard(() => {
        fsCall({ op: "rm", replica: i, path: path(), now: now() });
        log(`${d.name} deleted ${path()}`, "ok");
      }),
    );

    el(`offline-${i}`).addEventListener("click", (event) =>
      guard(() => {
        offline[i] = !offline[i];
        event.target.textContent = offline[i] ? "go online" : "go offline";
        log(`${d.name} went ${offline[i] ? "offline" : "online"}`, "warn");
      }),
    );
  }

  const blocked = (a, b) =>
    offline[a] || offline[b]
      ? (log("cannot sync while a device is offline", "error"), true)
      : false;

  el("sync-ab").addEventListener("click", () => guard(() => !blocked(0, 1) && sync(0, 1)));
  el("sync-ba").addEventListener("click", () => guard(() => !blocked(1, 0) && sync(1, 0)));
  el("sync-both").addEventListener("click", () =>
    guard(() => {
      if (blocked(0, 1)) return;
      sync(0, 1);
      sync(1, 0);
    }),
  );

  el("restart").addEventListener("click", () =>
    guard(() => {
      reset();
      clearLog();
      document.querySelectorAll('[id^="offline-"]').forEach((b) => (b.textContent = "go offline"));
      log("started over");
    }),
  );

  el("detail-grid").closest("details").addEventListener("toggle", renderDetails);
}

// --- boot ------------------------------------------------------------------

(async function main() {
  const status = el("boot-status");
  try {
    await CORE.boot();
    reset();
    clearLog();

    status.hidden = true;
    el("tutorial").hidden = false;
    el("stage").hidden = false;

    el("step-action").addEventListener("click", runStep);
    el("step-next").addEventListener("click", nextStep);
    el("step-back").addEventListener("click", prevStep);
    el("skip").addEventListener("click", () => {
      reset();
      clearLog();
      enterFreePlay();
    });

    wireFreePlay();
    render();
    renderStep();
  } catch (err) {
    status.className = "verdict verdict-error";
    status.textContent = `Could not start the playground: ${err.message}`;
  }
})();
