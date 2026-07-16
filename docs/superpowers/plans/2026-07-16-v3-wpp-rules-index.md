# v3 WPP Rule Index — extracted from compiler source (compactc 0.31.1)

> **Ground-truth index for the native Witness Protection Program (WPP) re-implementation.**
> Every v3a interpreter task consumes this document. Each rule is transcribed
> verbatim from the `track-witness-data` pass (the WPP) and its data tables at the
> pinned commit below. Downstream Rust tasks are FORBIDDEN from using training-data
> knowledge of Compact — this index (and the cited source) is the only trusted source.

## Pinned source revision

| Field | Value |
|---|---|
| Repo | `LFDT-Minokawa/compact` |
| **Release tag** | **`compactc-v0.31.1`** |
| **Commit SHA read** | **`0da5b0452eb0c1053d42418bf34b12cc29c7d63e`** (2026-06-25, "Merge pull request #537 … Compactc v0.31.1") |
| `compiler/compiler-version.ss` | `(make-version 'compiler 0 31 1)` → **Toolchain 0.31.1** ✓ |
| `compiler/language-version.ss` | `(make-version 'language 0 23 0)` → **language 0.23.0** ✓ (matches plan constraint `pragma language_version >= 0.23`) |
| WPP pass | `compiler/analysis-passes.ss` — `define-pass track-witness-data` at **line 4696** |
| Native conduit table | `compiler/midnight-natives.ss` (via `declare-native-entry` macro in `compiler/natives.ss`) |
| Ledger op table | `compiler/ledger.ss` (`parse-disclosure`, line 145) + `compiler/midnight-ledger.ss` (per-op `discloses` clauses) |

All line numbers below are for `analysis-passes.ss` **at commit `0da5b045`** (fetched directly from that SHA via the GitHub contents API; 5622 lines total).

### ⚠️ SHA / version discrepancy vs. the brief — READ THIS

The task brief and the Rev-2 spec pin the WPP at commit **`03da643`**. **That SHA is wrong.**
Verified against the GitHub API:

- `03da643acebb6f4627a8141895c6645a9b2727d0` (2026-07-15, "Merge PR #604 … Reject secp256k1 identity points") is at **Toolchain 0.33.107 / language 0.25.102 / runtime 0.18.101** — i.e. `compact compile --version` there is **0.33.107, not 0.31.1**.
- The analyzer targets **0.31.1** (confirmed locally: `compact compile --version` → `0.31.1`) and the plan's own Global Constraints say "compiler 0.31.x / language 0.23.0". Both point to `compactc-v0.31.1` = commit `0da5b045` (language 0.23.0), which is what this index uses.

**This index is extracted from the true 0.31.1 revision (`0da5b045`), not from `03da643`.** Downstream tasks and the R2 fixture set must use `0da5b045`, and the spec's pinned-SHA constant should be corrected to `0da5b045` (or `compactc-v0.31.1`).

### ⚠️ `emit` sink does NOT exist in 0.31.1

The brief lists **`emit`** as a WPP sink (and the R2 plan lists `emit_leak.compact` / `emit_disclosed.compact` fixtures). **There is no `emit`/events feature in compactc 0.31.1.** Events/`emit` (formerly `log`) were introduced in PR #470 ("[Issue 377] Events — phase 1", commit `0b3be94d`, merged **2026-06-26**, one day *after* the 0.31.1 release on 2026-06-25). At `0da5b045` there are no `events.ss` / `midnight-events.ss` files and no `emit`/`log` expression node in `track-witness-data`. The complete set of WPP sinks in 0.31.1 is **`public-ledger`, `contract-call`, and `return`** (plus a stdlib-circuit leak-remapping). **Drop the `emit_*` fixtures for 0.31.1**, or retarget the whole analyzer to a version that has events (a separate decision — not what the plan currently specifies).

### ⚠️ `contract-call` is NOT gated on `pure?` in the 0.31.1 WPP

The brief describes `contract-call` as "gated on `pure?` (impure cross-contract call is a sink)". In 0.31.1 the WPP's `contract-call` arm (line 5542) records leaks **unconditionally** — it never consults the callee's purity. Purity (`pure-dcl*`) is used only by the *earlier* `identify-pure-circuits` pass (line 4483) to decide whether the *calling* circuit is impure; it does not gate the WPP sink. So in 0.31.1 **every `contract-call` that carries witness data leaks**, pure or not. A "pure call is accepted" fixture is accepted only because it passes *no witness data*, not because purity suppresses the sink. (In v3a this sink is deferred to v3b as an amber advisory anyway — see plan Task A8.)

---

## Analyzer model (from the pass's own comments, lines 4696–4763)

The WPP is an **abstract interpreter**. Abstract values are the `Abs` datatype; each
tracked witness value is a `witness` record; a `Witness-Info` records where a witness
entered the contract; `path-point`s record interesting points on the witness→sink trail.

```scheme
; analysis-passes.ss:4707
(define-datatype Abs
  (Abs-atomic witness*)          ; scalar / anything not struct/tuple/array/const-bool
  (Abs-boolean true? witness*)   ; ONLY compile-time-constant booleans (carries true?)
  (Abs-multiple abs*)            ; struct & tuple: fields tracked INDIVIDUALLY
  (Abs-single abs))             ; array/vector: elements tracked in the AGGREGATE

; analysis-passes.ss:4714 — invariant: each witness* is sorted by uid, no duplicates
(define-record-type witness (fields src uid info path*))

; analysis-passes.ss:4739
(define-datatype Witness-Info
  (Witness-Return-Value function-name)     ; source #1
  (Constructor-Argument argument-name)     ; source #2
  (Circuit-Argument function-name argument-name)) ; source #3

; analysis-passes.ss:4754
(define-datatype Fun
  (Fun-circuit src name var-name* expr uid)
  (Fun-witness abs)              ; a witness call returns this fixed tainted abs
  (Fun-native disclosure?* type)) ; per-arg conduit flags + result type
```

