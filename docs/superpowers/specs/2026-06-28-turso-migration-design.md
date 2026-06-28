# Turso Migration — Design Spec

**Date:** 2026-06-28
**Status:** Draft (awaiting user review)
**Target stack:** Rust 2024 edition · `turso` 0.5.x · `tokio`

---

## 1. Goal & scope

Replace the project's direct SQLite dependency with the `turso` crate in
embedded local-database mode so the application continues to use local `.sqlite`
files, but the storage engine is no longer `rusqlite`.

The migration covers:

- `src/core/goal.rs` and the `GoalStore` API
- `src/core/sqlite_storage.rs` and the `SqliteSessionStore` API
- Direct call sites that invoke these stores
- Related tests and documentation

The migration does **not** include:

- Turso Cloud or remote sync
- A new storage abstraction layer
- Renaming `SqliteSessionStore` / `sqlite_storage.rs` in the same change
- Reworking the JSON-based session storage path in `src/core/mod.rs`

---

## 2. Current state

The repository currently uses SQLite in two direct Rust modules:

- `GoalStore` in `src/core/goal.rs`
- `SqliteSessionStore` in `src/core/sqlite_storage.rs`

Both modules hold a synchronous `rusqlite::Connection` and expose synchronous
database methods.

Important constraint discovered during exploration:

- `src/core/mod.rs` still uses JSON files as the main conversation-session
  persistence path
- `SqliteSessionStore` exists as a separate SQLite-backed store and is not the
  only session persistence implementation in the project

This spec therefore treats the migration as a direct engine replacement for the
existing SQLite-backed modules, not as a full persistence unification project.

---

## 3. Chosen approach

### 3.1 Options considered

**Option A: direct API swap**

- Replace `rusqlite::Connection` with `turso` usage in place
- Convert current store methods to `async`
- Keep the rest of the design unchanged

Pros:

- Fastest path to working code

Cons:

- Database-specific details stay tightly coupled to store implementations

**Option B: add storage traits first**

- Introduce interfaces for goals and sessions
- Implement Turso-backed concrete stores behind those interfaces

Pros:

- Cleanest long-term architecture

Cons:

- Expands scope beyond the requested migration
- Adds structural churn unrelated to replacing SQLite

**Option C: keep public type names, switch internals to Turso**

- Preserve `GoalStore` and `SqliteSessionStore` as the public entry points
- Replace internal storage implementation with `turso`
- Convert affected methods and call sites to `async`

Pros:

- Smallest reasonable migration surface
- Keeps current module boundaries intact
- Avoids introducing a new abstraction layer prematurely

Cons:

- `SqliteSessionStore` becomes a transitional name that no longer reflects the
  exact implementation

### 3.2 Decision

Use **Option C**.

This approach best matches the requested change: replace SQLite usage with the
`turso` crate while keeping the migration focused and low-risk.

---

## 4. Architecture changes

### 4.1 Dependency changes

`Cargo.toml` changes:

- Remove `rusqlite = { version = "...", features = ["bundled"] }`
- Add `turso = "0.5.1"` or the latest compatible `0.5.x`

No sync or cloud feature is enabled for this migration. The project uses Turso
only as an embedded local database.

### 4.2 Storage model

Each migrated store continues to open a local database file, for example:

- `:memory:` for tests
- `~/.claurst/goals.sqlite` for goals
- existing local session database paths where `SqliteSessionStore` is used

The implementation shifts from synchronous `rusqlite` calls to asynchronous
`turso` calls:

- connection opening becomes async
- schema initialization becomes async
- all query and mutation methods become async

### 4.3 Naming strategy

To minimize migration cost in this change:

- keep file names unchanged
- keep `GoalStore` unchanged
- keep `SqliteSessionStore` unchanged

If the migration is successful, a later cleanup may rename
`SqliteSessionStore` and `sqlite_storage.rs` to engine-neutral names.

---

## 5. API changes

### 5.1 GoalStore

`GoalStore` remains the public storage type but changes internally to use
`turso`.

Methods that become `async`:

- `open`
- `open_default`
- `set_goal`
- `get_goal`
- `get_active_goal`
- `set_status`
- `clear_goal`
- `record_turn`
- `add_tokens`

Error behavior remains aligned with the current model:

- keep `GoalError`
- continue to wrap database failures as `GoalError::Db(String)`

### 5.2 SqliteSessionStore

`SqliteSessionStore` also keeps its current external role but changes its
implementation to use `turso`.

Methods that become `async`:

- `open`
- `save_session`
- `save_message`
- `list_sessions`
- `search_sessions`
- `delete_session`
- `list_messages`

