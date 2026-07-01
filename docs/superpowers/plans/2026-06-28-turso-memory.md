# Turso Long-Term Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Turso-backed long-term memory system that stores extracted memories, retrieves relevant memories for new tasks, and injects them into the dynamic query prompt without breaking the existing agent flow.

**Architecture:** The implementation adds a focused storage layer in `src/core/` and a small orchestration layer in `src/query/`. `MemoryStore` owns Turso schema and queries, `EmbeddingProvider` abstracts vector generation, and `MemoryRuntime` bridges retrieval/writeback into the existing query loop and session memory extractor.

**Tech Stack:** Rust 2024 edition, `tokio`, `turso` 0.6.x, existing query loop and session memory extractor

---

## File map

- Create: `src/core/memory_types.rs`
- Create: `src/core/memory_store.rs`
- Create: `src/query/memory_runtime.rs`
- Create: `tests/memory_store_turso.rs`
- Create: `tests/query_memory_runtime.rs`
- Modify: `src/core/mod.rs`
- Modify: `src/query/mod.rs`
- Modify: `src/query/session_memory.rs`

Notes for implementers:

- Follow the project's existing Turso pattern from `src/core/sqlite_storage.rs`.
- Keep all database APIs async.
- Treat memory as best-effort: retrieval/writeback failures must not break the main query path.
- Reuse `MemoryCategory` from `src/query/session_memory.rs` instead of inventing a second enum.

---

### Task 1: Add Core Memory Types And Exports

**Files:**
- Create: `src/core/memory_types.rs`
- Modify: `src/core/mod.rs`
- Test: `tests/memory_store_turso.rs`

- [ ] **Step 1: Write the failing type/export test**

Add this test skeleton to `tests/memory_store_turso.rs`:

```rust
use local_workflow_agent::core::memory_types::{
    MemoryCandidate, MemorySearchResult, StoredMemory, TaskRecord,
};
use local_workflow_agent::query::MemoryCategory;

#[test]
fn memory_types_are_constructible() {
    let memory = StoredMemory {
        id: "mem-1".to_string(),
        content: "User prefers staged confirmation.".to_string(),
        category: MemoryCategory::UserPreference,
        created_at: 1,
        last_retrieved: None,
        retrieval_count: 0,
        source_task: Some("task-1".to_string()),
    };

    let result = MemorySearchResult {
        memory,
        distance: 0.1,
        similarity: 0.9,
    };

    let task = TaskRecord {
        id: "task-1".to_string(),
        description: "Implement memory feature".to_string(),
        started_at: 1,
        finished_at: None,
    };

    let candidate = MemoryCandidate {
        content: "Project uses Turso locally.".to_string(),
        category: MemoryCategory::ProjectFact,
        source_task: Some(task.id.clone()),
    };

    assert_eq!(candidate.category, MemoryCategory::ProjectFact);
    assert!(result.similarity > result.distance);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test memory_store_turso memory_types_are_constructible -- --nocapture
```

Expected: FAIL with missing `memory_types` module or unresolved imports.

- [ ] **Step 3: Write minimal implementation**

Create `src/core/memory_types.rs` with:

```rust
use crate::query::MemoryCategory;

#[derive(Debug, Clone)]
pub struct StoredMemory {
    pub id: String,
    pub content: String,
    pub category: MemoryCategory,
    pub created_at: i64,
    pub last_retrieved: Option<i64>,
    pub retrieval_count: u32,
    pub source_task: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub memory: StoredMemory,
    pub distance: f64,
    pub similarity: f64,
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: String,
    pub description: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MemoryCandidate {
    pub content: String,
    pub category: MemoryCategory,
    pub source_task: Option<String>,
}
```

Update `src/core/mod.rs` exports near the storage modules:

```rust
pub mod memory_types;
pub use memory_types::{MemoryCandidate, MemorySearchResult, StoredMemory, TaskRecord};
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test memory_store_turso memory_types_are_constructible -- --nocapture
```

Expected: PASS with 1 test passed.

- [ ] **Step 5: Commit**

```bash
git add src/core/memory_types.rs src/core/mod.rs tests/memory_store_turso.rs
git commit -m "feat: add long-term memory core types"
```

