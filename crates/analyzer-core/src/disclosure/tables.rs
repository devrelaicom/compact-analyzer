//! The two compiler flag tables that drive call handling (spec §3.6/§3.8,
//! R0 index rows N1/N2 and L1/L2/L3), transcribed VERBATIM from the R0 index
//! (`docs/superpowers/plans/2026-07-16-v3-wpp-rules-index.md`, itself lifted
//! from `midnight-natives.ss` / `midnight-ledger.ss`, compactc-v0.31.1). A
//! mis-transcribed flag silently changes a leak verdict, so these are copied
//! flag-for-flag and cross-checked against the R0 rows.
//!
//! Two DISTINCT mechanisms — do not confuse them:
//!
//! - **N2 native conduit** ([`native_conduit`]): a native (built-in circuit) is
//!   a CONDUIT, not a sink — it records NO leak. Each arg flagged `Some(e)`
//!   flows its witnesses INTO the native's result with exposure `e`; each arg
//!   flagged `None` (`discloses nothing`) is HIDDEN (does not flow). The commit
//!   family (`transientCommit`/`persistentCommit`) hides both args ⇒ it is the
//!   analyzer's native sanitizer.
//!
//! - **L2/L3 ledger op** ([`ledger_op`]): a ledger ADT operation LEAKS every arg
//!   whose `discloses?` flag is truthy (nature `"ledger operation"`, exposure =
//!   the flag string). L1 LOAD-BEARING DEFAULT: an op with no `discloses` clause
//!   defaults to `""` which is TRUTHY ⇒ leaks; only an explicit
//!   `(discloses nothing)` (⇒ `#f`) is safe. Every L2 container op takes the
//!   default ⇒ [`LedgerOp::AllArgsLeak`]; only L3 kernel ops carry explicit
//!   per-arg clauses ([`LedgerOp::PerArg`], the sole `None` being
//!   `claimContractCall.comm`).

/// Per-argument flags, in declaration order. `Some(exposure)` = the arg's
/// witnesses flow / leak with this exposure string (a native conduit exposure,
/// or a ledger op's `discloses?` string); `None` = `nothing` (native: the arg
/// is hidden; ledger: `(discloses nothing)`, the arg does not leak).
pub type ArgFlags = &'static [Option<&'static str>];

/// The N2 native conduit table (16 rows, `midnight-natives.ss`): native name →
/// per-arg conduit flags. `Some(e)` folds the arg's witnesses into the result
/// with exposure `e`; `None` hides the arg. Result types (documented per row)
/// are all scalars or structs-of-scalars derived from these same args, so the
/// interpreter models the conduit result as a flat `Atomic` of the folded
/// witnesses — a sound over-approximation that never under-taints (see
/// `interp::native_conduit_result`). `None` for a name not in the table ⇒ the
/// caller applies the fail-closed unknown-native default (conduit-taint all
/// args + advisory).
pub fn native_conduit(name: &str) -> Option<ArgFlags> {
    // Flag strings are compiler message text — copy VERBATIM.
    Some(match name {
        // value → "a hash of"                                   ⇒ Field
        "transientHash" => &[Some("a hash of")],
        // value → nothing; rand → nothing (HIDES witness)       ⇒ Field
        "transientCommit" => &[None, None],
        // value → "a hash of"                                   ⇒ Bytes<32>
        "persistentHash" => &[Some("a hash of")],
        // value → nothing; rand → nothing (HIDES witness)       ⇒ Bytes<32>
        "persistentCommit" => &[None, None],
        // x → "a modulus of"                                    ⇒ Field
        "degradeToTransient" => &[Some("a modulus of")],
        // x → "a converted form of"                             ⇒ Bytes<32>
        "upgradeFromTransient" => &[Some("a converted form of")],
        // np → "the X coordinate of"                            ⇒ Field
        "jubjubPointX" => &[Some("the X coordinate of")],
        // np → "the Y coordinate of"                            ⇒ Field
        "jubjubPointY" => &[Some("the Y coordinate of")],
        // a,b → "an elliptic curve sum including"               ⇒ JubjubPoint
        "ecAdd" => &[
            Some("an elliptic curve sum including"),
            Some("an elliptic curve sum including"),
        ],
        // a,b → "an elliptic curve product including"           ⇒ JubjubPoint
        "ecMul" => &[
            Some("an elliptic curve product including"),
            Some("an elliptic curve product including"),
        ],
        // b → "the product of the embedded group generator with" ⇒ JubjubPoint
        "ecMulGenerator" => &[Some("the product of the embedded group generator with")],
        // value → "a hash of"                                   ⇒ JubjubPoint
        "hashToCurve" => &[Some("a hash of")],
        // x → "…x coordinate"; y → "…y coordinate"              ⇒ JubjubPoint
        "constructJubjubPoint" => &[
            Some("a JubjubPoint containing x coordinate"),
            Some("a JubjubPoint containing y coordinate"),
        ],
        // (no args)                                             ⇒ ZswapCoinPublicKey
        "ownPublicKey" => &[],
        // coin → nothing                                        ⇒ Void
        "createZswapInput" => &[None],
        // coin → nothing; recipient → nothing                   ⇒ Void
        "createZswapOutput" => &[None, None],
        _ => return None,
    })
}

