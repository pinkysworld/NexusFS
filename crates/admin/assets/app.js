
(function () {
  function tokenHeader() {
    const t = document.getElementById("token").value || "";
    return t ? { "x-nexusfs-token": t } : {};
  }

  async function getJson(path) {
    const res = await fetch(path, { headers: tokenHeader() });
    if (!res.ok) {
      const txt = await res.text();
      throw new Error(`HTTP ${res.status}: ${txt}`);
    }
    return await res.json();
  }

  async function refresh() {
    try {
      const st = await getJson("/api/status");
      document.getElementById("device").textContent = st.device_id;
      document.getElementById("head").textContent = st.head || "(none)";
      document.getElementById("now").textContent = st.now_ms;

      const sum = await getJson("/api/oplog/summary");
      document.getElementById("oplog").textContent = JSON.stringify(sum, null, 2);
    } catch (e) {
      document.getElementById("oplog").textContent = String(e);
    }
  }

  window.NX = { refresh };
  refresh();
})();
