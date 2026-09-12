# code-kb: Tool Catalog & Schema Specification

## 1. Design Principles for Agent Tools

1. **Token Density Over Verbosity:** Deliver high-signal code intelligence in compact markdown or pseudo-syntax. Strip function bodies unless explicitly requested.
2. **Zero Workspace Friction:** No `workspace_id` or `repo_root` arguments. The server is bound to the workspace session.
3. **Resilient Inputs:** Accept both relative paths (`src/main.rs`) and absolute paths.
4. **Deterministic Ordering:** Stable sorting (alphabetical by identifier, or source order) for high LLM prompt-cache hit rates.

---

## 2. Core Inspection Tools (Read-Only)

### 2.1 `codebase_outline`
Provides a top-level architectural orientation of the repository or sub-package without reading files.

- **Parameters:**
  - `path` (string, optional): Subdirectory to scope the outline to (e.g. `crates/api`). Defaults to root.
  - `depth` (integer, optional, default: `2`): Directory recursion depth.
- **Returns:**
  A compact tree of packages, modules, entry points, and primary exported types.
- **Example Output (~120 tokens):**
  ```text
  c:/source/my-project/
  ├── crates/auth/
  │   ├── models.rs [struct User, enum Role, struct Session]
  │   └── service.rs [struct AuthService, trait Authenticator]
  └── crates/server/
      ├── main.rs [fn main]
      └── routes.rs [GET /login, POST /token, GET /profile]
  ```

---

### 2.2 `file_skeleton`
The primary progressive disclosure tool. Returns all types, functions, signatures, docstrings, and visibility for a file, with **implementation bodies stripped**.

- **Parameters:**
  - `file_path` (string, required): File path relative to workspace root or absolute path.
- **Returns:**
  Syntax-highlighted skeleton showing signatures, line ranges, and line counts of hidden bodies.
- **Example Output (~90 tokens for a 600-line file):**
  ```rust
  // File: src/payment.rs (Lines 1-580)
  
  pub struct PaymentRequest { ... } // L12-28
  
  pub trait PaymentGateway {
      fn charge(&self, req: PaymentRequest) -> Result<Receipt>; // L34-36
      fn refund(&self, receipt_id: &str) -> Result<()>;        // L38-40
  }
  
  pub struct StripeGateway { ... } // L45-60
  
  impl PaymentGateway for StripeGateway {
      /// Charges a customer via Stripe v3 API.
      pub fn charge(&self, req: PaymentRequest) -> Result<Receipt> { /* 84 lines hidden: L65-L149 */ }
      
      pub fn refund(&self, receipt_id: &str) -> Result<()> { /* 42 lines hidden: L151-L193 */ }
  }
  ```

---

### 2.3 `find_symbol`
Fast semantic lookup across symbols in the repository, replacing text grep.

- **Parameters:**
  - `query` (string, required): Symbol name or pattern (e.g. `PaymentRequest`, `charge`).
  - `kind` (string, optional): Filter by kind (`function`, `struct`, `trait`, `class`, `interface`, `enum`).
  - `is_test` (boolean, optional, default: `false`): When false, excludes test functions and test containers.
- **Returns:**
  Matching definitions, signatures, docstrings, and file locations.
- **Example Output (~60 tokens):**
  ```text
  Found 1 symbol matching "PaymentGateway":
  - trait PaymentGateway [src/payment.rs:34-42]
    Signature: pub trait PaymentGateway
    Doc: Core payment provider interface.
  ```

---

### 2.4 `search_symbols`
Tier 2 In-Database Full-Text Search (SQLite FTS5) with BM25 ranking over symbol names, signatures, and docstrings. Use when exact symbol names are unknown and searching by functional concepts.

- **Parameters:**
  - `query` (string, required): Keywords or natural language query (e.g. `parse tokens`, `reconcile offline edits`).
  - `kind` (string, optional): Filter by kind (`function`, `struct`, `trait`, `class`, `interface`, `enum`).
  - `is_test` (boolean, optional, default: `false`): When false, excludes test functions and test containers.
  - `limit` (integer, optional, default: `20`): Maximum number of symbols to return.
- **Returns:**
  BM25-ranked symbols with match snippets highlighting query hits.
- **Example Output (~80 tokens):**
  ```text
  Found 1 symbol matching concept "format skeleton":

  - function `format_file_skeleton` [crates/code-kb-core/src/formatters.rs:7-42] (score: -14.56)
    Signature: pub fn format_file_skeleton(file_path: &str, symbols: &[Symbol], line_count: Option<usize>) -> String
    Match: [Format] progressive disclosure file [skeleton] with implementation bodies stripped.
  ```