**Confirmed WPP error text** (the string the differential harness greps for), `analysis-passes.ss:5218`:

```scheme
(pending-errorf src
  "potential witness-value disclosure must be declared but is not:\n    witness value potentially disclosed:\n      ~a~{~a~}"
  ...)
```

**"Nature of the disclosure" sub-template**, `analysis-passes.ss:5221`:

```scheme
"\n    nature of the disclosure:\n      ~a might disclose ~a~@[\n    via this path through the program:~{\n      ~a~}~]"
```

In that template the first `~a` is the sink's **nature string** (the `what` argument of `record-leak!`, tabulated below); the second `~a` is the accumulated **exposure** string (built from path-point exposures, defaulting to `"the witness value"`, line 5214).

---

## Sources  (§3.1) — where witness taint enters

A source seeds `witness` records via `default-value` (which shapes the `Abs` from the
declared type; §Abs-shapes) with the leaf witnesses set to a fresh witness carrying the
right `Witness-Info`.

### S1 — Witness-function return value
- **Source anchor** `analysis-passes.ss:5254`
  ```scheme
  [(witness ,src ,function-name (,arg* ...) ,type)
   (hashtable-set! function-ht function-name
     (Fun-witness
       (default-value type
         (list (make-witness src (next-witness-uid)
                 (Witness-Return-Value function-name))))))]
  ```
  Retrieved on every call, `analysis-passes.ss:5108` — `[(Fun-witness abs) abs]`.
- **Behavior** A call to a witness function returns an `Abs` (shaped by its return
  type) whose every leaf carries a single `Witness-Return-Value` witness. This is the
  ONLY source that also leaks on `return` (see K7 / `filter-witnesses`).
- **Nature string** n/a (source, not a sink).
- **Fixture** `return_witness_leak.compact` (leaks via return), `hash_then_leak.compact` (leaks via ledger).
- **Fail-closed** n/a.

### S2 — Constructor argument
- **Source anchor** `analysis-passes.ss:5272`
  ```scheme
  [(public-ledger-declaration ,pl-array (constructor ,src ((,var-name* ,type*) ...) ,expr))
   (Expression expr
     (extend-env empty-env var-name*
       (map default-value type*
            (map (lambda (var-name)
                   (list (make-witness (id-src var-name) (next-witness-uid)
                           (Constructor-Argument var-name))))
                 var-name*)))
     '() #f)]  ; note: disclosing-function-name? = #f ⇒ constructor body cannot leak on return
  ```
- **Behavior** Each constructor parameter seeds an `Abs` (shaped by its type) whose
  leaves carry a `Constructor-Argument` witness. The constructor body is interpreted
  with `disclosing-function-name? = #f`, so a constructor argument NEVER leaks via
  `return`; it leaks only at ledger / contract-call sinks.
- **Nature string** n/a.
- **Fixture** `constructor_arg_leak.compact`.
- **Fail-closed** n/a.

### S3 — Exported-circuit argument
- **Source anchor** `analysis-passes.ss:5265`
  ```scheme
  [(circuit ,src ,function-name ((,var-name* ,type*) ...) ,type ,expr)
   (when (id-exported? function-name)
     (let ([witness** (maplr (lambda (var-name)
                               (list (make-witness (id-src var-name) (next-witness-uid)
                                       (Circuit-Argument function-name var-name))))
                             var-name*)])
       (handle-call #f function-name (map default-value type* witness**) '() #t)))]
  ```
- **Behavior** Only **exported** circuits are roots. Each exported-circuit parameter
  seeds an `Abs` whose leaves carry a `Circuit-Argument` witness. Root invocation uses
  `handle-call` with `return-value-discloses? = #t` (last arg) so that return-value
  leaks are reported for this root. Non-exported circuits are analyzed only as callees.
- **Nature string** n/a.
- **Fixture** `ledger_leak.compact`, `emit`→N/A, `impure_call_leak.compact`, `implicit_flow_leak.compact` (all use an exported circuit param as the witness source).
- **Fail-closed** If a parameter's declared type resolves to an unknown/`Unknown`
  v2b type, the native analyzer must emit an amber advisory and seed `Abs-atomic([w])`
  (never silently untainted) — plan Task A2.

---

## Sinks  (§3.2) — where taint leaking is reported

Every sink calls `record-leak! src what witness*` (`analysis-passes.ss:5160`), which
accumulates `witness*` into a leak table keyed by `(src . what)`. `what` is the
**nature string**. Leaks are drained and rendered by `complain` at end of `Program`
(`analysis-passes.ss:5244`). Complete list of `record-leak!` sites: 5100, 5525, 5537,
5544, 5546, 5550, 5575, 5580.

### K1 — Ledger operation argument (public-ledger write/update)
- **Source anchor** `analysis-passes.ss:5521`
  ```scheme
  [(public-ledger ,src ,ledger-field-name ,sugar? (,path-elt* ...) ,src^ ,adt-op ,[* abs*] ...)
   (nanopass-case (Lwithpaths ADT-Op) adt-op
     [(,ledger-op ,op-class (,adt-name (,adt-formal* ,adt-arg*) ...) ((,var-name* ,type* ,discloses?*) ...) ,type ,vm-code)
      ...
      (for-each
        (lambda (abs discloses? i?)
          (when discloses?                    ; ← leaks iff the op-arg's discloses? flag is truthy
            (let ([witness* (abs->witnesses
                              (add-path-point src^
                                (if sugar? (format "the right-hand side of ~a" sugar?)
                                    (format "the ~@[~:r ~]argument to ~a" (and i? (fx+ i? 1)) ledger-op))
                                discloses? abs))])
              (unless (null? witness*)
                (record-leak! src^ "ledger operation" witness*)))))
        abs* discloses?* ...)
      (default-value type)])]
  ```