---

### Task 2: Implement `MemoryStore` Schema And CRUD/Search

**Files:**
- Create: `src/core/memory_store.rs`
- Modify: `src/core/mod.rs`
- Test: `tests/memory_store_turso.rs`

- [ ] **Step 1: Write the failing storage tests**

Extend `tests/memory_store_turso.rs` with:

```rust
use local_workflow_agent::core::{MemoryCandidate, MemoryStore};
use local_workflow_agent::query::MemoryCategory;

#[tokio::test]
async fn memory_store_can_round_trip_task_and_memory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.unwrap();

    let embedding = vec![0.25_f32; 384];
    store.start_task("task-1", "test task", &embedding).await.unwrap();

    let memory_id = store
        .store_memory(
            &MemoryCandidate {
                content: "User prefers staged confirmation.".to_string(),
                category: MemoryCategory::UserPreference,
                source_task: Some("task-1".to_string()),
            },
            &embedding,
        )
        .await
        .unwrap();

    let results = store.search_memories(&embedding, 3).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, memory_id);

    store.finish_task("task-1").await.unwrap();
}

#[tokio::test]
async fn memory_store_purges_unretrieved_old_memories() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.unwrap();
    let embedding = vec![0.5_f32; 384];

    let _ = store
        .store_memory(
            &MemoryCandidate {
                content: "Old unused memory".to_string(),
                category: MemoryCategory::ProjectFact,
                source_task: None,
            },
            &embedding,
        )
        .await
        .unwrap();

    let purged = store.purge_stale_memories(i64::MAX).await.unwrap();
    assert_eq!(purged, 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test memory_store_turso -- --nocapture
```

Expected: FAIL because `MemoryStore` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

Create `src/core/memory_store.rs` based on the Turso patterns already used in
`src/core/sqlite_storage.rs`:

```rust
use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::Utc;
use turso::{Builder, Database, Row, Value};
use uuid::Uuid;

use crate::core::{MemoryCandidate, MemorySearchResult, StoredMemory, TaskRecord};
use crate::query::MemoryCategory;

pub struct MemoryStore {
    db: Database,
}

impl MemoryStore {
    pub async fn open(db_path: &Path) -> Result<Self> {
        let db = Builder::new_local(&db_path.to_string_lossy()).build().await?;
        let conn = db.connect()?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                embedding F8_BLOB(384),
                category TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_retrieved INTEGER,
                retrieval_count INTEGER NOT NULL DEFAULT 0,
                source_task TEXT
            )",
            (),
        ).await?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                description TEXT,
                embedding F8_BLOB(384),
                started_at INTEGER,
                finished_at INTEGER
            )",
            (),
        ).await?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS memory_task_uses (
                task_id TEXT NOT NULL,
                memory_id TEXT NOT NULL,
                similarity REAL NOT NULL,
                used_at INTEGER NOT NULL,
                credit REAL,
                PRIMARY KEY (task_id, memory_id)
            )",
            (),
        ).await?;

        Ok(Self { db })
    }

    pub async fn start_task(&self, task_id: &str, description: &str, embedding: &[f32]) -> Result<()> {
        self.ensure_embedding_dims(embedding)?;
        let conn = self.db.connect()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT OR REPLACE INTO tasks (id, description, embedding, started_at, finished_at)
             VALUES (?1, ?2, vector8(?3), ?4, NULL)",
            turso::params![task_id, description, serde_json::to_string(embedding)?, now],
        ).await?;
        Ok(())
    }

    pub async fn finish_task(&self, task_id: &str) -> Result<()> {
        let conn = self.db.connect()?;
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE tasks SET finished_at = ?1 WHERE id = ?2",
            turso::params![now, task_id],
        ).await?;
        Ok(())
    }

    pub async fn store_memory(&self, candidate: &MemoryCandidate, embedding: &[f32]) -> Result<String> {
        self.ensure_embedding_dims(embedding)?;
        let conn = self.db.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO memories (id, content, embedding, category, created_at, source_task)
             VALUES (?1, ?2, vector8(?3), ?4, ?5, ?6)",
            turso::params![
                id.as_str(),
                candidate.content.as_str(),
                serde_json::to_string(embedding)?,
                candidate.category.label(),
                now,
                candidate.source_task.as_deref()
            ],
        ).await?;
        Ok(id)
    }

    pub async fn search_memories(&self, embedding: &[f32], top_k: usize) -> Result<Vec<MemorySearchResult>> {
        self.ensure_embedding_dims(embedding)?;
        let conn = self.db.connect()?;
        let mut rows = conn.query(
            "SELECT id, content, category, created_at, last_retrieved, retrieval_count, source_task,
                    vector_distance_cos(embedding, vector8(?1)) AS distance
             FROM memories
             WHERE embedding IS NOT NULL
             ORDER BY distance ASC
             LIMIT ?2",
            turso::params![serde_json::to_string(embedding)?, top_k as i64],
        ).await?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let distance = match row.get_value(7)? {
                Value::Real(v) => v,
                Value::Float(v) => v as f64,
                other => return Err(anyhow!("unexpected distance value: {other:?}")),
            };

            out.push(MemorySearchResult {
                memory: self.read_stored_memory(&row)?,
                distance,
                similarity: 1.0 - distance,
            });
        }

        Ok(out)
    }
}
```

