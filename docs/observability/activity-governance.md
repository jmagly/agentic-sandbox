# Activity governance and integrity

`management/src/activity_governance.rs` defines the policy boundary around the
metadata activity store. The metadata endpoint independently accepts only
`activity.event/v1` records with `sensitivity=metadata`, authenticates the scope,
and recursively rejects fields associated with prompts, terminal input,
environment values, files, packets, credentials, cookies, keys, or authorization
data. Collectors also digest risky source values before spooling.

## Storage and authorization

Metadata and restricted content are separate authorization and encryption
domains. A restricted-content request must include an authenticated actor, case,
reason, exact tenant/instance/event scope, duration, retention, and automatic
expiry. Grants are actor-bound and limited to seven days. The content store
contract accepts only an encrypted envelope with a key identifier, nonce, and
ciphertext; it has no plaintext field.

Metadata readers can query standard metadata for their tenant. Security readers
can also query security-class metadata and verify evidence. Content custodians
need a matching live grant for each restricted read. Administrators can change
policy, create expiring holds, delete eligible records, and request cryptographic
erasure. Indexes, query results, and exports use the same exact tenant scope as
the source records. Cross-tenant records make an export fail closed.

Every query, content read, export, hold, policy change, verification, redaction
failure, deletion, and erasure attempt is auditable. Denied attempts are retained
as denied outcomes rather than disappearing from the audit trail.

## Retention and deletion

| Class | Default | Purpose |
|---|---:|---|
| standard | 30 days | routine operational metadata |
| security | 90 days | policy denials, loss, integrity, and security events |
| restricted | 7 days | separately encrypted, case-authorized content |
| ephemeral | 1 day | short-lived diagnostic metadata |

All defaults are configurable but must remain positive. Holds require an admin,
case, reason, exact scope, duration, and automatic expiry. Active matching holds
block deletion; unrelated tenants/instances and expired holds do not.

Cryptographic erasure destroys the content-encryption key and records the key ID,
actor, tenant, time, and outcome. It makes remaining ciphertext unrecoverable but
does not claim physical overwrite on snapshots, replicated media, SSDs, or an
external object store. Physical/non-overwrite guarantees are properties of the
configured storage backend and its retention controls.

## Integrity and key rotation

Each bounded export/batch hashes canonical JSON records into a Merkle root and
authenticates the manifest with HMAC-SHA256 using a key of at least 256 bits. The
signature covers batch, tenant, collector, count, root, previous root, and key ID.
Verification detects mutation, removal, wrong anchors, broken predecessor links,
and key-identity substitution.

Persist `AnchorCheckpoint` records in an append-only/WORM service outside the
workload and collector accounts. Rotate signing keys by issuing a new key ID and
retaining old verification keys until every governed retention/hold period has
expired. Never reuse a key ID for different material. HMAC provides shared-key
integrity, not public non-repudiation; deployments needing that property should
replace the signer behind the same manifest contract with an asymmetric KMS
signer and externally verifiable public key history.

## Verification

Run:

```sh
cd management
cargo test --lib activity_governance
cargo test --lib activity
```

Tests cover defaults/configuration, grant expiry and exact scope, recursive
secret-field rejection, tenant/sensitivity authorization, restricted-envelope
expiry, hold/deletion boundaries, audit-on-denial, governed export isolation,
manifest mutation/removal/anchor mismatch, chain continuity, and key identity.
