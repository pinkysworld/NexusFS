// Browser driver for the NexusFS WebAssembly core.
//
// The module is plain wasm with no imports — see crates/wasm. Everything below is
// buffer marshalling plus UI; all filesystem behaviour happens inside the same Rust
// code the native binary runs.

const PLAYGROUND = (() => {
  let wasm = null;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  async function boot() {
    const response = await fetch("./nexusfs.wasm");
    if (!response.ok) throw new Error(`could not load nexusfs.wasm (HTTP ${response.status})`);

    let instance;
    if (WebAssembly.instantiateStreaming && response.headers.get("content-type")?.includes("wasm")) {
      ({ instance } = await WebAssembly.instantiateStreaming(response, {}));
    } else {
      // Some static hosts serve .wasm with the wrong content type; fall back.
      const bytes = await response.arrayBuffer();
      ({ instance } = await WebAssembly.instantiate(bytes, {}));
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
    if (!wasm) throw new Error("wasm not loaded");
    const bytes = encoder.encode(JSON.stringify(request));
    const ptr = wasm.nx_alloc(bytes.length);
    new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);

    const len = wasm.nx_dispatch(ptr, bytes.length);
    // Re-read the buffer each time: allocation inside dispatch may have grown memory
    // and detached any view taken earlier.
    const out = new Uint8Array(wasm.memory.buffer, wasm.nx_response_ptr(), len);
    const parsed = JSON.parse(decoder.decode(out));
    if (!parsed.ok) throw new Error(parsed.error);
    return parsed.data;
  }

  return { boot, call };
})();

const NODES = [
  { index: 0, name: "Replica A" },
  { index: 1, name: "Replica B" },
];

const state = { linked: true };

function now() {
  return Date.now();
}

function log(message, kind = "info") {
  const el = document.getElementById("log");
  const line = document.createElement("div");
  line.className = `log-line log-${kind}`;
  const time = new Date().toLocaleTimeString();
  line.textContent = `${time}  ${message}`;
  el.prepend(line);
  while (el.childElementCount > 60) el.lastElementChild.remove();
}

function guard(fn) {
  try {
    fn();
  } catch (err) {
    log(String(err.message || err), "error");
  }
  render();
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  return `${(n / 1024).toFixed(1)} KiB`;
}

function render() {
  const roots = [];

  for (const node of NODES) {
    const st = PLAYGROUND.call({ op: "state", replica: node.index });
    roots.push(st.state_root);

    document.getElementById(`root-${node.index}`).textContent = st.state_root.slice(0, 24) + "…";
    document.getElementById(`ops-${node.index}`).textContent = st.ops;
    document.getElementById(`pending-${node.index}`).textContent = st.pending;
    document.getElementById(`blobs-${node.index}`).textContent =
      `${st.blob_count} (${formatBytes(st.blob_bytes)})`;

    const tree = PLAYGROUND.call({ op: "tree", replica: node.index });
    const list = document.getElementById(`tree-${node.index}`);
    list.textContent = "";

    if (tree.length === 0) {
      const empty = document.createElement("div");
      empty.className = "tree-empty";
      empty.textContent = "(empty)";
      list.appendChild(empty);
    }

    for (const entry of tree) {
      const row = document.createElement("div");
      row.className = "tree-row";
      row.style.paddingLeft = `${entry.depth * 16}px`;

      const name = document.createElement("span");
      name.className = entry.kind === "dir" ? "tree-dir" : "tree-file";
      name.textContent = entry.kind === "dir" ? `${entry.name}/` : entry.name;
      row.appendChild(name);

      if (entry.name.includes("~conflict-")) {
        const tag = document.createElement("span");
        tag.className = "tree-tag";
        tag.textContent = "conflict";
        row.appendChild(tag);
      }

      if (entry.kind === "file") {
        const size = document.createElement("span");
        size.className = "tree-size";
        size.textContent = formatBytes(entry.size);
        row.appendChild(size);
      }

      list.appendChild(row);
    }
  }

  const converged = roots.every((r) => r === roots[0]);
  const banner = document.getElementById("converged");
  banner.className = `verdict ${converged ? "verdict-ok" : "verdict-diverged"}`;
  banner.textContent = converged
    ? "State roots match — the replicas have converged"
    : "State roots differ — the replicas have diverged";
}

