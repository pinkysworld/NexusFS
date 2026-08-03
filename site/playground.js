// Browser driver for the NexusFS WebAssembly core.
//
// The module is plain wasm with no imports — see crates/wasm. Everything here is
// buffer marshalling plus UI; all filesystem behaviour happens inside the same Rust
// code the native binary runs.

const CORE = (() => {
  let wasm = null;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  async function boot() {
    const response = await fetch("./nexusfs.wasm");
    if (!response.ok) {
      throw new Error(`could not load nexusfs.wasm (HTTP ${response.status})`);
    }

    let instance;
    const type = response.headers.get("content-type") || "";
    if (WebAssembly.instantiateStreaming && type.includes("wasm")) {
      ({ instance } = await WebAssembly.instantiateStreaming(response, {}));
    } else {
      // Some static hosts serve .wasm with the wrong content type; fall back.
      ({ instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {}));
    }
    wasm = instance.exports;

    // Seed the module's entropy pool from the browser CSPRNG.
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
    // Re-read the buffer: allocation inside dispatch may have grown wasm memory and
    // detached any view taken before the call.
    const out = new Uint8Array(wasm.memory.buffer, wasm.nx_response_ptr(), len);
    const parsed = JSON.parse(decoder.decode(out));
    if (!parsed.ok) throw new Error(parsed.error);
    return parsed.data;
  }

  return { boot, call };
})();

const NODES = [
  { index: 0, label: "A" },
  { index: 1, label: "B" },
];

const el = (id) => document.getElementById(id);
const now = () => Date.now();

// --- logging ---------------------------------------------------------------

function log(message, kind = "info") {
  const box = el("log");
  box.querySelector(".log-empty")?.remove();

  const line = document.createElement("div");
  line.className = `log-line log-${kind}`;

  const time = document.createElement("span");
  time.className = "log-time";
  time.textContent = new Date().toLocaleTimeString([], { hour12: false });
  line.append(time, document.createTextNode(message));

  box.prepend(line);
  while (box.childElementCount > 80) box.lastElementChild.remove();
}

function clearLog() {
  const box = el("log");
  box.textContent = "";
  const empty = document.createElement("div");
  empty.className = "log-empty";
  empty.textContent = "No activity yet.";
  box.appendChild(empty);
}

function hint(text) {
  const node = el("scenario-hint");
  if (!text) {
    node.hidden = true;
    return;
  }
  node.textContent = text;
  node.hidden = false;
}

/** Run an action, surfacing failures in the log rather than the console. */
function guard(fn) {
  try {
    fn();
  } catch (err) {
    log(String(err.message || err), "error");
  }
  render();
}

// --- rendering -------------------------------------------------------------

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  return `${(n / 1024).toFixed(1)} KiB`;
}

function renderTree(container, entries) {
  container.textContent = "";

  if (entries.length === 0) {
    const empty = document.createElement("div");
    empty.className = "tree-empty";
    empty.textContent = "empty";
    container.appendChild(empty);
    return;
  }

  for (const entry of entries) {
    const row = document.createElement("div");
    row.className = "tree-row";
    row.style.paddingLeft = `${0.2 + entry.depth * 0.9}rem`;

    const name = document.createElement("span");
    name.className = entry.kind === "dir" ? "tree-dir" : "tree-file";
    name.textContent = entry.kind === "dir" ? `${entry.name}/` : entry.name;
    row.appendChild(name);

    if (entry.name.includes("~conflict-")) {
      const tag = document.createElement("span");
      tag.className = "tree-tag";
      tag.textContent = "renamed";
      row.appendChild(tag);
    }

    if (entry.kind === "file") {
      const size = document.createElement("span");
      size.className = "tree-size";
      size.textContent = formatBytes(entry.size);
      row.appendChild(size);
    }

    container.appendChild(row);
  }
}

function render() {
  const roots = [];
  const panels = document.querySelectorAll(".replica");

  for (const node of NODES) {
    const i = node.index;
    const state = CORE.call({ op: "state", replica: i });
    roots.push(state.state_root);

    el(`device-${i}`).textContent = `device ${state.device_id}`;
    el(`root-${i}`).textContent = state.state_root.slice(0, 32);
    el(`root-${i}`).title = state.state_root;
    el(`ops-${i}`).textContent = state.ops;
    el(`pending-${i}`).textContent = state.pending;
    el(`blobs-${i}`).textContent = state.blob_count;

    renderTree(el(`tree-${i}`), CORE.call({ op: "tree", replica: i }));
  }

  const converged = roots.every((r) => r === roots[0]);
  panels.forEach((p) => p.setAttribute("data-diverged", String(!converged)));

  const banner = el("converged");
  banner.className = `verdict ${converged ? "verdict-ok" : "verdict-diverged"}`;
  banner.textContent = converged
    ? "Converged — both replicas hold an identical filesystem"
    : "Diverged — the replicas hold different state; sync to reconcile";
}

// --- actions ---------------------------------------------------------------

