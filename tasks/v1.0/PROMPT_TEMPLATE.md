# Prompt Template — Executing a Concerto V1.0 Task

Hand this prompt to a fresh coding agent to execute one task from `tasks/v1.0/`. The agent gets everything it needs from the task file plus the design docs it references — no chat-history context required. This is the V1.0 variant of the root `tasks/PROMPT_TEMPLATE.md`; it adds task types and verification tiers.

---

## How to use

1. Pick the next unstarted task file (lowest-numbered `tasks/v1.0/NNN-<slug>.md` whose work isn't yet committed).
2. Copy the template below, replacing `{TASK_PATH}` with the relative path (e.g. `tasks/v1.0/106-agent-host-binary-resolution.md`).
3. Paste it as the first message to the agent.
4. When the agent reports done, **review the Handoff Notes it added** to the task file.

---

## The template (copy from the `---` markers)

---

You are executing **one** task from the Concerto **V1.0** build. Concerto is a local-first orchestration platform for AI coding agents; V0.1 shipped (a working macOS Core + Desktop), and V1.0 adds remote/multi-device access, the Maestro chat agent, monorepo + multi-repo support, mobile and web clients, and platform ports. The design is in `design/`; the V1.0 build is decomposed into phased tasks in `tasks/v1.0/`.

**Your task file:** `{TASK_PATH}`

**Your job:** read the task file, read every input it points at, implement exactly what's in `Scope — in`, run every command in `Verification` to its declared **tier**, tick every box in `Definition of Done`, fill in `Handoff Notes`, and create the single commit specified at the bottom of the task file. Nothing more.

### Mandatory reading order (before writing any code)

1. **`tasks/v1.0/README.md`** — the V1.0 meta-document. The locked decisions (§4), the **three-tier verification model (§5)**, the per-type verification command sets (§5.3), and the phase inventory (§6) constrain everything you do.
2. **Your task file** at `{TASK_PATH}` — top to bottom. Note its **`Task type`** and **`Verification tier`** fields.
3. Every entry under your task's **`Inputs to read before starting`**, including the **prior task's `Handoff Notes`** (load-bearing — that's where drift you must know about is recorded).
4. Skim `docs/interfaces/proto.md`, `schema.md`, `rust-api.md` for the existing Rust/proto surface. For `web-ts`/`rn-mobile` tasks, skim the shared client package and the relevant `apps/*` structure.

### Rules of engagement

- **Honor your tier (`tasks/v1.0/README.md` §5).** Tier 1 = fully CI-self-verifiable. Tier 2 = self-verifiable against a named test double (loopback Iroh / mock PushBackend / simulator / headless browser) — your `Verification` already names it. Tier 3 work that *can't* be machine-proven is captured as a phase-checklist line, NOT faked green. **Never downgrade your own tier to pass.** If a Tier-1 task only passes by mocking something it shouldn't, stop and ask.
- **Run the verification commands for your `Task type`** (§5.3) — `cargo …` for `rust`, `pnpm -C apps/web …` for `web-ts`, `pnpm -C apps/mobile …` + `expo prebuild` for `rn-mobile`, the stated gate for `infra-ops`. Do not skip and do not weaken a check (`-D warnings`, `--no-verify`, removing a test).
- **Spike tasks** produce a harness under `spikes/<name>/` and a findings doc at `design/spikes/<name>-findings.md` ending in an explicit **GO / NO-GO** with the measured numbers against the task's numeric bar. A NO-GO is a legitimate, valuable outcome — report it; do not force a GO.
- **Stay inside `Scope — in`.** Anything in `Scope — out` is someone else's task. Smells outside scope → note in Handoff under *Open questions*, don't fix.
- **Treat `Public interface this task locks` as a contract** — proto field numbers, SQL columns, Rust trait signatures, TS exported types are immutable after commit until an explicit revision task.
- **Modify only files in `Outputs`.** An unexpected file needing change is a signal your understanding drifted — add it to `Outputs` first.
- **No `TODO`/`FIXME`/`unimplemented!()`/`todo!()`** in new code unless the task explicitly defers it AND you record it under *Deliberate debt* with the closing task number.
- **One commit per task.** Exact message from the task file. Don't amend, don't split, don't push, don't open a PR — the operator handles git remote ops.
- **Don't modify prior tasks' files, the root `tasks/` (V0.1 history), or `design/`** — except a `doc`-type task whose `Outputs` explicitly lists a `design/` file (e.g. the embedded-mode retrofit). Drift is recorded in Handoff Notes.

### When you finish

1. Tick every `Definition of Done` box. If any is false, you're not done.
2. Fill in `Handoff Notes` in the same file — all four bullets (use `—` for "nothing"): **Drift from plan**, **Open questions for next task**, **Deliberate debt**, **Smoke-gate state**. For Tier-2 tasks, also state in *Open questions* what the test double did NOT cover (so the operator adds it to the phase checklist).
3. Stage `Outputs` + this task file, create the single commit with the task's message.
4. Report back: commit SHA, tier/type, smoke-gate state, and anything in Handoff Notes the operator should see before the next task.

### When to stop and ask

- An `Inputs` file/section doesn't exist or doesn't match what you find.
- A verification requires credentials/hardware/external services you don't have (real GitHub auth, real Expo push, a real second device, a signing key) — **for a Tier-2 task, build to the test-double bar and flag the Tier-3 remainder; for anything else, stop**.
- Two parts of the task file or two design sections contradict each other.
- A prior task's Handoff flags drift that invalidates this task's `Public interface this task locks`.
- A `Scope — in` item is impossible without changing something in `Scope — out`, or without downgrading your tier.
- A spike's measurement is a NO-GO against its bar.

In all cases: write a one-paragraph summary of what you found and what you'd recommend; do not code around the ambiguity.

---
