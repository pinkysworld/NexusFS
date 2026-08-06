// NexusFS admin console.
//
// Plain DOM, no framework and no build step: this file ships inside the binary and has
// to work on a node with no internet and no package manager.
//
// Two rules the rest of the file follows:
//
//   1. Text goes in through `textContent`, never `innerHTML`. Paths, peer errors and
//      operation kinds all originate outside this process, and a filename is allowed to
//      contain angle brackets.
//   2. Cheap reads go on the refresh path; anything that walks the whole repository
//      (the audit, the collection survey) runs only when asked. A console that quietly
//      re-audits every few seconds is a console nobody leaves open.

(function () {
  "use strict";

  const TOKEN_KEY = "nexusfs.admin.token";
  const AUTO_KEY = "nexusfs.admin.auto";
  const TAB_KEY = "nexusfs.admin.tab";
  const AUTO_INTERVAL_MS = 5000;

  let currentPath = "/";
  let entries = [];
  let autoTimer = null;
  let inFlight = false;

  // ------------------------------------------------------------------ helpers --

  const $ = (id) => document.getElementById(id);

  function tokenHeader() {
    const t = $("token").value.trim();
    return t ? { "x-nexusfs-token": t } : {};
  }

  async function getJson(path) {
    const res = await fetch(path, { headers: tokenHeader() });
    if (res.status === 401) {
      throw new Error("Unauthorized — check the admin token (nexusfs status prints it).");
    }
    if (!res.ok) {
      throw new Error(`${res.status}: ${(await res.text()).slice(0, 200)}`);
    }
    return res.json();
  }

  function setText(id, value) {
    const el = $(id);
    if (el) el.textContent = value;
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function cell(row, text, className) {
    const td = row.insertCell();
    td.textContent = text;
    if (className) td.className = className;
    return td;
  }

  function emptyRow(body, columns, message) {
    const row = body.insertRow();
    const td = cell(row, message, "empty");
    td.colSpan = columns;
  }

  function formatBytes(n) {
    if (n === null || n === undefined) return "—";
    if (n < 1024) return `${n} B`;
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let value = n / 1024;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i += 1;
    }
    return `${value.toFixed(1)} ${units[i]}`;
  }

  // Ages, not wall-clock times. The interesting question about a sync is "how long ago",
  // and a timestamp makes the reader do that subtraction themselves.
  function relativeTime(ms, nowMs) {
    if (!ms) return "never";
    const seconds = Math.max(0, Math.round((nowMs - ms) / 1000));
    if (seconds < 2) return "just now";
    if (seconds < 60) return `${seconds}s ago`;
    const minutes = Math.round(seconds / 60);
    if (minutes < 60) return `${minutes}m ago`;
    const hours = Math.round(minutes / 60);
    if (hours < 24) return `${hours}h ago`;
    return `${Math.round(hours / 24)}d ago`;
  }

  function shorten(hex, keep = 10) {
    if (!hex) return "—";
    return hex.length <= keep * 2 ? hex : `${hex.slice(0, keep)}…${hex.slice(-4)}`;
  }

  function setPill(id, text, tone) {
    const pill = $(id);
    if (!pill) return;
    pill.className = `pill ${tone}`;
    pill.textContent = text;
  }

  function showBanner(message, tone = "bad") {
    const banner = $("banner");
    banner.textContent = message;
    banner.className = `banner ${tone} show`;
  }

  function clearBanner() {
    const banner = $("banner");
    banner.className = "banner bad";
    // Clear the text too, not just the visibility. A hidden element holding a stale
    // error is a trap for anything that later reads it — including a human debugging
    // through devtools.
    banner.textContent = "";
  }

  // -------------------------------------------------------------------- files --

  function joinPath(base, name) {
    return base === "/" ? `/${name}` : `${base}/${name}`;
  }

  function renderCrumbs() {
    const container = $("crumbs");
    container.textContent = "";

    const root = el("button", "link", "/");
    root.onclick = () => navigate("/");
    container.appendChild(root);

    const parts = currentPath.split("/").filter(Boolean);
    parts.forEach((part, i) => {
      if (i > 0) container.appendChild(el("span", "sep", "/"));
      const crumb = el("button", "link", part);
      const target = "/" + parts.slice(0, i + 1).join("/");
      crumb.onclick = () => navigate(target);
      container.appendChild(crumb);
    });
  }

  function paintListing() {
    const body = $("listing");
    body.textContent = "";

    const needle = $("filter").value.trim().toLowerCase();
    const shown = needle
      ? entries.filter((e) => e.name.toLowerCase().includes(needle))
      : entries;

    if (!shown.length) {
      emptyRow(body, 4, entries.length ? "Nothing matches that filter." : "This directory is empty.");
      return;
    }

    for (const entry of shown) {
      const row = body.insertRow();
      row.className = "hoverable";

      const nameCell = row.insertCell();
      const open = el("button", "link", entry.name);
      const target = joinPath(currentPath, entry.name);
      open.onclick = () =>
        entry.kind === "dir" ? navigate(target) : openFile(target);
      nameCell.appendChild(open);

      cell(row, entry.kind, "dim");
      cell(row, entry.kind === "file" ? formatBytes(entry.size) : "", "num");
      cell(row, shorten(entry.inode, 12), "dim mono nowrap");
    }
  }

  async function loadListing() {
    renderCrumbs();
    const data = await getJson(`/api/fs/ls?path=${encodeURIComponent(currentPath)}`);
    entries = data.entries;
    paintListing();
  }

  function navigate(path) {
    currentPath = path;
    $("filter").value = "";
    closeViewer();
    loadListing().catch(fail);
  }

  async function openFile(path) {
    const data = await getJson(`/api/fs/cat?path=${encodeURIComponent(path)}`);
    $("viewerCard").hidden = false;
    setText("viewerPath", data.path);
    setPill("viewerMeta", `${data.kind} · ${formatBytes(data.size)}`, "neutral");

    const note = $("viewerNote");
    if (data.kind === "binary") {
      // Never dump binary into the page: it would corrupt the layout and tell the
      // reader nothing. The first bytes are enough to recognise a format.
      $("viewerBody").textContent = data.preview_hex || "";
      note.textContent = "Binary content. Showing the first bytes as hex.";
      note.hidden = false;
    } else {
      $("viewerBody").textContent = data.content || "";
      if (data.truncated) {
        note.textContent = "Truncated. Use nexusfs cat to read the whole file.";
        note.hidden = false;
      } else {
        note.hidden = true;
      }
    }
    $("viewerCard").scrollIntoView({ behavior: "smooth", block: "nearest" });
  }

  function closeViewer() {
    $("viewerCard").hidden = true;
  }

  // -------------------------------------------------------------- replication --

  async function renderPeers(nowMs) {
    const body = $("peers");
    body.textContent = "";
    const data = await getJson("/api/peers");

    if (!data.enabled) {
      emptyRow(body, 7, "Replication is not compiled into this build.");
      return;
    }
    if (!data.peers.length) {
      emptyRow(body, 7, "No peer contacted yet. Set net.peers to enable replication.");
      return;
    }

    for (const p of data.peers) {
      const row = body.insertRow();
      cell(row, p.address, "mono nowrap");
      cell(row, p.device_id ? shorten(p.device_id, 8) : "—", "dim mono nowrap");
      cell(row, p.ops_received, "num");
      cell(row, formatBytes(p.content_bytes || 0), "num");
      cell(row, p.syncs, "num");
      cell(row, relativeTime(p.last_success_ms, nowMs), "dim nowrap");

      const state = row.insertCell();
      if (p.last_error) {
        state.appendChild(pill(p.last_error, "bad"));
      } else if (p.content_deferred) {
        // Distinct from "synced": the namespace is current but the energy budget
        // stopped short of fetching every byte.
        state.appendChild(pill("content deferred", "warn"));
      } else if (p.last_success_ms) {
        state.appendChild(pill("synced", "ok"));
      } else {
        state.appendChild(pill("no contact", "neutral"));
      }
    }
  }

  function pill(text, tone) {
    return el("span", `pill ${tone}`, text);
  }

  async function renderEnrolled() {
    const body = $("enrolled");
    body.textContent = "";
    const peers = await getJson("/api/peers/enrolled");

    setPill("trustCount", `${peers.length} pinned`, peers.length ? "ok" : "neutral");

    if (!peers.length) {
      emptyRow(body, 3, "No keys pinned. With net.tofu on, the first peer seen is trusted.");
      return;
    }

    for (const p of peers) {
      const row = body.insertRow();
      cell(row, shorten(p.device_id, 10), "mono nowrap");
      cell(row, shorten(p.pubkey, 10), "dim mono nowrap");

      const actions = row.insertCell();
      const copy = el("button", "ghost", "copy");
      copy.onclick = () => copyText(`${p.device_id} ${p.pubkey}`, copy);
      actions.appendChild(copy);
    }
  }

  async function renderIdentity() {
    const id = await getJson("/api/identity");
    setText("idDevice", id.device_id);
    setText("idPubkey", id.pubkey || "unavailable in this build");
    setText("deviceChip", shorten(id.device_id, 8));
    $("deviceChip").title = `Device ${id.device_id}`;

    setText(
      "enrolCmd",
      id.pubkey
        ? `nexusfs peer add --config <peer-config> ${id.device_id} ${id.pubkey}`
        : "unavailable in this build"
    );

    // A format mismatch means this daemon should not be running against this store at
    // all, so it is worth shouting about rather than tucking into a table.
    if (id.format_version === null || id.format_version === undefined) {
      setPill("formatPill", "format unstamped", "warn");
    } else if (id.format_version === id.expects_format) {
      setPill("formatPill", `format v${id.format_version}`, "neutral");
    } else {
      setPill("formatPill", `format v${id.format_version} ≠ v${id.expects_format}`, "bad");
      showBanner(
        `On-disk format is v${id.format_version} but this build expects ` +
          `v${id.expects_format}. Run nexusfs migrate, or use a matching build.`
      );
    }
  }

  // -------------------------------------------------------------- operations --

  let recentOps = [];

  function paintOps() {
    const body = $("ops");
    body.textContent = "";

    const onlyPending = $("pendingOnly").checked;
    const shown = onlyPending ? recentOps.filter((o) => !o.applied) : recentOps;

    if (!shown.length) {
      emptyRow(body, 4, onlyPending ? "Nothing is parked." : "No operations yet.");
      return;
    }

    for (const op of shown) {
      const row = body.insertRow();
      cell(row, shorten(op.device_id, 8), "dim mono nowrap");
      cell(row, op.counter, "num");
      cell(row, op.kind);
      const state = row.insertCell();
      state.appendChild(
        op.applied ? pill("applied", "ok") : pill("waiting on content", "warn")
      );
    }
  }

  async function renderOps() {
    recentOps = await getJson("/api/oplog/recent?limit=50");
    paintOps();
  }

  async function renderClock() {
    const body = $("clock");
    body.textContent = "";
    const summary = await getJson("/api/oplog/summary");

    const rows = (summary && summary.entries) || [];
    if (!rows.length) {
      emptyRow(body, 2, "No operations applied yet.");
      return;
    }
    for (const entry of rows) {
      const row = body.insertRow();
      cell(row, shorten(entry.device_id, 10), "mono nowrap");
      cell(row, entry.through, "num");
    }
  }

  // ------------------------------------------------------------- maintenance --

  async function runAudit() {
    const button = $("auditBtn");
    button.disabled = true;
    setPill("auditPill", "running…", "neutral");
    try {
      const v = await getJson("/api/security");
      setText("vOps", v.ops_total);
      setText("vSigs", v.signature_failures);
      setText("vMalformed", v.malformed_proofs);
      setText("vNoProof", v.ops_without_proof);
      setText("vUnreadable", v.unreadable_files.length);
      setText("vEncryption", v.encryption_at_rest ? "on" : "off");
      setText("vPolicy", v.proof_policy);
      setPill("auditPill", v.healthy ? "verified" : "problems found", v.healthy ? "ok" : "bad");

      const files = $("vFiles");
      files.textContent = "";
      if (v.unreadable_files.length) {
        files.appendChild(el("div", "dim", "Unreadable:"));
        for (const path of v.unreadable_files.slice(0, 20)) {
          files.appendChild(el("div", "mono", path));
        }
        if (v.unreadable_files.length > 20) {
          files.appendChild(el("div", "faint", `…and ${v.unreadable_files.length - 20} more`));
        }
      }
    } catch (e) {
      setPill("auditPill", "failed", "bad");
      fail(e);
    } finally {
      button.disabled = false;
    }
  }

  async function runSurvey() {
    const button = $("gcBtn");
    button.disabled = true;
    setPill("gcPill", "surveying…", "neutral");
    try {
      const g = await getJson("/api/storage/gc");
      setText("gcReachable", g.reachable);
      setText("gcUnreachable", g.unreachable);
      setText("gcBytes", formatBytes(g.bytes_reclaimable));

      const share = g.bytes_scanned ? g.bytes_reclaimable / g.bytes_scanned : 0;
      setText("gcPct", `${Math.round(share * 100)}%`);
      $("gcBar").style.width = `${Math.round(share * 100)}%`;

      if (g.refused) {
        setPill("gcPill", "refused", "bad");
        showBanner(`Collection would refuse: ${g.refused}`, "warn");
      } else {
        setPill("gcPill", g.unreachable ? "reclaimable" : "nothing to do", g.unreachable ? "warn" : "ok");
      }
    } catch (e) {
      setPill("gcPill", "failed", "bad");
      fail(e);
    } finally {
      button.disabled = false;
    }
  }

  const COMMANDS = [
    ["nexusfs status", "Head, state root, operation counts and the admin token."],
    ["nexusfs verify", "Audit every signature, proof and file. Exits non-zero on failure."],
    ["nexusfs gc --apply", "Reclaim unreachable storage. Survey first without the flag."],
    ["nexusfs migrate", "Upgrade the on-disk format. Back the data directory up first."],
    ["nexusfs peer identity", "Print what another node needs in order to enrol this one."],
    ["nexusfs peer add <device> <key>", "Trust a peer ahead of first contact."],
    ["nexusfs peer list", "Show the pinned keys."],
    ["nexusfs peer remove <device>", "Forget a pinned key."],
  ];

  function renderCommands() {
    const body = $("cmds");
    body.textContent = "";
    for (const [command, description] of COMMANDS) {
      const row = body.insertRow();
      row.className = "hoverable";
      const first = row.insertCell();
      first.appendChild(el("code", "", command));
      cell(row, description, "dim");
    }
  }

  // ----------------------------------------------------------------- plumbing --

  async function copyText(text, button) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Clipboard access needs a secure context, which plain http://127.0.0.1 is —
      // but a remote-bound console over http is not. Fall back to selection so the
      // value is still obtainable.
      const area = document.createElement("textarea");
      area.value = text;
      document.body.appendChild(area);
      area.select();
      try {
        document.execCommand("copy");
      } catch {
        /* nothing left to try; the text is at least selected */
      }
      area.remove();
    }
    if (button) {
      const original = button.textContent;
      button.textContent = "copied";
      setTimeout(() => {
        button.textContent = original;
      }, 1200);
    }
  }

  function selectTab(name) {
    for (const tab of document.querySelectorAll(".tab")) {
      const selected = tab.dataset.tab === name;
      tab.setAttribute("aria-selected", String(selected));
    }
    for (const panel of document.querySelectorAll(".panel")) {
      panel.hidden = panel.id !== `panel-${name}`;
    }
    localStorage.setItem(TAB_KEY, name);
  }

  function fail(err) {
    const message = String(err && err.message ? err.message : err);

    // A node with a token configured answers 401 until one is supplied, which on a
    // first visit is expected rather than broken. Say what to do instead of reporting
    // a failure the operator has not caused yet.
    if (message.startsWith("Unauthorized") && !$("token").value.trim()) {
      showBanner(
        "This node requires an admin token. Paste it above — `nexusfs status` prints it.",
        "warn"
      );
      $("token").focus();
      return;
    }
    showBanner(message);
  }

  function setHealth(text, tone) {
    const pill = $("healthPill");
    pill.className = `pill ${tone}`;
    pill.textContent = "";
    pill.appendChild(el("span", "dot"));
    pill.appendChild(el("span", "", text));
  }

  // Everything here is a cheap read. The audit and the collection survey are not.
  async function refresh() {
    if (inFlight) return;
    inFlight = true;
    $("refreshBtn").disabled = true;
    try {
      const st = await getJson("/api/status");
      clearBanner();

      setText("mOps", st.ops);
      setText("mApplied", st.applied);
      setText("mPending", st.pending);
      setText("head", st.head || "(none)");
      setText("stateRoot", st.state_root || "(none)");

      const ratio = st.ops ? st.applied / st.ops : 1;
      setText("applyRatio", `${st.applied} / ${st.ops}`);
      $("applyBar").style.width = `${Math.round(ratio * 100)}%`;
      setText("bPending", st.pending);

      setHealth(st.pending ? `${st.pending} pending` : "healthy", st.pending ? "warn" : "ok");

      const stats = await getJson("/api/storage/stats");
      setText("mBlobs", stats.blob_count);
      setText("mBytes", formatBytes(stats.blob_bytes));
      setText("bState", stats.state_entries);

      await renderIdentity();
      await renderEnergy();
      await loadListing();
      await renderOps();
      await renderClock();
      await renderPeers(st.now_ms);
      await renderEnrolled();
    } catch (e) {
      setHealth("unreachable", "bad");
      fail(e);
    } finally {
      inFlight = false;
      $("refreshBtn").disabled = false;
    }
  }

  async function renderEnergy() {
    const e = await getJson("/api/energy");
    setText("ePower", e.power);
    setText("eBattery", e.battery_pct == null ? "—" : `${e.battery_pct}%`);
    setText("eTemp", e.temp_c == null ? "—" : `${e.temp_c} °C`);
    setText("eLoad", e.cpu_load == null ? "—" : e.cpu_load.toFixed(2));
    setText("eLink", e.link);
    setText("eInterval", `${e.interval_scale}× configured`);

    let label;
    let tone;
    if (!e.sync) {
      label = "paused";
      tone = "bad";
    } else if (!e.content) {
      label = "operations only";
      tone = "warn";
    } else if (e.max_content_bytes != null) {
      label = `capped at ${formatBytes(e.max_content_bytes)}`;
      tone = "warn";
    } else {
      label = "unlimited";
      tone = "ok";
    }
    setPill("energyPill", label, e.enabled ? tone : "neutral");

    setText(
      "eReason",
      e.enabled ? e.reason : "Energy-aware scheduling is off; the reading is informational."
    );
  }

  function setAuto(on) {
    localStorage.setItem(AUTO_KEY, on ? "1" : "0");
    if (autoTimer) clearInterval(autoTimer);
    autoTimer = on ? setInterval(() => refresh(), AUTO_INTERVAL_MS) : null;
  }

  function init() {
    $("token").value = localStorage.getItem(TOKEN_KEY) || "";
    $("token").addEventListener("change", () => {
      localStorage.setItem(TOKEN_KEY, $("token").value.trim());
      refresh();
    });
    $("token").addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") $("token").dispatchEvent(new Event("change"));
    });

    $("refreshBtn").onclick = () => refresh();
    $("filter").addEventListener("input", paintListing);
    $("pendingOnly").addEventListener("change", paintOps);
    $("viewerClose").onclick = closeViewer;
    $("auditBtn").onclick = runAudit;
    $("gcBtn").onclick = runSurvey;

    for (const tab of document.querySelectorAll(".tab")) {
      tab.onclick = () => selectTab(tab.dataset.tab);
    }
    selectTab(localStorage.getItem(TAB_KEY) || "overview");

    for (const button of document.querySelectorAll("[data-copy]")) {
      button.onclick = () => copyText($(button.dataset.copy).textContent, button);
    }

    const auto = localStorage.getItem(AUTO_KEY) === "1";
    $("auto").checked = auto;
    $("auto").addEventListener("change", () => setAuto($("auto").checked));
    setAuto(auto);

    // r refreshes, 1-5 jump to a tab — but never while the operator is typing.
    document.addEventListener("keydown", (ev) => {
      if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
      if (document.activeElement && document.activeElement.tagName === "INPUT") return;
      if (ev.key === "r") refresh();
      const tabs = [...document.querySelectorAll(".tab")];
      const index = Number(ev.key) - 1;
      if (index >= 0 && index < tabs.length) selectTab(tabs[index].dataset.tab);
    });

    renderCommands();
    refresh();
  }

  window.NX = { refresh, navigate, audit: runAudit, survey: runSurvey };
  init();
})();
