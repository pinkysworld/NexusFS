# Evaluation Plan

This plan describes how to measure NexusFS along the axes that matter:
performance, energy, privacy overhead, and verifiability costs.

---

## 1) Micro-benchmarks

### 1.1 CAS throughput
- write throughput (MiB/s) for varying chunk sizes
- read throughput for sequential and random access
- dedup ratio impact

### 1.2 Metadata / oplog
- ops/sec for create/rename/unlink
- apply latency under conflict

### 1.3 Replication
- time-to-converge under:
  - perfect link
  - 5% packet loss
  - partition for N minutes then heal
- oplog-only sync time vs full blob sync

### 1.4 Proof costs
Per op type:
- proof generation time (ms)
- proof verification time (ms)
- proof size (bytes)

---

## 2) Energy profiling

### 2.1 Method
- On Linux SBCs: read battery/thermal sensors where available
- External power meter (optional) for ground truth
- Collect:
  - cpu utilization
  - network bytes sent/received
  - temperature
  - battery discharge rate

### 2.2 Experiments
- replication enabled vs disabled
- cover traffic enabled vs disabled
- compression on/off
- scheduler thresholds sweeps

---

## 3) Privacy metrics

### 3.1 Size leakage
- compare real size vs padded size distribution
- bucket entropy and overhead

### 3.2 Access-pattern leakage (cover traffic)
- simulate adversary observing timing/volume
- compute distinguishability of access events

---

## 4) Device matrix

Minimum:
- Raspberry Pi (4/5)
- Jetson Nano (or similar)
- Android phone (later)
- ESP32 (metadata-only variant)

---

## 5) End-to-end demos (realistic workloads)

- drone data capture: burst writes, delayed sync on return-to-base
- industrial sensor logs: append-heavy, periodic sync
- smart city camera: large objects, prioritized replication of "hot" intervals