- **Behavior** For each argument of a ledger ADT operation, if that arg's `discloses?`
  flag (from the Ledger-op table) is **truthy**, any witnesses reaching the arg are an
  **immediate leak**. The op returns `(default-value type)` — untainted. A `discloses?`
  of `""` (the DEFAULT for any op arg with no explicit clause) is truthy ⇒ leaks. See
  Ledger-op table below.
- **Nature string** `"ledger operation"` (verbatim). Path exposure = the `discloses?`
  string (e.g. `"a link between…"`) or `""`.
- **Fixture** `ledger_leak.compact` (reject) / `ledger_disclosed.compact` (accept, `disclose()` added).
- **Fail-closed** Unknown ledger op / unrecognized op-arg shape ⇒ treat as **sink**
  (leak) + advisory (plan §0 / Task A6).

### K2 — Implicit flow at a ledger operation (control witnesses)
- **Source anchor** `analysis-passes.ss:5524`
  ```scheme
  (unless (null? control-witness*)
    (record-leak! src^ "performing this ledger operation" control-witness*))
  ```
- **Behavior** If any control witnesses are live (a witness governed a branch enclosing
  this ledger op), performing the op leaks them — the fact that the op executed reveals
  the witness. This is a SEPARATE leak class from the data leak K1 (distinct table entry).
- **Nature string** `"performing this ledger operation"` (verbatim).
- **Fixture** `implicit_flow_leak.compact` (reject) / `implicit_flow_disclosed.compact` (accept).
- **Fail-closed** n/a (control set is always known intraprocedurally).

### K3 — Contract-call, control witnesses
- **Source anchor** `analysis-passes.ss:5542`
  ```scheme
  [(contract-call ,src ,elt-name (,[* abs] ,type) ,[* abs*] ...)
   (unless (null? control-witness*)
     (record-leak! src "making this contract call" control-witness*))
   ...]
  ```
- **Behavior** Making a cross-contract call under witness-tainted control leaks the
  control witnesses. Unconditional — NOT gated on callee purity (see divergence note).
- **Nature string** `"making this contract call"` (verbatim).
- **Fixture** `impure_call_leak.compact` (v3a: expected to become an amber advisory, deferred to v3b).
- **Fail-closed** v3a defers the whole contract-call sink to an amber advisory (Task A8).

### K4 — Contract-call, contract reference
- **Source anchor** `analysis-passes.ss:5545`
  ```scheme
  (let ([witness* (abs->witnesses abs)])
    (unless (null? witness*) (record-leak! src "contract call contract reference" witness*)))
  ```
- **Behavior** Witnesses in the *contract-reference* operand (the callee address) leak.
- **Nature string** `"contract call contract reference"` (verbatim).
- **Fixture** (covered by contract-call fixtures; deferred to v3b).
- **Fail-closed** as K3.

### K5 — Contract-call, argument N
- **Source anchor** `analysis-passes.ss:5547`
  ```scheme
  (for-each
    (lambda (abs i)
      (let ([witness* (abs->witnesses abs)])
        (unless (null? witness*) (record-leak! src (format "contract call argument ~d" (fx+ i 1)) witness*))))
    abs* (enumerate abs*))
  ```
- **Behavior** Witnesses in any argument of a cross-contract call leak. Result is
  `(default-value …)` of the called element's return type — untainted.
- **Nature string** `"contract call argument N"` (1-based; `format "contract call argument ~d"`).
- **Fixture** `impure_call_leak.compact` (deferred to v3b advisory).
- **Fail-closed** as K3.

### K6 — Return, control witnesses (implicit flow through return)
- **Source anchor** `analysis-passes.ss:5573`
  ```scheme
  (let ([control-witness* (filter-witnesses control-witness*)])
    (unless (null? control-witness*)
      (record-leak! src
        (format "returning this value from exported circuit ~s" (id-sym disclosing-function-name?))
        control-witness*)))
  ```
- **Behavior** Only fires when `disclosing-function-name?` is set (i.e. within an
  exported-circuit root — S3). Control witnesses are first passed through
  `filter-witnesses` (K7 asymmetry), so only `Witness-Return-Value` witnesses survive.
- **Nature string** `"returning this value from exported circuit <name>"` (`format … ~s`).
- **Fixture** `implicit_flow_leak.compact` variant with witness-return in condition.
- **Fail-closed** n/a.

### K7 — Return, returned value + the `filter-witnesses` asymmetry
- **Source anchor** `analysis-passes.ss:5561`
  ```scheme
  [(return ,src ,[* abs])
   (when disclosing-function-name?
     (let ()
       (define (filter-witnesses witness*)
         (filter
           (lambda (witness)
             ; don't report exposure of an exported circuit's own arguments via the circuit's return value
             (Witness-Info-case (witness-info witness)
               [(Witness-Return-Value function-name) #t]     ; ← ONLY witness-returns leak on return
               [(Constructor-Argument argument-name) #f]     ; ← circuit/constructor args pass through freely
               [(Circuit-Argument function-name argument-name) #f]))
           witness*))
       ...
       (let ([witness* (filter-witnesses (abs->witnesses abs))])
         (unless (null? witness*)
           (record-leak! src
             (format "the value returned from exported circuit ~s" (id-sym disclosing-function-name?))
             witness*)))))
   abs])
  ```
- **Behavior** THE key sink asymmetry (§3.2): returning a value from an exported circuit
  leaks **only** `Witness-Return-Value` taint. Returning a circuit's own argument
  (`Circuit-Argument`) or a constructor argument (`Constructor-Argument`) is NOT a
  disclosure — those are dropped by `filter-witnesses`. Applies to both the returned
  value and the control witnesses (K6).
- **Nature string** `"the value returned from exported circuit <name>"` (`format … ~s`).
- **Fixture** `return_witness_leak.compact` (reject — witness return) / `return_arg_ok.compact` (accept — returning a circuit arg).
- **Fail-closed** n/a.

