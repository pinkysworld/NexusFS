const milestoneData = [
  {
    label: "M1",
    title: "Local filesystem core",
    body: "Canonical encoding, content-addressed chunks, persistent heads, and CRDT-backed state evolution."
  },
  {
    label: "M2",
    title: "External facade",
    body: "A practical first interface, either POSIX/FUSE or an S3-like API surface."
  },
  {
    label: "M3",
    title: "Verified replication",
    body: "QUIC transport, oplog synchronization, blob fetch, and trusted remote apply."
  },
  {
    label: "M4",
    title: "Encryption and proofs",
    body: "At-rest encryption, key envelopes, and transparent proof bundles on operations."
  },
  {
    label: "M5",
    title: "Energy-aware behavior",
    body: "Telemetry-informed scheduling that respects battery, heat, and expensive links."
  },
  {
    label: "M6+",
    title: "ZK expansion",
    body: "Commitment-oriented proof systems and deeper privacy and verification research."
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
    article.style.transitionDelay = `${index * 60}ms`;
    article.innerHTML = `
      <span>${item.label}</span>
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