Add the export in `src/core/mod.rs`:

```rust
pub mod memory_store;
pub use memory_store::MemoryStore;
```

Finish the file with helper methods for row conversion, `record_retrieval`,
`delete_memory`, `replace_memory`, `purge_stale_memories`, and
`ensure_embedding_dims(&[f32]) -> Result<()>`.

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test memory_store_turso -- --nocapture
```

Expected: PASS with the new store tests succeeding.

- [ ] **Step 5: Commit**

```bash
git add src/core/memory_store.rs src/core/mod.rs tests/memory_store_turso.rs
git commit -m "feat: add turso long-term memory store"
```

---

### Task 3: Add Query Memory Runtime And Prompt Formatting

**Files:**
- Create: `src/query/memory_runtime.rs`
- Modify: `src/query/mod.rs`
- Test: `tests/query_memory_runtime.rs`

- [ ] **Step 1: Write the failing runtime test**

Create `tests/query_memory_runtime.rs` with:

```rust
use local_workflow_agent::core::{MemoryCandidate, MemoryStore};
use local_workflow_agent::query::{MemoryCategory, MemoryRuntime};

struct FixedEmbeddingProvider;

#[async_trait::async_trait]
impl MemoryRuntimeEmbeddingProvider for FixedEmbeddingProvider {
    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.2_f32; 384])
    }
}