### K8 — Standard-library circuit call leak-remapping
- **Source anchor** `analysis-passes.ss:5099`
  ```scheme
  (if (and src? (not (stdlib-src? src?)) (stdlib-src? (id-src function-name)))
      (fluid-let ([record-leak!
                   (let ([record-leak! record-leak!])
                     (lambda (ignore-src ignore-what witness)
                       (record-leak! src? (format "the call to standard-library circuit ~a" (id-sym function-name)) witness)))])
        (go))
      (go))
  ```
- **Behavior** When a user calls a stdlib circuit that internally hits a sink, the leak
  is re-attributed to the *user's* call site with nature
  `"the call to standard-library circuit <name>"` (so errors point at user code, not
  library internals). Not a distinct sink — a wrapper over K1–K7 for stdlib callees.
- **Nature string** `"the call to standard-library circuit <name>"` (`format … ~a`).
- **Fixture** (optional) a call to a stdlib circuit that writes a witness to the ledger.
- **Fail-closed** n/a.

### K-emit — NOT PRESENT in 0.31.1
- The `emit` / events sink does not exist at `0da5b045` (added post-0.31.1 in PR #470).
  No rule. See the emit divergence note above. `emit_leak` / `emit_disclosed` fixtures
  are not applicable to 0.31.1.

---

## Sanitizer  (§3.5)

### D1 — `disclose(e)` clears witness taint at every level
- **Source anchor** `analysis-passes.ss:5462` (dispatch) + `5147` (impl)
  ```scheme
  [(disclose ,src ,[* abs]) (disclose abs)]
  ; ---
  (define (disclose abs)
    (Abs-case abs
      [(Abs-atomic witness*) (Abs-atomic '())]
      [(Abs-boolean true? witness*) (Abs-boolean true? '())]
      [(Abs-multiple abs*) (Abs-multiple (map disclose abs*))]
      [(Abs-single abs) (Abs-single (disclose abs))]))
  ```
- **Behavior** `disclose` recursively empties the `witness*` at EVERY level of the
  `Abs` (atomic, boolean, all `Multiple` fields, the `Single` element). The shape is
  preserved; only the taint is removed. This is the ONLY sanitizer.
- **Nature string** n/a.
- **Fixture** every `*_disclosed.compact` accept fixture.
- **Fail-closed** n/a.

### D2 — Hashing is NOT a sanitizer (but commit effectively is)
- **Source anchor** `midnight-natives.ss:16` (`transientHash` — `discloses "a hash of"`) vs `midnight-natives.ss:21` (`transientCommit` — `discloses nothing`).
- **Behavior** `transientHash`/`persistentHash`/`hashToCurve` are conduits with
  exposure `"a hash of"` — the witness flows through into the result (NOT sanitized).
  By contrast `transientCommit`/`persistentCommit` mark their `value` arg
  `(discloses nothing)` ⇒ the witness does NOT flow into the result, so committing
  effectively hides the witness. (Detail in the Native conduit table.)
- **Nature string** n/a (exposure `"a hash of"`).
- **Fixture** `hash_then_leak.compact` (reject — `transientHash(w)` then ledger write; hashing didn't sanitize).
- **Fail-closed** n/a.

---

## Abs shapes  (§3.4) — `default-value`

### AB1 — Type-driven `Abs` shape
- **Source anchor** `analysis-passes.ss:5124`
  ```scheme
  (define default-value
    (case-lambda
      [(type) (default-value type '())]
      [(type witness*)
       (let default-value ([type type])
         (nanopass-case (Lwithpaths Type) type
           [(tstruct ,src ,struct-name (,elt-name* ,type*) ...) (Abs-multiple (map default-value type*))]
           [(ttuple ,src ,type* ...) (Abs-multiple (map default-value type*))]
           [(tvector ,src ,len ,type) (Abs-single (default-value type))]
           [(talias ,src ,nominal? ,type-name ,type) (default-value type)]
           [else (Abs-atomic witness*)]))]))
  ```
- **Behavior** struct → `Abs-multiple` (one child per field, fields tracked
  individually); tuple → `Abs-multiple`; vector/array → `Abs-single` (elements
  aggregated — the single child abstracts ALL elements); alias → recurse on aliased
  type; everything else (Field, Boolean, Bytes, Uint, enum, opaque, …) → `Abs-atomic`.
  The seed `witness*` is placed at every atomic leaf (default `'()` = untainted).
- **Nature string** n/a.
- **Fixture** `comingled_return.compact` (struct field granularity), plus every source fixture.
- **Fail-closed** Unknown/`Unknown` v2b type ⇒ advisory + `Abs-atomic([seed])` (plan Task A2).

### AB2 — Booleans tracked only for compile-time constants
- **Source anchor** `analysis-passes.ss:5308`
  ```scheme
  [(quote ,src ,datum)
   (case datum
     [(#t) (Abs-boolean #t '())]
     [(#f) (Abs-boolean #f '())]
     [else (Abs-atomic '())])]
  ```
- **Behavior** `Abs-boolean` (carrying the compile-time `true?` value) is produced ONLY
  by literal `#t`/`#f`. It enables constant-folding of `if` (K-if). Any runtime boolean
  is `Abs-atomic`. On a join of two `Abs-boolean` with differing `true?`, the result
  decays to `Abs-atomic` (see `combine-abs`, F2).
- **Nature string** n/a.
- **Fixture** (covered by `if` fixtures).
- **Fail-closed** n/a.

---

## Natives — the `Fun-native` conduit table  (§3.6)

### N1 — Native conduit mechanism
- **Source anchor** `analysis-passes.ss:5109`
  ```scheme
  [(Fun-native disclosure?* type)
   (assert (fx= (length disclosure?*) (length abs*)))
   (default-value type
     (fold-left
       (lambda (witness* abs disclosure? i?)
         (if disclosure?
             (merge-witnesses
               (abs->witnesses (if src? (add-path-point src? (format "the ~@[~:r ~]argument to ~a" (and i? (fx+ i? 1)) (id-sym function-name)) disclosure? abs) abs))
               witness*)
             witness*))
       '() abs* disclosure?* ...))]
  ```
- **Behavior** A native (built-in circuit) is a **conduit, not a sink** — it records NO
  leak. For each arg whose `disclosure?` flag is a **string** (truthy), that arg's
  witnesses flow into the native's RESULT (shaped by the native's return type via
  `default-value`), tagged with a path-point whose *exposure* is the flag string. Args
  flagged `#f` (`discloses nothing`) do NOT contribute their witnesses to the result.
- **Nature string** n/a; the exposure is the per-arg flag string (table below).
- **Fixture** `hash_then_leak.compact` (`transientHash` conduit → ledger leak).
- **Fail-closed** **Unknown native ⇒ conduit-taint ALL args into the result + advisory**
  (plan §0 / Task A6). (In-source, all natives are known; the fail-closed rule governs
  the analyzer when a callee cannot be resolved to a known native.)

### N2 — The per-native `disclose?*` flag table (verbatim from `midnight-natives.ss`)

Flag meaning: `"…"` = arg's witnesses flow into result with this exposure (conduit);
`nothing` = witnesses do NOT flow (arg hidden). `class` = circuit unless noted.

| Native (`class`) | Arg → flag | Result type | Source |
|---|---|---|---|
| `transientHash[A]` | `value` → `"a hash of"` | `Field` | `midnight-natives.ss:16` |
| `transientCommit[A]` | `value` → `nothing`; `rand` → `nothing` | `Field` | `:21` |
| `persistentHash[A]` | `value` → `"a hash of"` | `Bytes<32>` | `:27` |
| `persistentCommit[A]` | `value` → `nothing`; `rand` → `nothing` | `Bytes<32>` | `:32` |
| `degradeToTransient` | `x` → `"a modulus of"` | `Field` | `:38` |
| `upgradeFromTransient` | `x` → `"a converted form of"` | `Bytes<32>` | `:43` |
| `jubjubPointX` | `np` → `"the X coordinate of"` | `Field` | `:48` |
| `jubjubPointY` | `np` → `"the Y coordinate of"` | `Field` | `:53` |
| `ecAdd` | `a` → `"an elliptic curve sum including"`; `b` → `"an elliptic curve sum including"` | `JubjubPoint` | `:58` |
| `ecMul` | `a` → `"an elliptic curve product including"`; `b` → `"an elliptic curve product including"` | `JubjubPoint` | `:64` |
| `ecMulGenerator` | `b` → `"the product of the embedded group generator with"` | `JubjubPoint` | `:70` |
| `hashToCurve[A]` | `value` → `"a hash of"` | `JubjubPoint` | `:75` |
| `constructJubjubPoint` | `x` → `"a JubjubPoint containing x coordinate"`; `y` → `"a JubjubPoint containing y coordinate"` | `JubjubPoint` | `:80` |
| `ownPublicKey` (**witness**) | (no args) | `ZswapCoinPublicKey` | `:86` |
| `createZswapInput` (**witness**) | `coin` → `nothing` | `Void` | `:91` |
| `createZswapOutput` (**witness**) | `coin` → `nothing`; `recipient` → `nothing` | `Void` | `:96` |

> `ownPublicKey`/`createZswapInput`/`createZswapOutput` are declared `witness` class
> but appear in the native table; they are treated as native circuits by
> `record-function-kind!` (line 5251), NOT as taint sources. Only `witness`-declared
> user functions (line 5254) are S1 sources.

> Flag STRINGS are compiler message text — copy them **verbatim**; downstream exposure
> wording pins to them.

---

## Ledger ops — the `discloses?` immediate-leak table  (§3.6/§3.8)

### L1 — The default rule (LOAD-BEARING)
- **Source anchor** `ledger.ss:145`
  ```scheme
  (define (parse-disclosure disclosure)
    (syntax-case disclosure (discloses nothing)
      [() ""]                                    ; ← DEFAULT: no clause ⇒ "" (TRUTHY ⇒ leaks)
      [((discloses nothing)) #f]                 ; ← explicitly safe ⇒ does NOT leak
      [((discloses what)) (string? (datum what)) #'what])) ; ← leaks with this exposure
  ```
- **Behavior** A ledger ADT operation argument's `discloses?` flag defaults to `""`
  when the op declaration gives NO disclosure clause. In the WPP's `when discloses?`
  test (K1, line 5528), only `#f` is false — so **`""` is truthy ⇒ any op arg with no
  explicit clause LEAKS witnesses** (exposure `""`). This is why writing a witness value
  into a `Cell`, `Counter`, `Set`, `Map`, `List`, or `MerkleTree` is a disclosure even
  though those ops carry no `discloses` annotation. Only an explicit `(discloses nothing)`
  makes an op arg witness-safe.
- **Fail-closed** unknown ledger op ⇒ treat as sink (default to leaking).

### L2 — Standard container ops (default `discloses? = ""` ⇒ every witness arg leaks)
From `midnight-ledger.ss`. None of these carry a `discloses` clause ⇒ all args leak.

| ADT / op (`class`) | Arg(s) | `discloses?` | Source |
|---|---|---|---|
| `Cell.write` (update) | `value` | `""` ⇒ leak | `midnight-ledger.ss:552` |
| `Cell.writeCoin` (update-with-coin-check) | `coin`, `recipient` | `""` ⇒ leak | `:567` |
| `Counter.increment` (update) | `amount` | `""` ⇒ leak | `:602` |
| `Counter.decrement` (update) | `amount` | `""` ⇒ leak | `:607` |
| `Counter.lessThan` (read) | `threshold` | `""` ⇒ leak | `:595` |
| `Set.member` (read) | `elem` | `""` ⇒ leak | `:649` |
| `Set.insert` (update) | `elem` | `""` ⇒ leak | `:656` |
| `Set.remove` (remove) | `elem` | `""` ⇒ leak | `:663` |
| `Set.insertCoin` (update-with-coin-check) | `coin`, `recipient` | `""` ⇒ leak | `:670` |
| `Map.member` (read) | `key` | `""` ⇒ leak | `:734` |
| `Map.lookup` (read) | `key` | `""` ⇒ leak | `:741` |
| `Map.insert` (update) | `key`, `value` | `""` ⇒ leak | `:748` |
| `Map.insertDefault` (update) | `key` | `""` ⇒ leak | `:755` |
| `Map.remove` (remove) | `key` | `""` ⇒ leak | `:762` |
| `Map.insertCoin` (update-with-coin-check) | `key`, `coin`, `recipient` | `""` ⇒ leak | `:769` |
| `List.pushFront` (update) | `value` | `""` ⇒ leak | `:885` |
| `List.pushFrontCoin` (update-with-coin-check) | `coin`, `recipient` | `""` ⇒ leak | `:918` |
| `MerkleTree.checkRoot` (read) | `rt` | `""` ⇒ leak | `:1031` |
| `MerkleTree.insert` (update) | `item` | `""` ⇒ leak | `:1041` |

> Zero-arg ops (`Cell.read`, `Counter.read`, `Set.isEmpty/size`, `Map.isEmpty/size`,
> `List.isEmpty/length/head`, `*.resetToDefault`, `MerkleTree.isFull`, …) have no args
> ⇒ nothing to leak.

### L3 — Kernel ops with EXPLICIT `discloses` clauses (verbatim exposures)
From `midnight-ledger.ss` (the `Kernel` ADT). These override the L1 default.

| Op (`update`/`read`) | Arg → flag (verbatim) | Source |
|---|---|---|
| `claimZswapNullifier` | `nul` → `"a link between a claim of nullifier and the coin with the nullifier given by"` | `midnight-ledger.ss:163` |
| `claimZswapCoinSpend` | `note` → `"a link between a coin spend and the coin with the commitment given by"` | `:174` |
| `claimZswapCoinReceive` | `note` → `"a link between a coin receive and the coin with the commitment given by"` | `:185` |
| `claimContractCall` | `addr` → `"the address of a contract being called given by"`; `entry_point` → `"the hash of the contract's circuit being called given by"`; `comm` → **`nothing`** (does NOT leak) | `:196` |
| `mintShielded` | `domain_sep` → `"the domain separator of the token being minted given by"`; `amount` → `"the value of a token mint given by"` | `:217` |
| `mintUnshielded` | `domain_sep` → `"the domain separator of the unshielded token being minted given by"`; `amount` → `"the amount of the unshielded token being minted given by"` | `:262` |
| `claimUnshieldedCoinSpend` | `token_type` → `"the type of the unshielded token being transferred given by"`; `address` → `"the recipient of the unshielded token being transferred given by"`; `amount` → `"the amount of the unshielded token being transferred given by"` | `:302` |
| `incUnshieldedOutputs` | `token_type` → `"the type of the unshielded token being spent given by"`; `amount` → `"the amount of the unshielded token being spent given by"` | `:343` |
| `incUnshieldedInputs` | `token_type` → `"the type of the unshielded token being received given by"`; `amount` → `"the amount of the unshielded token being received given by"` | `:382` |
| `balance` (read) | `token_type` → `"the type of the unshielded token having its balanced checked given by"` | `:421` |
| `balanceLessThan` (read) | `token_type` → `"…having its balanced checked given by"`; `amount` → `"the upper bound of the balance of the unshielded token being checked"` | `:450` |
| `balanceGreaterThan` (read) | `token_type` → `"…having its balanced checked given by"`; `amount` → `"the lower bound of the balance of the unshielded token being checked"` | `:483` |
| `blockTimeLessThan` (read) | `time` → `"the lower bound of the time being checked"` | `:516` |
| `blockTimeGreaterThan` (read) | `time` → `"the upper bound of the time being checked"` | `:530` |

> All 24 `discloses` clauses in `midnight-ledger.ss` are captured across L3 (the two
> `nothing` args are `claimContractCall.comm` and — none other; every other clause is a
> string). Exposures are compiler message text — copy verbatim.

---

## Control flow / implicit flow  (§3.3)

### F1 — `if` (branch, join, constant-fold, control-witness threading)
- **Source anchor** `analysis-passes.ss:5319` (expression position); `5286` (effect/statement position)
  ```scheme
  [(if ,src ,[* abs0] ,expr1 ,expr2)
   (let ([control-witness* (merge-witnesses
                             (abs->witnesses (add-path-point src "the conditional branch" "the boolean value of" abs0))
                             control-witness*)])
     (add-witnesses (abs->witnesses (add-path-point src "the conditional expression" "the boolean value of" abs0))
       (Abs-case abs0
         [(Abs-boolean true? witness*) (Expression (if true? expr1 expr2) p control-witness* disclosing-function-name?)]
         [(Abs-atomic witness*) (combine-abs (Expression expr1 …) (Expression expr2 …))]
         [else (assert cannot-happen)])))]
  ```
- **Behavior** (a) The condition's witnesses are merged into `control-witness*` for
  BOTH branches (implicit-flow threading; nature strings recorded at sinks via K2/K6).
  (b) If the condition is a compile-time constant (`Abs-boolean`), only the taken
  branch is analyzed. (c) Otherwise both branches are analyzed and joined with
  `combine-abs` (F2). (d) In **expression** position, the condition's witnesses are
  ALSO added to the result value via `add-witnesses` — a non-constant `if` expression's
  value carries the condition's taint (with exposure `"the boolean value of"`). In
  **effect/statement** position (line 5286) there is no result, so no `add-witnesses`.
- **Nature string** n/a here (the leak nature strings are K2/K6); path-point exposures
  `"the boolean value of"`, descriptions `"the conditional branch"` /
  `"the conditional expression"` (verbatim).
- **Fixture** `implicit_flow_leak.compact` (`if (w) { F.increment(1); }`), `implicit_flow_disclosed.compact`.
- **Fail-closed** n/a.

### F2 — `combine-abs` (the join / lattice union)
- **Source anchor** `analysis-passes.ss:4997`
  ```scheme
  (define (combine-abs abs1 abs2)
    ; invariant: abs1 and abs2 have the same shape
    (Abs-case abs1
      [(Abs-atomic w1*) (… (Abs-atomic (merge-witnesses w1* w2*)) …)]
      [(Abs-boolean t1? w1*)
       (Abs-case abs2
         [(Abs-atomic w2*) (Abs-atomic (merge-witnesses w1* w2*))]
         [(Abs-boolean t2? w2*)
          (if (eq? t2? t1?) (Abs-boolean t1? (merge-witnesses w1* w2*))   ; same const ⇒ stay boolean
                            (Abs-atomic (merge-witnesses w1* w2*)))])]     ; differing const ⇒ DECAY to atomic
      [(Abs-multiple abs1*) …elementwise combine, Single distributes over Multiple…]
      [(Abs-single abs1) …]))
  ```
- **Behavior** Join = **union** of witnesses (`merge-witnesses` — dedup by uid, union
  the path sets). Two constant booleans keep `Abs-boolean` only if their `true?` agree;
  otherwise decay to `Abs-atomic`. `Abs-multiple`/`Abs-single` join elementwise;
  `Single` distributes over `Multiple`. Union direction is symmetric.
- **Nature string** n/a.
- **Fixture** covered by `if` fixtures (A1 unit test `disclose_clears…and_join_unions`).
- **Fail-closed** n/a.

### F3 — `&&` / `||`
- **Source anchor** none — no dedicated arm in `track-witness-data`.
- **Behavior** In `Lwithpaths`, short-circuit `&&`/`||` have already been desugared to
  `if` by earlier passes (`recognize-let`/frontend); there is NO `&&`/`||` node in the
  WPP. They are handled entirely by the `if` rule F1. **Do not** add a separate
  `&&`/`||` arm — treat them as `if`.
- **Nature string** n/a.
- **Fixture** (optional) an `&&` with a witness operand governing a sink.
- **Fail-closed** n/a.

### F4 — `handle-comparison` → atomic union
- **Source anchor** `analysis-passes.ss:5302` (def) + dispatch at `5395`–`5400` (`< <= > >= == !=`)
  ```scheme
  (define (handle-comparison src abs1 abs2)
    (add-path-point src "the comparison" "the result of a comparison involving"
      (Abs-atomic (merge-witnesses (abs->witnesses abs1) (abs->witnesses abs2)))))
  ```
- **Behavior** All six comparisons collapse both operands' witnesses into a single
  `Abs-atomic` (a comparison result is scalar), tagged exposure
  `"the result of a comparison involving"`. Comparisons are NOT sanitizing.
- **Nature string** n/a; exposure `"the result of a comparison involving"`, description `"the comparison"` (verbatim).
- **Fixture** a comparison of a witness leaked to the ledger.
- **Fail-closed** n/a.

### F5 — Arithmetic is not sanitizing (`+ - *`)
- **Source anchor** `analysis-passes.ss:5389`
  ```scheme
  [(+ ,src ,mbits ,[* abs1] ,[* abs2]) (add-path-point src "the computation" "the result of an addition involving" (combine-abs abs1 abs2))]
  [(- …) … "the result of a subtraction involving" …]
  [(* …) … "the result of a multiplication involving" …]
  ```
- **Behavior** `+`/`-`/`*` join operands with `combine-abs` and keep the taint (comment
  at line 5388: "arithmetic isn't sanitizing: could be x + 0, x - 0, x * 1 …").
  Exposures `"the result of an addition/subtraction/multiplication involving"`.
- **Nature string** n/a; exposures verbatim, description `"the computation"`.
- **Fixture** `witness + 0` leaked to ledger (arithmetic does not launder).
- **Fail-closed** n/a.

---

## Fixpoint (fold / map)  (§3.9)

### FX1 — `fold`
- **Source anchor** `analysis-passes.ss:5428`
  ```scheme
  [(fold ,src ,len ,fun (,[* abs0] ,type0) ,[* abs] ,[* abs*] ...)
   (if (= len 0) abs0
       (… (if (aggregate? …)   ; Abs-single elements ⇒ bounded fixpoint
              (let loop ([abs (Function fun … (cons abs0 abs+) …)] [len len])
                (if (= len 1) abs
                    (let ([abs^ (Function fun … (cons abs abs+) …)])
                      (if (abs-equal? abs^ abs) abs (loop abs^ (- len 1))))))
              …full unroll for Abs-multiple…)))]
  ```
- **Behavior** Two regimes: (a) if any input is `Abs-multiple` (tuple/struct, i.e.
  heterogeneous known length) the fold is **fully unrolled** element-by-element. (b)
  Otherwise (aggregated array — `Abs-single`/`Abs-atomic`) the accumulator is iterated
  applying `fun`, stopping early when `abs-equal?` reports a **fixed point**, bounded by
  `len` iterations. `len = 0` ⇒ returns the seed `abs0` untouched.
- **Nature string** n/a.
- **Fixture** `fold_over_witness_array_reaches_fixpoint` (leak of the accumulator flagged).
- **Fail-closed** n/a (bounded by `len`).

### FX2 — `map`
- **Source anchor** `analysis-passes.ss:5402`
  ```scheme
  [(map ,src ,len ,fun ,[* abs] ,[* abs*] ...)
   (if (= len 0) (Abs-multiple '())
       (… if any input Abs-multiple ⇒ Abs-multiple of per-index (Function fun …)
          else ⇒ Abs-single (Function fun … over the aggregated element)))]
  ```
- **Behavior** `len = 0` ⇒ `Abs-multiple '()`. If any input is `Abs-multiple`, map
  produces an `Abs-multiple` with `fun` applied per index (full unroll). Otherwise
  (aggregated array) produces an `Abs-single` whose element is `fun` applied to the
  aggregated element abs. No fixpoint needed for map (each element independent).
- **Nature string** n/a.
- **Fixture** (optional) `map` over a witness array whose result is leaked.
- **Fail-closed** n/a.

### FX3 — `abs-equal?` (fixpoint equality)
- **Source anchor** `analysis-passes.ss:4978`
  ```scheme
  (define (abs-equal? abs1 abs2) …) ; same shape + same witnesses (uid + same path sets)
  ```
- **Behavior** Structural equality used by FX1's fixpoint test: same `Abs` shape and
  `same-witnesses?` (equal uid lists and equal path sets, via `same-paths?`). The
  `add-path-point` dedup (line 4847) ensures paths stop growing so this converges.
- **Nature string** n/a.
- **Fixture** (covered by FX1).
- **Fail-closed** n/a.

---

## Passthrough  (§3.4 leaf ops that preserve taint unchanged)

### P1 — `default<T>`
- **Source anchor** `analysis-passes.ss:5314` — `[(default ,src ,type) (default-value type)]`
- **Behavior** `default<T>` yields an untainted `Abs` shaped by `T` (empty witnesses).
- **Fixture** (implicit); **Fail-closed** Unknown type ⇒ advisory (AB1).

### P2 — Casts / conversions (identity passthrough)
- **Source anchor** `analysis-passes.ss:5512`
  ```scheme
  [(cast-from-enum …) abs]  [(cast-to-enum …) abs]  [(cast-from-bytes …) abs]
  [(field->bytes …) abs]    [(downcast-unsigned …) abs]  [(safe-cast …) abs]
  ```
- **Behavior** These casts pass the operand's `Abs` through UNCHANGED (taint preserved,
  no path-point). Casting does not sanitize.
