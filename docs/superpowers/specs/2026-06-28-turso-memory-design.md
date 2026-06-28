# Turso Long-Term Memory — Design Spec

**Date:** 2026-06-28
**Status:** Draft (awaiting user review)
**Target stack:** Rust 2024 edition · `turso` 0.5.x · `tokio`

---

## 1. Goal & scope

Add a Turso-backed long-term memory module to `local-workflow-agent` following
the design pattern from Turso's AI memory guide. The module should let the
agent:

- store durable memories with vector embeddings
- retrieve relevant memories for a new task via semantic search
- track task start and finish lifecycle
- feed retrieved memories into the dynamic system-prompt context
- persist high-value memories extracted from completed sessions

This spec covers:

- database schema for long-term memory
- Rust storage and runtime integration points
- retrieval and writeback data flow
- failure handling, deduplication, and cleanup strategy
- tests for the new memory path

This spec does **not** include:

- Turso Cloud sync or remote replication
- a user-facing memory management UI
- reinforcement-learning style weighting in v1
- cross-project shared memory
- replacing the existing `AGENTS.md` memory path in the same change

---

## 2. Current state

The repository already has two adjacent capabilities that matter for this work:

- Turso-based local persistence through modules such as
  `src/core/sqlite_storage.rs`
- post-session fact extraction in `src/query/session_memory.rs`

The current `session_memory` flow extracts useful facts from longer sessions and
appends them under `## Auto-extracted memories` in `AGENTS.md`. That gives the
project a lightweight persistent memory mechanism, but it is file-based and not
semantically searchable.

The query loop already has a natural dynamic-context injection point:

- `src/query/mod.rs` builds the system prompt before each model call
- dynamic additions are passed through `append_system_prompt`
- this section already sits after the system-prompt dynamic boundary

This means long-term memory can be added without redesigning prompt assembly.

---

## 3. Options considered

### 3.1 Option A: storage-only memory store

- Add a standalone `MemoryStore`
- Support schema creation, memory insert, task insert, and similarity query
- Do not connect it to the query loop yet

Pros:

- smallest implementation surface
- easy to validate storage behavior first

Cons:

- does not actually improve agent behavior yet
- requires a second follow-up integration change

### 3.2 Option B: replace `AGENTS.md` memory directly

- Rework `SessionMemoryExtractor` so extracted memories go straight into Turso
- Keep focus on session-end writeback
- Defer task tracking and retrieval injection

Pros:

- leverages the current extraction path
- low noise because only extracted facts are persisted

Cons:

- retrieval is incomplete
- misses the main value of Turso's AI memory pattern

### 3.3 Option C: complete long-term memory runtime

- Add a Turso-backed memory store
- Track tasks
- Retrieve relevant memories at task start
- Inject retrieved memories into the dynamic prompt
- Reuse session extraction as the writeback source

Pros:

- closest match to Turso's AI memory guide
- produces immediate end-user value
- keeps extraction and retrieval in one coherent flow

Cons:

- larger change surface than a storage-only MVP

### 3.4 Decision

Use **Option C**, but keep the first implementation intentionally narrow:

- implement the storage and retrieval path fully
- use existing session extraction as the only automatic write source
- defer weighting, decay, and interactive memory editing

This provides real long-term semantic memory without expanding into a broad
memory-management project.

---

## 4. Architecture changes

### 4.1 New modules

Add the following modules:

- `src/core/memory_types.rs`
- `src/core/memory_store.rs`
- `src/query/memory_runtime.rs`

Responsibilities:

- `memory_types.rs`: shared structs and enums for stored memory, search result,
  task record, and write candidates
- `memory_store.rs`: Turso schema initialization and all database I/O
- `memory_runtime.rs`: orchestration for task start retrieval, prompt
  formatting, and session-end writeback

### 4.2 Boundary rules

The long-term memory design uses strict boundaries:

- `MemoryStore` owns SQL, schema, and data conversion only
- `EmbeddingProvider` owns text-to-vector conversion only
- `MemoryRuntime` decides when retrieval and writeback happen
- `SessionMemoryExtractor` remains responsible for extracting candidate facts
  from conversations

This keeps the database layer independent from model-provider details.

### 4.3 Embedding abstraction

The storage layer must not directly call Anthropic, OpenAI, or any other
provider SDK. Instead, define a small trait such as:

```rust
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}
```

The v1 contract assumes 384-dimensional embeddings because the Turso guide uses
`F8_BLOB(384)`. The runtime must validate the returned dimension before writing
or querying.

---

## 5. Schema design

### 5.1 `memories`

The primary long-term memory table:

```sql
CREATE TABLE IF NOT EXISTS memories (
    id               TEXT PRIMARY KEY,
    content          TEXT NOT NULL,
    embedding        F8_BLOB(384),
    category         TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    last_retrieved   INTEGER,
    retrieval_count  INTEGER NOT NULL DEFAULT 0,
    source_task      TEXT
);
```