function sync(from, to) {
  const payload = CORE.call({ op: "export", replica: from });
  const result = CORE.call({ op: "import", replica: to, payload });
  log(
    `${NODES[from].label} → ${NODES[to].label}  sent ${payload.ops.length} ops, ` +
      `${payload.blobs.length} chunks · ${result.applied} newly applied`,
    "sync",
  );
}

function reset(quiet) {
  CORE.call({ op: "reset", names: NODES.map((n) => `Replica ${n.label}`) });
  hint("");
  if (!quiet) {
    clearLog();
    log("reset both replicas to an empty filesystem");
  }
}

const SCENARIOS = {
  "scenario-offline": {
    hint:
      "Replica A built a tree while disconnected. B has never seen any of it. " +
      "Press “A → B” to replicate the log and the chunks it references.",
    run() {
      CORE.call({ op: "mkdir", replica: 0, path: "/notes", now: 1000 });
      CORE.call({ op: "put", replica: 0, path: "/notes/todo.md", content: "buy milk\nship M2", now: 1100 });
      CORE.call({ op: "put", replica: 0, path: "/notes/journal.txt", content: "y".repeat(200), now: 1200 });
      log("A worked offline: created /notes with two files", "warn");
    },
  },

  "scenario-partition": {
    hint:
      "Both replicas created /shared while partitioned, each with a different file inside. " +
      "Press “Sync both ways” — both directories survive and one is deterministically renamed.",
    run() {
      CORE.call({ op: "mkdir", replica: 0, path: "/shared", now: 1000 });
      CORE.call({ op: "put", replica: 0, path: "/shared/from-a.txt", content: "written on A", now: 1100 });
      CORE.call({ op: "mkdir", replica: 1, path: "/shared", now: 2000 });
      CORE.call({ op: "put", replica: 1, path: "/shared/from-b.txt", content: "written on B", now: 2100 });
      log("both replicas independently created /shared", "warn");
    },
  },

  "scenario-writes": {
    hint:
      "Both replicas wrote different content to the same file at the same timestamp. " +
      "Sync both ways: one write wins, and both replicas agree on which without negotiating.",
    run() {
      CORE.call({ op: "mkdir", replica: 0, path: "/doc", now: 1000 });
      CORE.call({ op: "put", replica: 0, path: "/doc/report.md", content: "draft", now: 1100 });
      // Give B the file first so both are writing to the same inode.
      const seed = CORE.call({ op: "export", replica: 0 });
      CORE.call({ op: "import", replica: 1, payload: seed });
      CORE.call({ op: "put", replica: 0, path: "/doc/report.md", content: "A's version of the report", now: 5000 });
      CORE.call({ op: "put", replica: 1, path: "/doc/report.md", content: "B's version of the report", now: 5000 });
      log("both replicas rewrote /doc/report.md at the same timestamp", "warn");
    },
  },
};

function wire() {
  for (const node of NODES) {
    const i = node.index;
    const path = () => el(`path-${i}`).value.trim();

    el(`mkdir-${i}`).addEventListener("click", () =>
      guard(() => {
        const p = path();
        CORE.call({ op: "mkdir", replica: i, path: p, now: now() });
        log(`${node.label}  mkdir ${p}`, "ok");
      }),
    );

    el(`put-${i}`).addEventListener("click", () =>
      guard(() => {
        const p = path();
        const content = el(`content-${i}`).value;
        CORE.call({ op: "put", replica: i, path: p, content, now: now() });
        log(`${node.label}  put ${p} (${content.length} bytes)`, "ok");
      }),
    );

    el(`cat-${i}`).addEventListener("click", () =>
      guard(() => {
        const p = path();
        const body = CORE.call({ op: "cat", replica: i, path: p });
        log(`${node.label}  cat ${p} → ${JSON.stringify(body)}`, "ok");
      }),
    );

    el(`rm-${i}`).addEventListener("click", () =>
      guard(() => {
        const p = path();
        CORE.call({ op: "rm", replica: i, path: p, now: now() });
        log(`${node.label}  rm ${p}`, "ok");
      }),
    );
  }

  el("sync-ab").addEventListener("click", () => guard(() => sync(0, 1)));
  el("sync-ba").addEventListener("click", () => guard(() => sync(1, 0)));
  el("sync-both").addEventListener("click", () =>
    guard(() => {
      // Two one-way transfers, as a real bidirectional session would perform.
      sync(0, 1);
      sync(1, 0);
    }),
  );
  el("reset").addEventListener("click", () => guard(() => reset(false)));

  for (const [id, scenario] of Object.entries(SCENARIOS)) {
    el(id).addEventListener("click", () =>
      guard(() => {
        reset(true);
        clearLog();
        scenario.run();
        hint(scenario.hint);
      }),
    );
  }
}

(async function main() {
  const status = el("boot-status");
  try {
    await CORE.boot();
    reset(true);
    clearLog();
    wire();
    render();
    status.hidden = true;
  } catch (err) {
    status.className = "verdict verdict-error";
    status.textContent = `Could not start the playground: ${err.message}`;
  }
})();