- **Fixture** `hash_then_leak`-style with an intervening cast.
- **Fail-closed** n/a.

### P3 — Byte/vector reshaping casts (taint preserved, shape changed)
- **Source anchor** `analysis-passes.ss:5516`
  ```scheme
  [(bytes->vector ,src ,len ,[* abs]) (Abs-single (Abs-atomic (abs->witnesses abs)))]
  [(vector->bytes ,src ,len ,[* abs]) (Abs-atomic (abs->witnesses abs))]
  ```
- **Behavior** `bytes->vector` collapses all taint into a `Single(Atomic(all-witnesses))`;
  `vector->bytes` collapses to `Atomic(all-witnesses)`. Taint fully preserved (unioned),
  only the shape changes.
- **Fixture** (optional). **Fail-closed** n/a.

### P4 — `assert`, `enum-ref`, literals → untainted atomic
- **Source anchor** `analysis-passes.ss:5510` (`assert` → `Abs-atomic '()`), `5315`
  (`enum-ref` → `Abs-atomic '()`), `5308` (`quote` non-bool → `Abs-atomic '()`).
- **Behavior** `assert` returns untainted `Abs-atomic '()` (its subexpr is walked for
  effects but the result value is untainted); enum refs and non-boolean literals are
  untainted. **Note:** `assert`'s tested expression is still interpreted (control/effects
  observed) but does not by itself leak — `assert` is not a sink.