Category values in v1:

- `user_preference`
- `project_fact`
- `code_pattern`
- `decision`
- `constraint`

These categories intentionally align with `src/query/session_memory.rs` so the
existing extraction flow can feed the new store without inventing a second
classification system.

### 5.2 `tasks`

Task lifecycle storage:

```sql
CREATE TABLE IF NOT EXISTS tasks (
    id           TEXT PRIMARY KEY,
    description  TEXT,
    embedding    F8_BLOB(384),
    started_at   INTEGER,
    finished_at  INTEGER
);
```

This matches the Turso guide and gives the agent a durable record of what it
was working on when memories were retrieved or created.

### 5.3 `memory_task_uses`

An additional attribution table for v1:

```sql
CREATE TABLE IF NOT EXISTS memory_task_uses (
    task_id      TEXT NOT NULL,
    memory_id    TEXT NOT NULL,
    similarity   REAL NOT NULL,
    used_at      INTEGER NOT NULL,
    credit       REAL,
    PRIMARY KEY (task_id, memory_id)
);
```

This table is not strictly required by Turso's minimal schema, but it is the
lowest-cost way to preserve future room for weighting, pruning, and usefulness
feedback without redesigning the database later.

### 5.4 Schema policy

Use `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` during
startup. Recommended indexes:

- `idx_memories_created_at` on `memories(created_at)`
- `idx_memories_category` on `memories(category)`
- `idx_tasks_started_at` on `tasks(started_at)`

No vector index is required in v1. The expected per-project dataset is small
enough that `ORDER BY distance LIMIT k` is sufficient.

---

## 6. Rust API design

### 6.1 Core types

Add Rust types similar to:

```rust
pub struct StoredMemory {
    pub id: String,
    pub content: String,
    pub category: MemoryCategory,
    pub created_at: i64,
    pub last_retrieved: Option<i64>,
    pub retrieval_count: u32,
    pub source_task: Option<String>,
}

pub struct MemorySearchResult {
    pub memory: StoredMemory,
    pub distance: f64,
    pub similarity: f64,
}

pub struct TaskRecord {
    pub id: String,
    pub description: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

pub struct MemoryCandidate {
    pub content: String,
    pub category: MemoryCategory,
    pub source_task: Option<String>,
}
```

These types separate storage shape, retrieval result, and write candidate
shape so callers do not need to overload a single struct for all operations.

### 6.2 `MemoryStore`

`MemoryStore` should expose async methods:

- `open(db_path: &Path) -> Result<Self>`
- `start_task(task_id: &str, description: &str, embedding: &[f32]) -> Result<()>`
- `finish_task(task_id: &str) -> Result<()>`
- `store_memory(candidate: &MemoryCandidate, embedding: &[f32]) -> Result<String>`
- `search_memories(embedding: &[f32], top_k: usize) -> Result<Vec<MemorySearchResult>>`
- `record_retrieval(task_id: &str, memory_id: &str, similarity: f64) -> Result<()>`
- `delete_memory(memory_id: &str) -> Result<()>`
- `replace_memory(old_id: &str, candidate: &MemoryCandidate, embedding: &[f32]) -> Result<String>`
- `purge_stale_memories(older_than_ms: i64) -> Result<u64>`

All methods remain non-blocking and follow the project's current convention that
Turso-backed store APIs are async.

### 6.3 `MemoryRuntime`

`MemoryRuntime` is a coordination layer with methods such as:

- `begin_task_context(...) -> Result<RetrievedMemoryContext>`
- `writeback_extracted_memories(...) -> Result<()>`
- `format_prompt_memories(...) -> String`

`MemoryRuntime` does not own the prompt builder or the query loop. It only
prepares data so the existing query code can inject it.

---

## 7. Retrieval and writeback flow

### 7.1 Task-start retrieval

When a new query begins:

1. derive a task description from the newest user message, working directory,
   and any active goal context
2. call `EmbeddingProvider::embed`
3. write a `tasks` row with `started_at`
4. query `memories` using `vector_distance_cos(embedding, vector8(?))`
5. keep the top `k` results that pass a distance threshold
6. update `last_retrieved` and `retrieval_count`
7. record `(task_id, memory_id, similarity)` in `memory_task_uses`
8. format the retained results into a short prompt appendix

The prompt appendix should be injected through `append_system_prompt`, not by
rewriting the base prompt assembly.

### 7.2 Suggested prompt shape

Use a compact, bounded format such as:

```text
Relevant long-term memories:
- [user_preference] The user prefers staged confirmation before major edits.
- [project_fact] This project uses Turso for local embedded persistence.
- [constraint] Database store APIs must remain async and be awaited.
```

This keeps the added context legible, auditable, and easy to cap.

### 7.3 Session-end writeback

When the query loop reaches `end_turn`, reuse the existing extraction trigger in
`src/query/session_memory.rs`:

1. run `SessionMemoryExtractor`
2. convert `ExtractedMemory` into `MemoryCandidate`
3. normalize and deduplicate candidates
4. embed each candidate
5. store accepted memories in `memories`
6. mark the task as finished

The v1 system writes only extracted high-value memories, not every user or
assistant utterance. This is deliberate: it keeps the store compact and avoids
high-noise long-term memory.

---

## 8. Query-loop integration

### 8.1 Integration point

The best insertion point is in `src/query/mod.rs`, before the request is built
and before `build_system_prompt(&patched)` is called.

At that point the query loop already has:

- the current message list
- the working directory
- the current config
- the dynamic `append_system_prompt` mechanism

This allows memory retrieval to be layered in without redesigning the rest of
the query flow.

### 8.2 Minimal code-path changes

The v1 integration should:

- add an optional `memory_runtime` handle to `QueryConfig` or adjacent runtime
  state
- fetch retrieved prompt memory once for the current task
- append the formatted memory block to `patched.append_system_prompt`
- keep all failures non-fatal

The system prompt builder itself does not need new memory-specific logic beyond
receiving appended text.

### 8.3 Compatibility with existing `AGENTS.md`

Keep the current `AGENTS.md` reading behavior unchanged in v1.

The new Turso-backed memory layer is additive:

- `AGENTS.md` remains a readable hierarchical memory source
- Turso long-term memory provides semantic retrieval

This avoids mixing a storage migration with a prompt-behavior removal.

---

## 9. Deduplication and cleanup policy

### 9.1 Write-time normalization

Before storing a memory:

- trim leading and trailing whitespace
- collapse repeated whitespace
- reject empty content
- reject obviously generic content that carries no future value

### 9.2 Deduplication rule

V1 uses a simple deterministic dedupe policy:

- normalize content
- dedupe on `(category, normalized_content)`

Do **not** attempt semantic deduplication in v1. That would add cost and
complexity without being necessary for the first release.

### 9.3 Cleanup rule

Add a purge operation that deletes memories where:

- `retrieval_count = 0`
- `created_at < cutoff`

This directly follows Turso's guide and is a safe initial cleanup strategy.

---

## 10. Failure handling

Long-term memory is an enhancement layer, not a hard dependency for the core
query experience.

### 10.1 Embedding failures

If embedding generation fails:

- skip retrieval or writeback for that attempt
- log a warning or debug event
- continue the main agent flow

### 10.2 Database failures

If `MemoryStore` fails to connect or execute:

- do not abort the query loop
- do not fail the user-facing turn
- preserve diagnostics in logs

### 10.3 Validation failures

If an embedding has the wrong length:

- reject the operation
- log the dimension mismatch explicitly
- do not write malformed rows

### 10.4 Retrieval quality guardrails

To prevent noisy context injection:

- cap retrieved memories to a small `top_k` such as 3-5
- cap per-memory rendered length
- drop matches above a distance threshold

This keeps long-term memory from becoming a prompt-bloat source.

---

## 11. Testing strategy

### 11.1 Storage tests

Add focused tests for:

- schema initialization
- memory insertion
- task start and finish updates
- retrieval ordering
- delete and replace behavior
- purge behavior

### 11.2 Query/runtime tests

Add tests for:

- task-start retrieval formatting
- append-system-prompt injection behavior
- session-end extracted-memory writeback

### 11.3 Degradation tests

Explicitly test that:

- embedding failures do not break the query loop
- Turso failures do not break the query loop
- empty or low-quality extraction output is ignored safely

### 11.4 Regression focus

Preserve current session extraction behavior as much as possible:

- same extraction thresholds
- same category mapping
- same non-fatal background processing model

The main regression risk is not SQL correctness but accidental prompt pollution
or over-eager writeback. Tests should therefore focus on relevance and bounded
context addition, not only database CRUD.

---

## 12. Implementation boundaries for v1

### In scope

- Turso memory schema and store
- async Rust APIs for memory and task lifecycle
- embedding abstraction trait
- retrieval injection through dynamic prompt append
- session-extracted memory writeback
- basic dedupe and purge support
- focused tests

### Out of scope

- memory weighting
- time decay in retrieval ranking
- user-facing correction commands
- credit learning from task success
- project-global or user-global shared memory

---

## 13. Recommended rollout

Implement in the following order:

1. add schema, types, and `MemoryStore`
2. add `EmbeddingProvider` and runtime validation
3. add task-start retrieval and prompt formatting
4. add session-end extracted-memory writeback
5. add purge and replace operations
6. add focused tests and regression coverage

This sequence keeps the retrieval path visible early while preserving a narrow
and debuggable rollout.

---

## 14. Success criteria

The feature is considered successful when:

- a new task can retrieve semantically relevant stored memories from Turso
- retrieved memories are injected into the dynamic prompt in a bounded format
- completed sessions can persist extracted high-value memories into Turso
- task lifecycle rows are written and closed correctly
- memory failures do not interrupt normal query execution
- the feature passes focused Turso and query integration tests