/// A known ledger ADT operation's `discloses?` shape (L2/L3).
pub enum LedgerOp {
    /// L1/L2 default: EVERY argument leaks with exposure `""` (no `discloses`
    /// clause ⇒ `""` ⇒ truthy). Arity-agnostic — one entry covers every ADT
    /// that shares an op name (`insert`: Set/Map/MerkleTree; `member`/`remove`/
    /// `insertCoin`: Set/Map), each with its own arity, since every arg leaks
    /// identically.
    AllArgsLeak,
    /// An L3 Kernel op with an EXPLICIT `discloses` clause: per-arg flags
    /// (`Some(exposure)` leaks; `None` = `(discloses nothing)`, the only `None`
    /// across all L3 being `claimContractCall.comm`).
    PerArg(ArgFlags),
}

/// The L2 (container) + L3 (Kernel) ledger-op table: op name → its `discloses?`
/// shape. `None` for a name not in the table ⇒ the caller applies the
/// fail-closed unknown-ledger-op default (leak every witness-carrying arg +
/// advisory), which matches the L1 default (no clause ⇒ leaks).
pub fn ledger_op(name: &str) -> Option<LedgerOp> {
    // L3 Kernel exposures are compiler message text — copy VERBATIM.
    Some(match name {
        // --- L2 container ops (Cell/Counter/Set/Map/List/MerkleTree) ---
        // Every arg carries the default `discloses? = ""` ⇒ leaks.
        "write"          // Cell.write (also reached as `c = e` sugar in interp_stmt)
        | "writeCoin"    // Cell.writeCoin
        | "increment"    // Counter.increment
        | "decrement"    // Counter.decrement
        | "lessThan"     // Counter.lessThan (read)
        | "member"       // Set.member / Map.member (read)
        | "lookup"       // Map.lookup (read)
        | "insert"       // Set.insert / Map.insert / MerkleTree.insert
        | "insertDefault"// Map.insertDefault
        | "remove"       // Set.remove / Map.remove
        | "insertCoin"   // Set.insertCoin / Map.insertCoin
        | "pushFront"    // List.pushFront
        | "pushFrontCoin"// List.pushFrontCoin
        | "checkRoot" => LedgerOp::AllArgsLeak, // MerkleTree.checkRoot (read)

        // --- L3 Kernel ops (explicit `discloses` clauses) ---
        "claimZswapNullifier" => LedgerOp::PerArg(&[Some(
            "a link between a claim of nullifier and the coin with the nullifier given by",
        )]),
        "claimZswapCoinSpend" => LedgerOp::PerArg(&[Some(
            "a link between a coin spend and the coin with the commitment given by",
        )]),
        "claimZswapCoinReceive" => LedgerOp::PerArg(&[Some(
            "a link between a coin receive and the coin with the commitment given by",
        )]),
        // addr, entry_point → leak; comm → nothing (does NOT leak)
        "claimContractCall" => LedgerOp::PerArg(&[
            Some("the address of a contract being called given by"),
            Some("the hash of the contract's circuit being called given by"),
            None,
        ]),
        "mintShielded" => LedgerOp::PerArg(&[
            Some("the domain separator of the token being minted given by"),
            Some("the value of a token mint given by"),
        ]),
        "mintUnshielded" => LedgerOp::PerArg(&[
            Some("the domain separator of the unshielded token being minted given by"),
            Some("the amount of the unshielded token being minted given by"),
        ]),
        "claimUnshieldedCoinSpend" => LedgerOp::PerArg(&[
            Some("the type of the unshielded token being transferred given by"),
            Some("the recipient of the unshielded token being transferred given by"),
            Some("the amount of the unshielded token being transferred given by"),
        ]),
        "incUnshieldedOutputs" => LedgerOp::PerArg(&[
            Some("the type of the unshielded token being spent given by"),
            Some("the amount of the unshielded token being spent given by"),
        ]),
        "incUnshieldedInputs" => LedgerOp::PerArg(&[
            Some("the type of the unshielded token being received given by"),
            Some("the amount of the unshielded token being received given by"),
        ]),
        "balance" => LedgerOp::PerArg(&[Some(
            "the type of the unshielded token having its balanced checked given by",
        )]),
        "balanceLessThan" => LedgerOp::PerArg(&[
            Some("the type of the unshielded token having its balanced checked given by"),
            Some("the upper bound of the balance of the unshielded token being checked"),
        ]),
        "balanceGreaterThan" => LedgerOp::PerArg(&[
            Some("the type of the unshielded token having its balanced checked given by"),
            Some("the lower bound of the balance of the unshielded token being checked"),
        ]),
        "blockTimeLessThan" => LedgerOp::PerArg(&[Some("the lower bound of the time being checked")]),
        "blockTimeGreaterThan" => {
            LedgerOp::PerArg(&[Some("the upper bound of the time being checked")])
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sample_cross_check_against_r0() {
        // R0 N2: transientHash / persistentHash value → "a hash of".
        assert_eq!(
            native_conduit("transientHash"),
            Some(&[Some("a hash of")][..])
        );
        assert_eq!(
            native_conduit("persistentHash"),
            Some(&[Some("a hash of")][..])
        );
        // R0 N2: persistentCommit HIDES both args (the native sanitizer).
        assert_eq!(native_conduit("persistentCommit"), Some(&[None, None][..]));
        assert_eq!(native_conduit("transientCommit"), Some(&[None, None][..]));
        // A name not in the table ⇒ unknown (fail-closed at the call site).
        assert!(native_conduit("notANative").is_none());
    }

    #[test]
    fn ledger_sample_cross_check_against_r0() {
        // R0 L2 container op: every arg leaks (default `discloses? = ""`).
        assert!(matches!(ledger_op("insert"), Some(LedgerOp::AllArgsLeak)));
        assert!(matches!(
            ledger_op("increment"),
            Some(LedgerOp::AllArgsLeak)
        ));
        // R0 L3 Kernel op: claimContractCall's third arg (comm) is `nothing`.
        match ledger_op("claimContractCall") {
            Some(LedgerOp::PerArg(flags)) => {
                assert_eq!(flags.len(), 3);
                assert!(flags[0].is_some());
                assert!(flags[1].is_some());
                assert_eq!(flags[2], None, "claimContractCall.comm must be nothing");
            }
            other => panic!(
                "expected PerArg for claimContractCall, got {}",
                match other {
                    Some(LedgerOp::AllArgsLeak) => "AllArgsLeak",
                    None => "None",
                    _ => "?",
                }
            ),
        }
        // A name not in the table ⇒ unknown (fail-closed leak at the call site).
        assert!(ledger_op("notALedgerOp").is_none());
    }
}
