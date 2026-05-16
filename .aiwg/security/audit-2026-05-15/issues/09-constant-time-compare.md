# [HIGH] Non-constant-time hash comparison in SecretStore::verify

**Labels**: `priority: high`, `area: security`, `area: crypto`, `type: maintenance`

## Summary

`management/src/auth.rs:95` compares stored vs computed SHA-256 hex hashes with Rust's `String` `==`, which short-circuits on the first byte mismatch:

```rust
if stored_hash == hash { /* ... */ }
```

The same pattern repeats at line 108 in the pending-rotation branch.

## Severity / nuance

Practical exploitability is **low**:

1. The compared values are SHA-256 hex of the submitted token, not the token itself — so a timing oracle reveals hash bytes, and the attacker would need to brute-force preimages (2^256 work) to weaponize.
2. Token entropy is 256 bits — even with a complete timing leak of the hash, brute-force is infeasible.

But the fix is one line, the `==` pattern is being copy-pasted to other verification sites in the codebase as new auth paths are added, and the `subtle` crate is already in the transitive dependency graph.

## Remediation

Add `subtle = "2"` as a direct dependency in `management/Cargo.toml` (currently transitive — pin it explicitly so a `cargo update` can't drop it).

Replace both `==` comparisons:

```rust
use subtle::ConstantTimeEq;

let stored_bytes = stored_hash.as_bytes();
let candidate_bytes = hash.as_bytes();
if stored_bytes.len() == candidate_bytes.len()
    && stored_bytes.ct_eq(candidate_bytes).into() {
    return true;
}
```

## Acceptance

- `grep -rn '== hash\|hash ==' management/src/auth.rs` returns nothing.
- `subtle` listed as direct dep in `management/Cargo.toml`.
- Unit test: two same-length but different-prefix hash strings take ~the same time to compare (microbenchmark, not strict timing assertion).

## References

- RFC 6234 §6 (constant-time MAC verification)
- `subtle` crate docs — `ConstantTimeEq`
- Bernstein, "Cache-timing attacks on AES" (2005)
- Internal audit finding H1 (applied-cryptographer)
