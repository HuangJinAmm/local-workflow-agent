# Turso Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the project's direct `rusqlite` usage with the embedded-local `turso` crate while preserving the current goal/session store behavior.

**Architecture:** Keep `GoalStore` and `SqliteSessionStore` as the public entry points, but change their internals to store a `turso::Database` and open async connections per operation. Migrate the smaller `GoalStore` path first, then move `SqliteSessionStore`, remove `rusqlite`, and finish with targeted regression tests plus docs updates.

**Tech Stack:** Rust 2024 · `tokio` · `turso` 0.5.x · `anyhow` · existing unit/integration test harness

**Spec:** `docs/superpowers/specs/2026-06-28-turso-migration-design.md`

---

## File map (locked in by this plan)

```text
Cargo.toml                                                   # modify: add `turso`, later remove `rusqlite`
src/core/goal.rs                                             # modify: migrate GoalStore to async Turso
src/tools/goal_complete.rs                                   # modify: await GoalStore open/status update
src/core/sqlite_storage.rs                                   # modify: migrate SqliteSessionStore to async Turso
tests/sqlite_storage_turso.rs                                # create: idempotency + list/query regression tests
README.md                                                    # modify: storage engine wording
docs/superpowers/specs/2026-06-28-turso-migration-design.md  # reference only
```

---

## Phase 1 — GoalStore first

### Task 1: Add `turso` and migrate `GoalStore`

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/core/goal.rs`
- Modify: `src/tools/goal_complete.rs`

- [ ] **Step 1: Add `turso` without removing `rusqlite` yet**

Edit `Cargo.toml` so the database section temporarily contains both crates:

```toml
# Utilities / storage
urlencoding = "2"
rusqlite = { version = "0.31", features = ["bundled"] }
turso = "0.5.1"
```

Reason: keep the tree compiling while `GoalStore` is migrated before
`SqliteSessionStore`.

- [ ] **Step 2: Change `GoalStore` to own a `turso::Database`**

Replace the store definition and opening path in `src/core/goal.rs`:

```rust
use turso::Builder;

pub struct GoalStore {
    db: turso::Database,
}

impl GoalStore {
    pub async fn open(db_path: &std::path::Path) -> Result<Self, GoalError> {
        let path = db_path.to_string_lossy().to_string();
        let db = Builder::new_local(path)
            .build()
            .await
            .map_err(|e| GoalError::Db(e.to_string()))?;

        let conn = db.connect().map_err(|e| GoalError::Db(e.to_string()))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS goals (
                id              TEXT PRIMARY KEY,
                session_id      TEXT NOT NULL,
                objective       TEXT NOT NULL,
                status          TEXT NOT NULL DEFAULT 'active',
                token_budget    INTEGER,
                tokens_used     INTEGER NOT NULL DEFAULT 0,
                time_used_secs  INTEGER NOT NULL DEFAULT 0,
                turns_used      INTEGER NOT NULL DEFAULT 0,
                created_at_ms   INTEGER NOT NULL,
                updated_at_ms   INTEGER NOT NULL
            )",
            (),
        )
        .await
        .map_err(|e| GoalError::Db(e.to_string()))?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_goals_session ON goals(session_id)",
            (),
        )
        .await
        .map_err(|e| GoalError::Db(e.to_string()))?;

        Ok(Self { db })
    }
}
```

- [ ] **Step 3: Convert all GoalStore methods and tests to async**

Update method signatures and the test helper:

```rust
impl GoalStore {
    pub async fn open_default() -> Option<Self> {
        let path = Self::default_path()?;
        Self::open(&path).await.ok()
    }

