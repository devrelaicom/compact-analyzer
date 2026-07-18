# Disclosure advisories

This page explains the **amber "unverified" advisories** the analyzer attaches
to disclosure findings, and the contract they carry. It is the target of the
"learn more" link on every `U`-family advisory diagnostic.

## The advisory contract

**A clean editor is not a proof of privacy. The compiler is the authoritative
gate.**

compact-analyzer's disclosure analysis is a *live, in-editor approximation* of
what `compact compile` decides authoritatively. It exists to catch witness-value
disclosures early — while you type, before you compile — and to explain them
with a witness-to-sink trail. It does **not** replace the compiler: only
`compact compile` renders the binding verdict on whether a program discloses a
witness value. If the analyzer is silent but the compiler rejects your program,
the compiler is right.

## What an amber advisory (`U3100`) means

An amber `U3100` advisory marks a point the analyzer **could not fully decide**.
It is deliberately *neither* of the two confident verdicts:

- it is **not** a confirmed leak (that is a red `E3100` — see below), and
- it is **not** a statement that the point is safe.

It means: "the analysis reached a construct it does not model precisely here, so
it is declining to guess." Common triggers are an unresolved declared type, an
unmodeled expression or operator, a cross-module or cross-contract call the live
layer defers, or a native/callee it cannot resolve. When you see an advisory,
treat that point as **undecided** and lean on `compact compile` for the
authoritative answer.

## Fail-closed philosophy (§0)

The analysis is **fail-closed**: every construct it cannot decide precisely
becomes a visible amber advisory rather than a silent pass. It never silently
drops an uncertainty and reports green. This is a deliberate safety property —
the cost is occasional advisories on code the compiler ultimately accepts; the
benefit is that a genuinely undecided disclosure point is never hidden from you.
An advisory is a prompt to verify, not a defect in your code.

## Confirmed leaks (`E3100`) vs advisories (`U3100`)

- **`E3100` (red) — a confirmed finding.** The analyzer traced a witness value
  to a disclosing sink (a ledger write/operation, or a value returned from an
  exported circuit) with no `disclose(...)` on the path. These are the
  analyzer's *positive* results and mirror what the compiler will reject. Where
  the leak has a single wrappable expression, the analyzer offers a
  minimal-scope `disclose(...)` quick-fix that wraps exactly that expression.
- **`U3100` (amber) — an advisory.** Undecided, as described above. No quick-fix
  is offered (there is no confirmed leak to fix).

## Pinned compiler version

The disclosure rules — the sink table, the per-operation `disclose?` flags, the
native-conduit behaviour, and the differential fixtures — are extracted from and
validated against **`compact` compiler 0.31.1**. If your installed toolchain is
a different version, the compiler's disclosure semantics may have changed and
these advisories (and confirmed findings) may be stale relative to it. When the
analyzer's view and the compiler disagree, the compiler you are compiling with
is authoritative.
