// Kept deliberately blunt about what is not built. A roadmap that reads as though
// everything is nearly done is worse than no roadmap.
const milestoneData = [
  {
    label: "M0",
    title: "Workspace and daemon",
    status: "done",
    body: "Multi-crate Rust workspace, runnable binary, embedded admin surface, specifications."
  },
  {
    label: "M1",
    title: "Local filesystem core",
    status: "done",
    body: "Signed operations applied to CRDT namespace state, deterministic conflict naming, path resolution and file reads, snapshots committing to structure and content."
  },
  {
    label: "M2",
    title: "External facade",
    status: "done",
    body: "An S3-compatible API — objects, buckets and ListObjectsV2 — routed through the same signed-operation pipeline the CLI uses. POSIX/FUSE remains unimplemented."
  },
  {
    label: "M3",
    title: "Verified replication",
    status: "done",
    body: "Two nodes converge over QUIC. Operations transfer before content, signatures and chunk hashes are verified before anything is accepted, and peer keys are pinned on first use."
  },
  {
    label: "M4",
    title: "Encryption and proofs",
    status: "done",
    body: "Chunk content encrypted at rest with per-file keys, still addressed by ciphertext hash so peers verify transfers without holding a key. Operations carry signed transparent proofs; nexusfs verify audits the repository."
  },
  {
    label: "M5",
    title: "Energy-aware scheduling",
    status: "done",
    body: "Replication reads the device's power source, charge, temperature and link cost, then decides how much it may transfer. Under constraint it keeps taking operations and defers content, so a low-battery node stays current on what exists without spending the bytes."
  },
  {
    label: "M6",
    title: "Operational hardening",
    status: "done",
    body: "Reclaiming unreachable storage, an enforced on-disk format version with a migration path, and enrolling peer keys ahead of first contact so trust-on-first-use is optional rather than the only route."
  },
  {
    label: "M7",
    title: "State commitments",
    status: "done",
    body: "The state root is a Merkle tree, so any one entry can be proved to someone holding no filesystem at all — a root, an inode, its hash and a handful of siblings. A commitment scheme rather than zero-knowledge: the verifier learns the entry being proved, just not the rest of the tree."
  },
  {
    label: "M8",
    title: "Research expansion",
    status: "todo",
    body: "Stronger privacy layers, proof batching, delay-tolerant replication and a real proving system — the last of which needs a circuit-friendly hash and a setup ceremony, so it stays behind its own milestone. Not started."
  }
];

const researchData = [
  "Deterministic offline-first replication",
  "Proof-carrying storage operations",
  "Energy-aware sync policy",
  "Edge-friendly content addressing",
  "Transparent-to-ZK migration path",
  "Private metadata strategies"
];

function renderMilestones() {
  const container = document.getElementById("milestone-grid");
  if (!container) {
    return;
  }

  milestoneData.forEach((item, index) => {
    const article = document.createElement("article");
    article.className = "track-card reveal";
    article.dataset.status = item.status;
    article.style.transitionDelay = `${index * 60}ms`;
    article.innerHTML = `
      <span class="track-head">
        <span class="track-label">${item.label}</span>
        <span class="track-status">${item.status === "done" ? "Complete" : "Not started"}</span>
      </span>
      <h3>${item.title}</h3>
      <p>${item.body}</p>
    `;
    container.appendChild(article);
  });
}

function renderResearch() {
  const container = document.getElementById("research-grid");
  if (!container) {
    return;
  }

  researchData.forEach((item, index) => {
    const div = document.createElement("div");
    div.className = "research-chip reveal";
    div.style.transitionDelay = `${index * 50}ms`;
    div.textContent = item;
    container.appendChild(div);
  });
}

function setYear() {
  const year = document.getElementById("year");
  if (year) {
    year.textContent = new Date().getFullYear();
  }
}

function enableReveal() {
  const items = Array.from(document.querySelectorAll(".reveal"));
  if (!items.length) {
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add("is-visible");
          observer.unobserve(entry.target);
        }
      });
    },
    {
      threshold: 0.14
    }
  );

  items.forEach((item) => observer.observe(item));
}

renderMilestones();
renderResearch();
setYear();
enableReveal();
