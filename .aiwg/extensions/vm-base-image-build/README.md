# vm-base-image-build

Build/refresh the agentic-sandbox QEMU base image with the current agent-client, self-enrolling agent-client.service, virtio-vsock guest transport, and a kernel/libs/tools audit (ADR-023/026, #561).

## What this is

A project-local AIWG extension living under `.aiwg/extensions/vm-base-image-build/`.
Discovered automatically by `aiwg use` and deployed alongside upstream
artifacts.

## Layout

```
.aiwg/extensions/vm-base-image-build/
├── manifest.json          # Bundle metadata (validated by aiwg)
├── README.md              # This file
└── skills/ or rules/
```

## Usage

Deploy to your configured providers:
```bash
aiwg use vm-base-image-build
```

Inspect health:
```bash
aiwg doctor --project-local
```

Remove (preserves source under `.aiwg/`):
```bash
aiwg remove vm-base-image-build
```

## Identical-form portability

This directory is shaped **byte-identical** to upstream
`agentic/code/addons/vm-base-image-build/`. To graduate, run:

```bash
aiwg promote vm-base-image-build --dry-run     # preview
aiwg promote vm-base-image-build                # copy to upstream
aiwg promote vm-base-image-build --to corpus ~/my-corpus/   # or to a private corpus
```

Keep this directory shaped like upstream so `aiwg promote` works.

## Customization tips

- Edit `manifest.json` to set a real `description`, bump `version` to
  `1.0.0` when stable, and add platforms beyond `claude` if needed.
- Add new artifacts under `skills/`, `rules/`, `agents/`, or `commands/`
  per AIWG conventions.
- Use `@`-references for cross-artifact links: `@$AIWG_ROOT/...` for
  upstream paths, `@.aiwg/...` for project-local references (note: the
  latter will block promotion unless `--force` is passed).

## See also

- `docs/customization/project-local-quickstart.md` — first bundle in 5 minutes
- `docs/customization/project-local-lifecycle.md` — full lifecycle reference
- `docs/customization/extensions-vs-addons-vs-frameworks-vs-plugins.md` — pick the right type