#[tokio::test]
async fn runtime_formats_retrieved_memories_for_prompt_append() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.unwrap();
    let embedding = vec![0.2_f32; 384];

    let _ = store
        .store_memory(
            &MemoryCandidate {
                content: "User prefers staged confirmation before major edits.".to_string(),
                category: MemoryCategory::UserPreference,
                source_task: None,
            },
            &embedding,
        )
        .await
        .unwrap();

    let runtime = MemoryRuntime::new(store, std::sync::Arc::new(FixedEmbeddingProvider));
    let ctx = runtime
        .begin_task_context("Implement memory feature", 3, Some(0.5))
        .await
        .unwrap();

    let rendered = runtime.format_prompt_memories(&ctx);
    assert!(rendered.contains("Relevant long-term memories"));
    assert!(rendered.contains("staged confirmation"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test query_memory_runtime runtime_formats_retrieved_memories_for_prompt_append -- --nocapture
```

Expected: FAIL because `MemoryRuntime` and its embedding trait do not exist.

- [ ] **Step 3: Write minimal implementation**

Create `src/query/memory_runtime.rs`:

```rust
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::core::{MemoryCandidate, MemorySearchResult, MemoryStore};

#[async_trait]
pub trait MemoryRuntimeEmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

#[derive(Debug, Clone)]
pub struct RetrievedMemoryContext {
    pub task_id: String,
    pub description: String,
    pub memories: Vec<MemorySearchResult>,
}

pub struct MemoryRuntime {
    store: MemoryStore,
    embedding_provider: Arc<dyn MemoryRuntimeEmbeddingProvider>,
}

impl MemoryRuntime {
    pub fn new(store: MemoryStore, embedding_provider: Arc<dyn MemoryRuntimeEmbeddingProvider>) -> Self {
        Self { store, embedding_provider }
    }

    pub async fn begin_task_context(
        &self,
        description: &str,
        top_k: usize,
        max_distance: Option<f64>,
    ) -> Result<RetrievedMemoryContext> {
        let task_id = Uuid::new_v4().to_string();
        let embedding = self.embedding_provider.embed(description).await?;
        self.store.start_task(&task_id, description, &embedding).await?;

        let mut memories = self.store.search_memories(&embedding, top_k).await?;
        if let Some(limit) = max_distance {
            memories.retain(|m| m.distance <= limit);
        }

        for item in &memories {
            self.store
                .record_retrieval(&task_id, &item.memory.id, item.similarity)
                .await?;
        }

        Ok(RetrievedMemoryContext {
            task_id,
            description: description.to_string(),
            memories,
        })
    }

    pub fn format_prompt_memories(&self, ctx: &RetrievedMemoryContext) -> String {
        if ctx.memories.is_empty() {
            return String::new();
        }

        let mut out = String::from("Relevant long-term memories:\n");
        for item in &ctx.memories {
            out.push_str(&format!(
                "- [{}] {}\n",
                item.memory.category.label(),
                item.memory.content
            ));
        }
        out
    }

    pub async fn writeback_extracted_memories(
        &self,
        task_id: &str,
        candidates: &[MemoryCandidate],
    ) -> Result<()> {
        for candidate in candidates {
            let embedding = self.embedding_provider.embed(&candidate.content).await?;
            self.store.store_memory(candidate, &embedding).await?;
        }
        self.store.finish_task(task_id).await?;
        Ok(())
    }
}
```

Update `src/query/mod.rs` exports near `session_memory`:

```rust
pub mod memory_runtime;
pub use memory_runtime::{MemoryRuntime, MemoryRuntimeEmbeddingProvider, RetrievedMemoryContext};
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test query_memory_runtime -- --nocapture
```

Expected: PASS with the prompt-formatting test succeeding.

- [ ] **Step 5: Commit**

```bash
git add src/query/memory_runtime.rs src/query/mod.rs tests/query_memory_runtime.rs
git commit -m "feat: add query memory runtime"
```

---

### Task 4: Integrate Retrieval Into The Query Loop

**Files:**
- Modify: `src/query/mod.rs`
- Test: `tests/query_memory_runtime.rs`

- [ ] **Step 1: Write the failing integration test**

Extend `tests/query_memory_runtime.rs` with:

```rust
use local_workflow_agent::query::QueryConfig;

#[test]
fn query_config_can_hold_memory_append_text() {
    let mut cfg = QueryConfig::default();
    cfg.append_system_prompt = Some("existing".to_string());

    let appended = match &cfg.append_system_prompt {
        Some(value) => format!("{}\n\n{}", value, "Relevant long-term memories:\n- [project_fact] Uses Turso"),
        None => unreachable!(),
    };

    assert!(appended.contains("Relevant long-term memories"));
    assert!(appended.contains("existing"));
}
```

Then add a second async test that exercises the helper function you introduce in
`src/query/mod.rs`:

```rust
#[test]
fn combine_append_system_prompt_preserves_existing_text() {
    let existing = Some("todo nudge".to_string());
    let memory = "Relevant long-term memories:\n- [constraint] Keep store APIs async";
    let combined = local_workflow_agent::query::combine_append_system_prompt(existing, memory);
    assert!(combined.contains("todo nudge"));
    assert!(combined.contains("Keep store APIs async"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test query_memory_runtime combine_append_system_prompt_preserves_existing_text -- --nocapture
```

Expected: FAIL because the helper does not exist yet.

- [ ] **Step 3: Write minimal implementation**

In `src/query/mod.rs`, add a small helper near `build_todo_nudge`:

```rust
pub fn combine_append_system_prompt(existing: Option<String>, extra: &str) -> String {
    match existing {
        Some(current) if !current.trim().is_empty() => format!("{}\n\n{}", current, extra),
        _ => extra.to_string(),
    }
}
```

Then add the query-loop integration where `patched.append_system_prompt` is
assembled:

```rust
let retrieved_memory_text = if let Some(runtime) = &patched.memory_runtime {
    let description = messages
        .iter()
        .rev()
        .find(|m| m.role == crate::core::types::Role::User)
        .map(|m| m.get_all_text())
        .unwrap_or_default();

    match runtime.begin_task_context(&description, 5, Some(0.6)).await {
        Ok(ctx) => {
            patched.active_memory_task_id = Some(ctx.task_id.clone());
            runtime.format_prompt_memories(&ctx)
        }
        Err(err) => {
            tracing::debug!(error = %err, "long-term memory retrieval skipped");
            String::new()
        }
    }
} else {
    String::new()
};

if !retrieved_memory_text.is_empty() {
    patched.append_system_prompt = Some(combine_append_system_prompt(
        patched.append_system_prompt.take(),
        &retrieved_memory_text,
    ));
}
```

Also extend `QueryConfig` with:

```rust
pub memory_runtime: Option<std::sync::Arc<crate::query::MemoryRuntime>>,
pub active_memory_task_id: Option<String>,
```

And initialize both to `None` in `Default`.

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test query_memory_runtime -- --nocapture
```

Expected: PASS with the append helper tests succeeding.

- [ ] **Step 5: Commit**

```bash
git add src/query/mod.rs tests/query_memory_runtime.rs
git commit -m "feat: inject long-term memory into query prompt"
```

---

### Task 5: Wire Session-End Extraction Into Memory Writeback

**Files:**
- Modify: `src/query/session_memory.rs`
- Modify: `src/query/mod.rs`
- Test: `tests/query_memory_runtime.rs`

- [ ] **Step 1: Write the failing writeback test**

Extend `tests/query_memory_runtime.rs` with:

```rust
use local_workflow_agent::query::ExtractedMemory;

#[tokio::test]
async fn extracted_memories_convert_to_writeback_candidates() {
    let extracted = vec![
        ExtractedMemory {
            content: "Project uses async Turso storage.".to_string(),
            category: MemoryCategory::ProjectFact,
            confidence: 0.9,
        },
        ExtractedMemory {
            content: "Project uses async Turso storage.".to_string(),
            category: MemoryCategory::ProjectFact,
            confidence: 0.8,
        },
    ];

    let candidates = local_workflow_agent::query::session_memory::to_memory_candidates(&extracted);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].content, "Project uses async Turso storage.");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test --test query_memory_runtime extracted_memories_convert_to_writeback_candidates -- --nocapture
```

Expected: FAIL because the conversion helper does not exist.

- [ ] **Step 3: Write minimal implementation**

In `src/query/session_memory.rs`, add:

```rust
use std::collections::HashSet;

use crate::core::MemoryCandidate;

pub fn to_memory_candidates(memories: &[ExtractedMemory]) -> Vec<MemoryCandidate> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out = Vec::new();

    for memory in memories {
        let normalized = memory.content.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.is_empty() {
            continue;
        }

        let key = (memory.category.label().to_string(), normalized.clone());
        if seen.insert(key) {
            out.push(MemoryCandidate {
                content: normalized,
                category: memory.category.clone(),
                source_task: None,
            });
        }
    }

    out
}
```

Then update the `end_turn` branch in `src/query/mod.rs` so the detached session
memory task uses `MemoryRuntime::writeback_extracted_memories(...)` when a
runtime and active task id are present, while keeping `AGENTS.md` persistence
untouched for v1:

```rust
let memory_runtime = config.memory_runtime.clone();
let active_task_id = config.active_memory_task_id.clone();

tokio::spawn(async move {
    let extractor = session_memory::SessionMemoryExtractor::new(&model_clone);
    match extractor.extract(&messages_clone, &working_dir_clone, &sm_client).await {
        Ok(memories) if !memories.is_empty() => {
            let candidates = session_memory::to_memory_candidates(&memories);

            if let (Some(runtime), Some(task_id)) = (memory_runtime.as_ref(), active_task_id.as_deref()) {
                if let Err(err) = runtime.writeback_extracted_memories(task_id, &candidates).await {
                    tracing::debug!(error = %err, "long-term memory writeback failed");
                }
            }

            let target = working_dir_clone.join(".claurst").join("AGENTS.md");
            if let Err(err) = session_memory::SessionMemoryExtractor::persist(&memories, &target).await {
                tracing::warn!(error = %err, "Failed to persist session memories");
            }
        }
        Ok(_) => {}
        Err(err) => tracing::debug!(error = %err, "Session memory extraction failed (non-fatal)"),
    }
});
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test --test query_memory_runtime extracted_memories_convert_to_writeback_candidates -- --nocapture
```

Expected: PASS with duplicate extracted memories collapsed into one candidate.

- [ ] **Step 5: Commit**

```bash
git add src/query/session_memory.rs src/query/mod.rs tests/query_memory_runtime.rs
git commit -m "feat: write extracted memories into turso store"
```

---

### Task 6: Add Failure-Path Coverage And Full Regression Run

**Files:**
- Modify: `tests/memory_store_turso.rs`
- Modify: `tests/query_memory_runtime.rs`

- [ ] **Step 1: Write the failing degradation tests**

Add these tests:

```rust
#[tokio::test]
async fn memory_store_rejects_wrong_embedding_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.unwrap();

    let err = store.start_task("task-1", "bad dims", &[0.1_f32; 12]).await.unwrap_err();
    assert!(err.to_string().contains("384"));
}
```

And:

```rust
struct FailingEmbeddingProvider;

#[async_trait::async_trait]
impl MemoryRuntimeEmbeddingProvider for FailingEmbeddingProvider {
    async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
        Err(anyhow::anyhow!("embedding unavailable"))
    }
}

#[tokio::test]
async fn runtime_begin_task_context_surfaces_embedding_failure() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("memory.db");
    let store = MemoryStore::open(&db_path).await.unwrap();
    let runtime = MemoryRuntime::new(store, std::sync::Arc::new(FailingEmbeddingProvider));

    let err = runtime.begin_task_context("test", 5, Some(0.5)).await.unwrap_err();
    assert!(err.to_string().contains("embedding unavailable"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test --test memory_store_turso --test query_memory_runtime -- --nocapture
```

Expected: FAIL until validation and degradation behavior are fully implemented.

- [ ] **Step 3: Write minimal implementation**

Ensure `src/core/memory_store.rs` contains explicit dimension validation:

```rust
fn ensure_embedding_dims(&self, embedding: &[f32]) -> Result<()> {
    if embedding.len() != 384 {
        return Err(anyhow!("expected 384 embedding dimensions, got {}", embedding.len()));
    }
    Ok(())
}
```

Ensure `src/query/mod.rs` keeps retrieval/writeback failures non-fatal by
wrapping runtime calls:

```rust
match runtime.begin_task_context(&description, 5, Some(0.6)).await {
    Ok(ctx) => runtime.format_prompt_memories(&ctx),
    Err(err) => {
        tracing::debug!(error = %err, "long-term memory retrieval skipped");
        String::new()
    }
}
```

And:

```rust
if let Err(err) = runtime.writeback_extracted_memories(task_id, &candidates).await {
    tracing::debug!(error = %err, "long-term memory writeback failed");
}
```

- [ ] **Step 4: Run the full targeted regression suite**

Run:

```bash
cargo test --test memory_store_turso --test query_memory_runtime -- --nocapture
cargo test sqlite_storage_turso -- --nocapture
```

Expected:

- all new memory tests PASS
- existing Turso session-storage regression tests still PASS

- [ ] **Step 5: Commit**

```bash
git add tests/memory_store_turso.rs tests/query_memory_runtime.rs src/core/memory_store.rs src/query/mod.rs
git commit -m "test: cover long-term memory failure paths"
```

---

## Self-review checklist

- Spec coverage:
  - schema and storage: Tasks 1-2
  - runtime retrieval and prompt injection: Tasks 3-4
  - session-end writeback: Task 5
  - failure handling and regression coverage: Task 6
- Placeholder scan:
  - no `TODO`, `TBD`, or deferred code markers remain in the tasks
- Type consistency:
  - `MemoryStore`, `MemoryCandidate`, `MemoryRuntime`, and `MemoryRuntimeEmbeddingProvider`
    use the same names across all tasks
  - `MemoryCategory` is reused from `session_memory.rs` throughout the plan

