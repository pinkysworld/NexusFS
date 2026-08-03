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
    status: "todo",
    body: "A practical first interface, either POSIX/FUSE or an S3-like API surface, routed through the same operation pipeline."
  },
  {
    label: "M3",
    title: "Verified replication",
    status: "todo",
    body: "Peer manager, oplog synchronization, blob transfer and verified remote apply. The transport exists; the protocol above it does not."
  },
  {
    label: "M4",
    title: "Encryption and proofs",
    status: "todo",
    body: "At-rest chunk encryption, key envelopes, and transparent proof bundles generated and enforced on operations."
  },
  {
    label: "M5–M8",
    title: "Scheduling, hardening, ZK",
    status: "todo",
    body: "Energy-aware background work, operational tooling, then commitment-oriented proof systems and privacy research."
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