---

### 2.5 `get_symbol_body`
Retrieves the exact implementation body of a specific symbol.

- **Parameters:**
  - `symbol_name` (string, required): Full or qualified symbol name (e.g. `StripeGateway::charge`).
  - `file_path` (string, optional): Disambiguate if multiple symbols share the name.
- **Returns:**
  Exact source code slice using `start_byte..end_byte`.
- **Example Output:**
  ```rust
  // src/payment.rs:65-149 (StripeGateway::charge)
  pub fn charge(&self, req: PaymentRequest) -> Result<Receipt> {
      let client = self.client.lock()?;
      client.post("/v1/charges", &req)?;
      Ok(Receipt::new())
  }
  ```

---

### 2.6 `get_context_slice`
The "Surgical Context Bundle". Packages everything an agent needs to edit a symbol in one call.

- **Parameters:**
  - `symbol_name` (string, required): Target symbol to edit.
  - `depth` (integer, optional, default: `1`): Dependency depth.
- **Returns:**
  1. Full implementation body of the target symbol.
  2. Signatures of all callee symbols called by it.
  3. Type definitions for parameters and return types.
  4. Unit test functions targeting this symbol.
- **Example Output (~250 tokens total):**
  ```markdown
  ### Target: `OrderService::checkout` (src/order.rs:45-85)
  ```rust
  pub fn checkout(&self, cart: Cart) -> Result<Receipt> {
      self.inventory.reserve(&cart.items)?;
      self.payment.charge(cart.total())
  }
  ```
  
  ### Dependencies (Signatures):
  - `InventoryClient::reserve(&self, items: &[Item]) -> Result<()>` (src/inventory.rs:12)
  - `PaymentGateway::charge(&self, amount: Amount) -> Result<Receipt>` (src/payment.rs:34)
  
  ### Types:
  - `struct Cart { pub items: Vec<Item>, pub user_id: UserId }` (src/models.rs:20)
  
  ### Related Tests:
  - `test_checkout_insufficient_inventory` (tests/order_test.rs:110)
  ```

---

### 2.7 `find_references`
Discovers callers or callees of a symbol.

- **Parameters:**
  - `symbol_name` (string, required): Target symbol name.
  - `direction` (string, required, enum: `["callers", "callees"]`).
- **Returns:**
  List of referencing symbols, their containing files, and call site line numbers.

---

### 2.8 `find_structural_facts`
Queries framework-level and domain-level facts extracted by tree-sitter.

- **Parameters:**
  - `category` (string, required, enum: `["route", "query", "model", "config"]`).
- **Returns:**
  List of HTTP routes (`GET /api/orders`), SQL tables (`CREATE TABLE users`), or config keys used.

---

## 3. Proposed AST-Guided Edit Tool: `replace_symbol_body`

### The Problem with Line/String Replacement
Standard agent edit tools (`replace_file_content`) suffer from:
1. **Line number drift:** Earlier edits shift line numbers, causing later edits to fail.
2. **Indentation/Whitespace sensitivity:** Agents frequently misalign 2 vs 4 spaces or tabs.
3. **Accidental Syntax Errors:** Missing braces or unclosed strings break builds.

### The Solution: AST-Grounded Body Replacement
```json
{
  "name": "replace_symbol_body",
  "description": "Atomically replaces the implementation body of a function or method by symbol name.",
  "parameters": {
    "symbol_name": { "type": "string", "description": "Name of symbol to edit" },
    "file_path": { "type": "string", "description": "Path to file containing symbol" },
    "new_body": { "type": "string", "description": "New body content" },
    "expected_body_hash": { "type": "string", "description": "Optional optimistic lock hash" }
  }
}
```

### Safety Guarantees:
1. **Byte-Span Precision:** Replaces `symbols.body_start_byte..symbols.body_end_byte` directly. Zero line number drift.
2. **Pre-Flight Tree-Sitter Validation:** Parses the edited snippet before touching disk. If a syntax error is detected, rejects the edit immediately with the diagnostic.
3. **Optimistic Locking:** Verifies `symbols.body_hash` to ensure no concurrent modification occurred.
4. **Immediate Re-Index:** Runs `julie-extract update --file <path>` in 5ms to keep the database fresh.