function sync(from, to) {
  const payload = PLAYGROUND.call({ op: "export", replica: from });
  const result = PLAYGROUND.call({ op: "import", replica: to, payload });
  log(
    `sync ${NODES[from].name} → ${NODES[to].name}: carried ${payload.ops.length} ops, ` +
      `${payload.blobs.length} blobs, applied ${result.applied}`,
    result.applied > 0 ? "ok" : "info",
  );
}

function resetAll() {
  PLAYGROUND.call({ op: "reset", names: NODES.map((n) => n.name) });
  log("reset both replicas to an empty filesystem");
}

function wire() {
  for (const node of NODES) {
    const i = node.index;

    document.getElementById(`mkdir-${i}`).addEventListener("click", () =>
      guard(() => {
        const path = document.getElementById(`path-${i}`).value.trim();
        PLAYGROUND.call({ op: "mkdir", replica: i, path, now: now() });
        log(`${node.name}: mkdir ${path}`, "ok");
      }),
    );

    document.getElementById(`put-${i}`).addEventListener("click", () =>
      guard(() => {
        const path = document.getElementById(`path-${i}`).value.trim();
        const content = document.getElementById(`content-${i}`).value;
        PLAYGROUND.call({ op: "put", replica: i, path, content, now: now() });
        log(`${node.name}: wrote ${content.length} bytes to ${path}`, "ok");
      }),
    );

    document.getElementById(`cat-${i}`).addEventListener("click", () =>
      guard(() => {
        const path = document.getElementById(`path-${i}`).value.trim();
        const body = PLAYGROUND.call({ op: "cat", replica: i, path });
        log(`${node.name}: cat ${path} → ${JSON.stringify(body)}`, "ok");
      }),
    );

    document.getElementById(`rm-${i}`).addEventListener("click", () =>
      guard(() => {
        const path = document.getElementById(`path-${i}`).value.trim();
        PLAYGROUND.call({ op: "rm", replica: i, path, now: now() });
        log(`${node.name}: rm ${path}`, "ok");
      }),
    );
  }

  document.getElementById("sync-ab").addEventListener("click", () => guard(() => sync(0, 1)));
  document.getElementById("sync-ba").addEventListener("click", () => guard(() => sync(1, 0)));
  document.getElementById("sync-both").addEventListener("click", () =>
    guard(() => {
      // Two one-way transfers, as a real bidirectional session would do.
      sync(0, 1);
      sync(1, 0);
    }),
  );
  document.getElementById("reset").addEventListener("click", () => guard(resetAll));

  document.getElementById("scenario-partition").addEventListener("click", () =>
    guard(() => {
      resetAll();
      PLAYGROUND.call({ op: "mkdir", replica: 0, path: "/shared", now: 1000 });
      PLAYGROUND.call({ op: "put", replica: 0, path: "/shared/from-a.txt", content: "written on A", now: 1100 });
      PLAYGROUND.call({ op: "mkdir", replica: 1, path: "/shared", now: 2000 });
      PLAYGROUND.call({ op: "put", replica: 1, path: "/shared/from-b.txt", content: "written on B", now: 2100 });
      log("both replicas independently created /shared while partitioned", "warn");
      log("now press “Sync both ways” — both /shared links survive, one keeps the plain name", "info");
    }),
  );

  document.getElementById("scenario-offline").addEventListener("click", () =>
    guard(() => {
      resetAll();
      PLAYGROUND.call({ op: "mkdir", replica: 0, path: "/notes", now: 1000 });
      PLAYGROUND.call({ op: "put", replica: 0, path: "/notes/todo.md", content: "buy milk\nship M2", now: 1100 });
      PLAYGROUND.call({ op: "put", replica: 0, path: "/notes/long.txt", content: "y".repeat(200), now: 1200 });
      log("A worked offline and built a tree; B has never seen it", "warn");
      log("press “A → B” to replicate the oplog and its blobs", "info");
    }),
  );
}

(async function main() {
  const status = document.getElementById("boot-status");
  try {
    await PLAYGROUND.boot();
    resetAll();
    wire();
    render();
    status.textContent = "";
    status.hidden = true;
  } catch (err) {
    status.className = "verdict verdict-diverged";
    status.textContent = `Could not start the playground: ${err.message}`;
  }
})();
