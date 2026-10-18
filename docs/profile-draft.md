<!-- Public profile facts preserved from Alaneel/Alaneel at dfa8bca0731c8876bc40d1bc73e8738e4d51f4d8. -->

# Alan Wang

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/Alaneel/video-toolkit/master/assets/portfolio/profile-dark.svg">
  <img alt="AI and systems engineering — typed tools, durable state, inspectable evidence" src="https://raw.githubusercontent.com/Alaneel/video-toolkit/master/assets/portfolio/profile-light.svg" width="1200">
</picture>

**AI / Agentic Systems Engineering · Backend / Distributed Infrastructure**

I build around the point where a model's answer becomes a system's action: what it can observe, what it may change, and how we verify the result.

Computer Science graduate from **Nanyang Technological University**, based in Singapore. My experience spans **RAG and LLM applications at Binance**, **backend services supporting ML workflows at ByteDance**, and **Redis / KV infrastructure at Xiaomi**.

[LinkedIn](https://www.linkedin.com/in/yangming-wang/) · [Email](mailto:agooseelan@gmail.com)

## What connects my work

| AI / Agentic systems | Backend / Distributed infrastructure |
| --- | --- |
| Typed tools and explicit authority boundaries | Idempotency, fencing and durable state |
| Evidence-linked proposals and human approval | Crash recovery and failure injection |
| Evaluation that keeps failures visible | Inspectable invariants and reproducible experiments |

## Selected systems

### [Video Agent · reviewable video rough cuts](https://github.com/Alaneel/video-toolkit)

`Rust` · `egui` · `FFmpeg` · `OpenAI Responses`

A native desktop agent that turns timestamped visual observations into a reviewable clip plan. Typed tools, exact-revision approval and recoverable FFmpeg stages connect model proposals to real media execution—without giving the model authority to approve or export.

**Experimental source preview.** A real GPT-6 Luna draft reached a verified, playable 20-second export after the user authorized removal of its first clip; the development agent applied the edit and exact-revision approval in the native UI. Formal human semantic evaluation remains pending; public Agent binaries remain gated. [v2.1.0](https://github.com/Alaneel/video-toolkit/releases/tag/v2.1.0) is the existing Toolkit only.

[20-second output](https://raw.githubusercontent.com/Alaneel/video-toolkit/master/assets/video-agent/luna-human-edited-20s.mp4) · [Interface & media credits](https://github.com/Alaneel/video-toolkit#video-agent) · [Evidence & limits](https://github.com/Alaneel/video-toolkit/blob/master/docs/video-agent-evidence.json) · [Evaluation status](https://github.com/Alaneel/video-toolkit/blob/master/evals/video-agent/results/status.json)

### [OpsPilot · AI Production Incident Copilot](https://github.com/Alaneel/ai-production-incident-copilot)

`LangGraph` · `MCP` · `BM25` · `Signed approvals`

An incident copilot with a compiled stateful workflow, MCP HTTP tools, a BM25 knowledge index, signed approvals and persistent audit records. The PostgreSQL path uses durable checkpoints; the in-memory fallback is separate.

**Source-only preview.** An actual-model smoke run with synthetic telemetry and an in-memory graph stopped at human approval. It does not establish production RCA quality; the current Kubernetes matrix and production readiness remain unverified.

[Offline demo](https://github.com/Alaneel/ai-production-incident-copilot/blob/main/docs/demo.md) · [Evidence & limits](https://github.com/Alaneel/ai-production-incident-copilot/blob/main/docs/evidence.md) · [Publication manifest](https://github.com/Alaneel/ai-production-incident-copilot/blob/main/PUBLICATION-MANIFEST.json)

### Transaction Processing Platform

`Java` · `PostgreSQL` · `Kafka`

A ledger with exact integer balances, durable idempotency, a fenced transactional outbox and idempotent consumer effects. Tests cover concurrency, crashes and outages.

**Implemented locally; public release preparation is in progress.** Public source and evidence links are not yet available. Local benchmarks do not establish production capacity or exactly-once messaging.

### [Redis Reliability Lab](https://github.com/Alaneel/redis-reliability-lab)

`Go` · `Redis Cluster` · `Docker`

A fault laboratory that separates service recovery, restored redundancy and acknowledged-write survival. A deterministic, read-only diagnostic extension cites recorded observations, preserves uncertainty and proposes remediation plans.

**Public lab, not a production reliability claim.** The diagnosis engine has no LLM integration. Results come from a small, single-host setup and do not establish production RCA accuracy or power-loss durability.

[Fault experiments](https://github.com/Alaneel/redis-reliability-lab/blob/main/docs/running-experiments.md) · [Case studies](https://github.com/Alaneel/redis-reliability-lab/blob/main/docs/aiops-case-studies.md) · [Evaluation & limits](https://github.com/Alaneel/redis-reliability-lab/blob/main/docs/aiops-evaluation.md) · [Tests & evidence](https://github.com/Alaneel/redis-reliability-lab/blob/main/docs/phase4-validation.md)

## Experience

- **Binance** — RAG / LLM applications with Dify and Lucene.
- **ByteDance** — backend services supporting ML workflows.
- **Xiaomi** — Redis / KV infrastructure.

---

I like systems with inspectable state, reproducible failures, and evidence that survives a closer look.