- **Fixture** n/a. **Fail-closed** n/a.

### P5 — Projection / construction (member, index, slice, tuple, struct `new`)
- **Source anchor** `elt-ref` `5329`, `tuple-ref` `5334`, `vector-ref` `5347`,
  `tuple-slice` `5359`, `bytes-ref` `5340`, `new` (struct) `5464`, `tuple` `5466`,
  `vector` `5482`, `var-ref` `5317`, `let*` `5504`, `seq` `5502`.
- **Behavior** Struct/tuple construction → `Abs-multiple` of the field abses; `new`
  (struct literal) → `Abs-multiple abs*`. Field/index projection pulls the corresponding
  child out of an `Abs-multiple`/`Abs-single`. Index/slice on a runtime index unions the
  index's witnesses into the selected element (`add-witnesses` with exposure
  `"the element selected by"` / `"the elements selected by"`). `var-ref` = env lookup.
  `let*` binds names to their abses (adding a `"the binding of <name>"` path-point via
  `add-path-binding`, line 4882) and walks the body.
- **Fixture** `comingled_return.compact` (struct field granularity).
- **Fail-closed** n/a.

---

## Fail-closed defaults  (§0 — load-bearing for the analyzer's fail-closed invariant)

These are the analyzer's OWN policy for partial/unknown surfaces (the compiler source is
total over its known IR; the native analyzer must not silent-green on anything it cannot
decide). Each MUST emit an **amber advisory** (`U3100`, `Severity::Advisory`), never
silent green, and excluded from the differential `native_discloses` verdict.

| Surface | Fail-closed behavior | Plan task |
|---|---|---|
| Unknown / `Unknown` v2b type in `abs_of_type` | advisory + `Abs::Atomic([seed])` (never silently untainted) | A2 |
| Unhandled expression kind | advisory + conservative `Abs::Atomic(union of all subexpr witnesses)` (never drop taint) | A3 |
| **Unknown native callee** | **conduit-taint ALL args into the result** + advisory | A6 |
| **Unknown ledger op** | **treat as a sink** (record leak) + advisory | A6 |
| Cross-contract `contract-call` sink (K3–K5) | deferred to v3b ⇒ advisory (NOT silent green, NOT a false-positive E-leak) | A8 |

---

## Cross-check: every `record-leak!` site is covered

`analysis-passes.ss` `record-leak!` call sites → rule: 5100 → **K8**; 5525 → **K2**;
5537 → **K1**; 5544 → **K3**; 5546 → **K4**; 5550 → **K5**; 5575 → **K6**; 5580 → **K7**.
(Definition at 5160; drain/render `get-leaks`+`complain` at 5164/5173.) No `emit` sink
exists in 0.31.1. This is the complete WPP verdict surface for compactc 0.31.1.