    pub async fn set_status(
        &self,
        session_id: &str,
        status: GoalStatus,
    ) -> Result<(), GoalError> {
        let now = Self::now_ms();
        let conn = self.db.connect().map_err(|e| GoalError::Db(e.to_string()))?;
        conn.execute(
            "UPDATE goals SET status = ?1, updated_at_ms = ?2 WHERE session_id = ?3",
            (status.as_str(), now as i64, session_id),
        )
        .await
        .map_err(|e| GoalError::Db(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    async fn open_tmp() -> GoalStore {
        GoalStore::open(Path::new(":memory:")).await.unwrap()
    }

    #[tokio::test]
    async fn test_set_and_get_goal() {
        let store = open_tmp().await;
        let goal = store.set_goal("sess1", "fix all the bugs", None).await.unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
    }
}
```

Also update the remaining tests in this module from `#[test]` to
`#[tokio::test]` and insert `.await` on store calls.

- [ ] **Step 4: Make `GoalCompleteTool` await the store**

Edit `src/tools/goal_complete.rs`:

```rust
match crate::core::GoalStore::open_default().await {
    None => ToolResult::error("Could not open goal store.".to_string()),
    Some(store) => match store
        .set_status(session_id, crate::core::GoalStatus::Complete)
        .await
    {
        Ok(()) => ToolResult::success(format!(
            "Goal marked complete.\n\nAudit summary: {}\n\nEvidence: {}",
            params.audit_summary, params.evidence,
        )),
        Err(e) => ToolResult::error(format!(
            "Failed to mark goal complete: {}. There may be no active goal for this session.",
            e
        )),
    },
}
```

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test test_set_and_get_goal --lib
cargo test test_status_transitions --lib
cargo check
```

Expected:

- both focused tests pass
- `cargo check` still succeeds because `rusqlite` remains for
  `SqliteSessionStore`

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/core/goal.rs src/tools/goal_complete.rs
git commit -m "feat: migrate goal store to embedded turso"
```

---

## Phase 2 — Session store migration

### Task 2: Migrate `SqliteSessionStore` and add regression tests

**Files:**
- Modify: `src/core/sqlite_storage.rs`
- Create: `tests/sqlite_storage_turso.rs`

- [ ] **Step 1: Change the store to `turso::Database`**

Replace the struct and open path in `src/core/sqlite_storage.rs`:

```rust
use std::path::Path;
use turso::Builder;

pub struct SqliteSessionStore {
    db: turso::Database,
}

impl SqliteSessionStore {
    pub async fn open(db_path: &Path) -> anyhow::Result<Self> {
        let db = Builder::new_local(db_path.to_string_lossy().to_string())
            .build()
            .await?;
        let conn = db.connect()?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT,
                model TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                message_count INTEGER NOT NULL DEFAULT 0
            )",
            (),
        )
        .await?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                cost_usd REAL
            )",
            (),
        )
        .await?;

        Ok(Self { db })
    }
}
```

- [ ] **Step 2: Convert all session-store methods to async and preserve behavior**

Use a fresh connection per operation and explicit row conversion helpers:

```rust
impl SqliteSessionStore {
    fn row_string(row: &turso::Row, idx: usize) -> anyhow::Result<String> {
        Ok(row.get_value(idx)?.as_text().cloned().unwrap_or_default())
    }

    pub async fn save_session(
        &self,
        session_id: &str,
        title: Option<&str>,
        model: &str,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.db.connect()?;
        conn.execute(
            "INSERT INTO sessions (id, title, model, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 title = excluded.title,
                 model = excluded.model,
                 updated_at = excluded.updated_at",
            (session_id, title, model, now.as_str()),
        )
        .await?;
        Ok(())
    }
}
```

Apply the same async pattern to:

- `save_message`
- `list_sessions`
- `search_sessions`
- `delete_session`
- `list_messages`

Keep ordering and idempotency semantics unchanged. Wrap the `save_message`
insert plus counter bump in one transaction.

- [ ] **Step 3: Add focused integration tests**

Create `tests/sqlite_storage_turso.rs`:

```rust
use local_workflow_agent::core::SqliteSessionStore;
use tempfile::tempdir;

#[tokio::test]
async fn save_message_is_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("sessions.sqlite");
    let store = SqliteSessionStore::open(&db_path).await.unwrap();

    store.save_session("s1", Some("Test"), "gpt-4").await.unwrap();
    store
        .save_message("s1", "m1", "user", "hello", Some(0.1))
        .await
        .unwrap();
    store
        .save_message("s1", "m1", "user", "hello", Some(0.1))
        .await
        .unwrap();

    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].message_count, 1);
}

#[tokio::test]
async fn list_messages_returns_oldest_first() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("sessions.sqlite");
    let store = SqliteSessionStore::open(&db_path).await.unwrap();

    store.save_session("s1", Some("Test"), "gpt-4").await.unwrap();
    store.save_message("s1", "m1", "user", "first", None).await.unwrap();
    store.save_message("s1", "m2", "assistant", "second", None).await.unwrap();

    let messages = store.list_messages("s1").await.unwrap();
    assert_eq!(messages[0].id, "m1");
    assert_eq!(messages[1].id, "m2");
}
```

- [ ] **Step 4: Verify the session path before removing `rusqlite`**

Run:

```bash
cargo test --test sqlite_storage_turso
cargo check
```

Expected:

- both new tests pass
- the crate still compiles while both database dependencies are present

- [ ] **Step 5: Commit**

```bash
git add src/core/sqlite_storage.rs tests/sqlite_storage_turso.rs
git commit -m "feat: migrate session store to embedded turso"
```

---

## Phase 3 — Remove `rusqlite` and finish docs

### Task 3: Remove the old dependency and update docs

**Files:**
- Modify: `Cargo.toml`
- Modify: `README.md`

- [ ] **Step 1: Remove `rusqlite` from Cargo.toml**

Edit the dependency list so it becomes:

```toml
urlencoding = "2"
turso = "0.5.1"
```

Then run lockfile resolution with Cargo:

```bash
cargo update
```

Expected: `Cargo.lock` no longer contains a direct dependency edge from this
crate to `rusqlite`.

- [ ] **Step 2: Update README wording**

Change the storage wording in `README.md` from implementation-specific SQLite
phrasing to Turso-embedded phrasing while keeping the local file semantics.

Use wording like:

```md
Session history lives in `<data-dir>/sessions.sqlite`; attachments are written
under `<data-dir>/attachments/`. The local database is backed by the embedded
`turso` crate.
```

And update the architecture note to:

```md
- **`SqliteSessionStore`** persists sessions and per-session message history
  using the embedded `turso` database engine. The GUI's `load_session` reads
  from this store; submitting a turn currently keeps message bodies in memory
  only (persisting the bodies is tracked as follow-up).
```

- [ ] **Step 3: Run final verification**

Run:

```bash
cargo check
cargo test test_set_and_get_goal --lib
cargo test --test sqlite_storage_turso
```

Expected:

- all commands succeed
- no direct `rusqlite` usage remains in project code

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock README.md
git commit -m "chore: remove rusqlite after turso migration"
```

---

## Phase 4 — Cleanup and handoff

### Task 4: Final sweep for leftover sync assumptions

**Files:**
- Modify: any compile-error sites discovered by `cargo check`

- [ ] **Step 1: Search for leftover `rusqlite` references**

Run:

```bash
rg "rusqlite|Connection::open\\(|query_row\\(" src Cargo.toml README.md
```

Expected:

- no `rusqlite` references in source or docs
- any remaining `query_row` usage is unrelated or intentionally retained

- [ ] **Step 2: Check diagnostics in edited Rust files**

Check diagnostics for:

- `src/core/goal.rs`
- `src/core/sqlite_storage.rs`
- `src/tools/goal_complete.rs`

Expected: no new Rust errors after the async migration.

- [ ] **Step 3: Create the final handoff note**

Record the migration outcome in the PR / final handoff using this summary:

```text
Replaced direct rusqlite usage with embedded Turso in GoalStore and
SqliteSessionStore, converted store APIs to async, updated GoalCompleteTool,
added regression tests for session-store idempotency/order, and updated README
to describe Turso as the local embedded database engine.
```

- [ ] **Step 4: Commit any final compile-fix edits**

```bash
git add -A
git commit -m "test: finalize turso migration verification"
```

---

## Self-review checklist

- Spec coverage:
  - dependency swap: Tasks 1 and 3
  - `GoalStore` async migration: Task 1
  - `SqliteSessionStore` async migration: Task 2
  - `GoalCompleteTool` caller update: Task 1
  - behavior preservation and transactions: Tasks 1 and 2
  - focused testing: Tasks 1, 2, and 3
  - docs update: Task 3
- Placeholder scan:
  - no `TODO`, `TBD`, or "similar to above" markers remain
- Type consistency:
  - both stores are planned around `turso::Database`
  - all migrated public methods use `async`
  - `GoalCompleteTool` awaits `GoalStore::open_default()` and `set_status()`
