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
    } catch (e) {
      showError(e);
    }
  }

  window.NX = { refresh, navigate };
  refresh();
})();
