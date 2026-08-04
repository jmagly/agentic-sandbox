# Private AIWG Corpus

Agentic Sandbox's AIWG project artifacts and durable agent memory are
maintained outside the public product repository.

## Repository boundary

| Repository | Owns |
|---|---|
| `roctinam/agentic-sandbox` | Product source, public documentation, tests, packaging, and release automation |
| `roctinam/agentic-sandbox-aiwg` | Private requirements, architecture, plans, security artifacts, reports, runtime state, and agent memory |

The private artifact root is:

```text
../agentic-sandbox-aiwg/corpus/.aiwg
```

This assumes adjacent maintainer checkouts:

```text
~/dev/agentic-sandbox/
~/dev/agentic-sandbox-aiwg/
```

## Maintainer setup

Attach an existing private corpus non-destructively:

```bash
aiwg artifacts attach --to ../agentic-sandbox-aiwg/corpus/.aiwg
aiwg config show --project
```

The attach command validates the artifact root, writes the local ignored
`.aiwg-location` pointer, rebuilds the project index, and refreshes the Fortemi
Core cache. It does not copy or overwrite the private corpus.

For a temporary shell-specific override:

```bash
export AIWG_ARTIFACTS_PATH=../agentic-sandbox-aiwg/corpus/.aiwg
```

`AIWG_ARTIFACTS_PATH` points at the artifact directory itself and takes
precedence over `.aiwg-location`.

## Rules

- Do not recommit the private `.aiwg` corpus to the public product repository.
- Do not commit credentials, tokens, private keys, or environment files to
  either repository.
- Keep public operator and user documentation under `docs/` here.
- Use `aiwg artifacts attach` for an existing corpus and
  `aiwg artifacts move` only when relocating into a new or empty destination.
