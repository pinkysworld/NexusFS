(function () {
  let currentPath = "/";

  function tokenHeader() {
    const t = document.getElementById("token").value || "";
    return t ? { "x-nexusfs-token": t } : {};
  }

  async function getJson(path) {
    const res = await fetch(path, { headers: tokenHeader() });
    if (!res.ok) {
      throw new Error(`HTTP ${res.status}: ${await res.text()}`);
    }
    return await res.json();
  }

  function setText(id, value) {
    document.getElementById(id).textContent = value;
  }

  function cell(row, text, className) {
    const td = row.insertCell();
    td.textContent = text;
    if (className) td.className = className;
    return td;
  }

  function formatBytes(n) {
    if (n < 1024) return `${n} B`;
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let value = n / 1024;
    let i = 0;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i++;
    }
    return `${value.toFixed(1)} ${units[i]}`;
  }

  function renderBreadcrumbs() {
    const container = document.getElementById("breadcrumbs");
    container.textContent = "";

    const parts = currentPath.split("/").filter(Boolean);
    const root = document.createElement("span");
    root.textContent = "/";
    root.className = "crumb";
    root.onclick = () => navigate("/");
    container.appendChild(root);

    parts.forEach((part, i) => {
      const span = document.createElement("span");
      span.textContent = part;
      span.className = "crumb";
      span.onclick = () => navigate("/" + parts.slice(0, i + 1).join("/"));
      container.appendChild(span);
      if (i < parts.length - 1) container.appendChild(document.createTextNode(" / "));
    });
  }

  async function renderListing() {
    const body = document.getElementById("listing");
    body.textContent = "";
    renderBreadcrumbs();

    const data = await getJson(`/api/fs/ls?path=${encodeURIComponent(currentPath)}`);
    if (data.entries.length === 0) {
      const row = body.insertRow();
      const td = cell(row, "(empty)", "dim");
      td.colSpan = 4;
      return;
    }

    for (const entry of data.entries) {
      const row = body.insertRow();
      if (entry.kind === "dir") {
        const td = row.insertCell();
        const link = document.createElement("span");
        link.textContent = entry.name;
        link.className = "crumb";
        const child = currentPath === "/" ? `/${entry.name}` : `${currentPath}/${entry.name}`;
        link.onclick = () => navigate(child);
        td.appendChild(link);
      } else {
        cell(row, entry.name);
      }
      cell(row, entry.kind, "dim");
      cell(row, entry.kind === "file" ? formatBytes(entry.size) : "", "num");
      cell(row, entry.inode.slice(0, 12) + "…", "dim");
    }
  }

  async function renderOps() {
    const body = document.getElementById("ops");
    body.textContent = "";
    const ops = await getJson("/api/oplog/recent?limit=15");
    for (const op of ops) {
      const row = body.insertRow();
      cell(row, op.device_id.slice(0, 8) + "…", "dim");
      cell(row, op.counter, "num");
      cell(row, op.kind);
      cell(row, op.applied ? "applied" : "pending", op.applied ? "dim" : "err");
    }
  }

  async function renderPeers() {
    const body = document.getElementById("peers");
    body.textContent = "";
    const data = await getJson("/api/peers");

    if (!data.enabled) {
      document.getElementById("peersNote").textContent =
        "Replication is not running in this build.";
      return;
    }
    if (!data.peers.length) {
      document.getElementById("peersNote").textContent =
        "No peer contacted yet.";
      return;
    }
    document.getElementById("peersNote").textContent = "";

    for (const p of data.peers) {
      const row = body.insertRow();
      cell(row, p.address);
      cell(row, p.device_id ? p.device_id.slice(0, 8) + "\u2026" : "\u2014", "dim");
      cell(row, p.ops_received, "num");
      cell(row, formatBytes(p.content_bytes || 0), "num");
      if (p.last_error) {
        cell(row, p.last_error, "err");
      } else if (p.content_deferred) {
        // Worth distinguishing from "up to date": the namespace is current but the
        // budget stopped short of fetching every byte.
        cell(row, "synced, content deferred");
      } else if (p.last_success_ms) {
        cell(row, "synced", "dim");
      } else {
        cell(row, "no contact yet", "dim");
      }
    }
  }

  async function renderEnergy() {
    const e = await getJson("/api/energy");
    setText("power", e.power);
    setText("battery", e.battery_pct == null ? "\u2014" : `${e.battery_pct}%`);
    setText("temp", e.temp_c == null ? "\u2014" : `${e.temp_c} \u00b0C`);
    setText("link", e.link);

    let budget;
    if (!e.sync) {
      budget = "paused";
    } else if (!e.content) {
      budget = "operations only";
    } else if (e.max_content_bytes != null) {
      budget = `capped at ${formatBytes(e.max_content_bytes)}`;
    } else {
      budget = "unlimited";
    }
    setText("budget", budget);

    const note = document.getElementById("energyReason");
    note.textContent = e.enabled
      ? e.reason
      : "Energy-aware scheduling is off; the reading is informational.";
  }

  // Both of the following read the whole repository, so neither belongs on the refresh
  // path — a console that quietly re-audits every few seconds is a console nobody
  // leaves open.
  async function renderIntegrity() {
    const v = await getJson("/api/security");
    setText("vOps", v.ops_total);
    setText("vSigs", v.signature_failures);
    setText("vMalformed", v.malformed_proofs);
    setText("vUnreadable", v.unreadable_files.length);
    setText("vEncryption", v.encryption_at_rest ? "on" : "off");

    const verdict = document.getElementById("verdict");
    verdict.textContent = v.healthy
      ? "repository verified"
      : "verification found problems";
    verdict.className = v.healthy ? "muted" : "err";
  }

  async function renderGc() {
    const g = await getJson("/api/storage/gc");
    setText("gcScanned", g.blobs_scanned);
    setText("gcReachable", g.reachable);
    setText("gcUnreachable", g.unreachable);
    setText("gcBytes", formatBytes(g.bytes_reclaimable));
  }

  function navigate(path) {
    currentPath = path;
    renderListing().catch(showError);
  }

  function showError(err) {
    document.getElementById("error").textContent = String(err);
  }

  async function refresh() {
    document.getElementById("error").textContent = "";
    try {
      const st = await getJson("/api/status");
      setText("device", st.device_id);
      setText("head", st.head || "(none)");
      setText("stateRoot", st.state_root || "(none)");

      const stats = await getJson("/api/storage/stats");
      setText("blobCount", stats.blob_count);
      setText("blobBytes", formatBytes(stats.blob_bytes));
      setText("stateEntries", stats.state_entries);
      setText("opCount", stats.op_count);
      setText("appliedCount", stats.applied_count);
      setText("pendingCount", stats.pending_count);

      setText("oplog", JSON.stringify(await getJson("/api/oplog/summary"), null, 2));

      await renderListing();
      await renderOps();
      await renderPeers();
      await renderEnergy();
    } catch (e) {
      showError(e);
    }
  }

  window.NX = {
    refresh,
    navigate,
    audit: () => renderIntegrity().catch(showError),
    survey: () => renderGc().catch(showError),
  };
  refresh();
})();
