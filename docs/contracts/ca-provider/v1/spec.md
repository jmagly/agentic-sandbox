# CA provider command protocol v1

This directory is the authoritative machine-readable contract for external
Agentic Sandbox CA provider executables. The normative Rust types are in
`management/src/grpc_ca_provider_protocol.rs`; `protocol.schema.json` and the
committed fixtures must remain byte-shape compatible with those types.

## Commands

| Command | Stdin | Stdout |
| --- | --- | --- |
| `describe` | empty | `#/$defs/describeResponse` |
| `health` | empty | `#/$defs/healthResponse` |
| `trust-bundle` | `#/$defs/trustBundleRequest` | `#/$defs/trustBundleResponse` |
| `sign` | `#/$defs/signRequest` | `#/$defs/signResponse` |

Schema references in the table resolve within `protocol.schema.json`.
Diagnostics go to stderr and are never part of the response contract.

## Compatibility

- `protocol.major` must equal `1`.
- The public core accepts a newer non-negative `protocol.minor` when the
  message still conforms to the v1 schema.
- Every v1 object is closed: unknown fields are rejected. A later minor
  revision can add a field only after the public schema and Rust type explicitly
  define it as optional.
- Unknown capability strings are ignored. Required capabilities are enforced
  by the core.
- The core bounds serialized requests to 1 MiB, responses to 4 MiB, and
  diagnostics to 64 KiB.
- `request_id`, trust-domain, SPIFFE ID, bundle revision, certificate chain,
  CSR key, validity, and requested TTL receive additional semantic validation
  in the public core. Schema success alone never authorizes issuance.

## Conformance

Validate the committed fixtures:

```bash
python3 scripts/check-grpc-ca-provider-contract.py
```

Validate fixtures emitted by another implementation:

```bash
python3 scripts/check-grpc-ca-provider-contract.py \
  --fixtures /absolute/path/to/adapter/fixtures
```

The directory must contain the same six filenames as `fixtures/`. The
validator performs strict shape, bound, correlation, and version checks using
only the Python standard library. It does not start an adapter.

The committed fixture certificates and CSR are deliberately synthetic wire
placeholders. They prove serialization compatibility, not X.509 validity.
End-to-end certificate semantics remain covered by the public Rust integration
suite and each adapter's isolated integration tests.