Return types remain unchanged wherever practical:

- `anyhow::Result<Vec<SessionSummary>>`
- `anyhow::Result<Vec<StoredMessage>>`
- `anyhow::Result<()>`

### 5.3 Call-site rule

Direct callers must be updated to use `.await` rather than introducing blocking
bridges such as `block_on`.

This avoids mixing blocking database wrappers into the existing Tokio runtime.

---

## 6. Schema and behavior preservation

### 6.1 Schema policy

The existing table and index definitions remain semantically unchanged:

- `goals`
- `sessions`
- `messages`

`CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` continue to be
used during startup/initialization.

This migration is an engine replacement, not a schema redesign.

### 6.2 Goal semantics

The following `GoalStore` behaviors must stay unchanged:

- a session keeps at most one current goal
- `set_goal` replaces any existing goal for the same `session_id`
- `get_active_goal` returns only goals with status `active`
- token/time/turn counters continue to accumulate exactly as before

`set_goal` should be executed transactionally so that deleting the old row and
inserting the new row cannot leave an intermediate empty state if an error
occurs mid-operation.

### 6.3 Session-store semantics

The following `SqliteSessionStore` behaviors must stay unchanged:

- `save_session` preserves `created_at` on update
- `save_message` is idempotent by `msg_id`
- `message_count` increases only when a message is newly inserted
- `list_sessions` sorts by `updated_at DESC`
- `search_sessions` preserves current title/content search behavior
- `list_messages` returns messages oldest-first

`save_message` should run the insert and the session counter update in one
transaction for stronger consistency than the current implementation.

---

## 7. Data mapping rules

`rusqlite` row extraction is replaced by explicit Turso value conversion.

Mapping rules:

- `TEXT -> String`
- nullable `TEXT -> Option<String>`
- `INTEGER -> i64`, then explicit conversion to `u64` / `u32` where needed
- `REAL -> f64`
- nullable numeric fields remain `Option<_>`

Field format rules remain unchanged:

- goal timestamps stay as integer milliseconds
- session/message timestamps stay as RFC3339 strings

The migration must not silently change on-disk value formats.

---

## 8. Testing strategy

### 8.1 Unit tests

Existing `goal.rs` tests remain but are converted to async tests:

- switch from `#[test]` to `#[tokio::test]` where database access is involved
- keep `:memory:` coverage using `Builder::new_local(":memory:")`

### 8.2 New focused regression coverage

Add a small amount of focused regression testing:

- `set_goal` replacement remains atomic and returns the new goal
- `save_message` remains idempotent and does not over-increment
  `message_count`

No broad test expansion is required.

### 8.3 Verification

Minimum verification steps:

- `cargo check`
- targeted tests for migrated modules
- fix any diagnostics introduced by async signature changes

---

## 9. Risks and mitigations

### 9.1 Async signature spread

Risk:

- converting store methods to `async` can force upstream signature changes

Mitigation:

- migrate `GoalStore` first because its call surface is smaller
- update direct callers immediately rather than layering adapters

### 9.2 Row conversion errors

Risk:

- explicit Turso value extraction can introduce type-conversion mistakes

Mitigation:

- centralize repeated conversion helpers
- keep test coverage around nullable fields and integer counters

### 9.3 Transaction consistency regressions

Risk:

- multi-step mutations may lose atomicity during migration

Mitigation:

- use transactions for `set_goal`
- use transactions for `save_message`

### 9.4 Naming mismatch

Risk:

- `SqliteSessionStore` becomes an implementation-misaligned name

Mitigation:

- accept it as a temporary compatibility compromise
- defer renaming to a follow-up cleanup

---

## 10. Implementation order

1. Update `Cargo.toml` dependencies from `rusqlite` to `turso`
2. Migrate `src/core/goal.rs` to Turso and async methods
3. Update `GoalStore` call sites such as `src/tools/goal_complete.rs`
4. Migrate `src/core/sqlite_storage.rs` to Turso and async methods
5. Update remaining direct callers and fix compile errors
6. Convert affected tests to async and add focused regression coverage
7. Run `cargo check` and relevant tests
8. Update README references that describe SQLite as the implementation detail

This order reduces risk by migrating the smaller, more isolated goal-storage
path before touching the larger session-storage surface.

---

## 11. Success criteria

The migration is complete when all of the following are true:

- the project no longer directly depends on `rusqlite`
- direct SQLite-backed modules use `turso` embedded local databases
- direct call sites compile and run with async storage methods
- goal and session-store behavior matches the pre-migration semantics
- tests and `cargo check` pass
- documentation no longer incorrectly states that `rusqlite` is the storage
  engine
