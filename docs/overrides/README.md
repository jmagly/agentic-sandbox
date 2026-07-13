# Docs Theme Overrides (Pagenary)

Agentic Sandbox documentation is built with the Pagenary static publisher, but
the project re-skins it into a terminal-console experience. These files are the
override surface:

| File | Replaces / augments | Purpose |
|------|---------------------|---------|
| `index.html` | Pagenary's root shell | Custom app shell, header/footer, and project links |
| `styles.css` | Pagenary's stylesheet entirely | Full terminal theme plus compatibility vars (`--ink`, `--surface`, `--grid-line`, `--accent`, `--muted`, `--font-mono`) |
| `terminal.js` | Additive behavior | Wraps rendered sections in the `.log-entry` console pattern |

## Drift Risk

Because `styles.css` replaces Pagenary's stylesheet rather than layering on top
of it, any Pagenary UI component we do not explicitly re-style arrives
unstyled. Unstyled components can fall back to raw markup, such as an icon
button whose glyph renders as a bare text character or a popover that renders
inline in the content flow.

When bumping `@pagenary/publisher`, audit the built site for new or renamed
components and adapt them here. Pagenary component classes are namespaced
`.doc-*`, for example `.doc-content`, `.doc-fortemi-*`, `.doc-summary`, and
`.doc-meta`.

## Re-Styled Components

| Component | Classes | Notes |
|-----------|---------|-------|
| Fortemi page-metadata control | `.doc-fortemi-button`, `.doc-fortemi-tools`, `.doc-fortemi-panel`, `.doc-fortemi-chip`, `.doc-fortemi-row`, `.doc-fortemi-link` | Circular info icon anchored inside the reading column plus an overlay popover panel. It only appears on sections with Fortemi corpus metadata; absence on other pages is expected Pagenary behavior. |

If Pagenary renames any `.doc-fortemi-*` class, the control can revert to a
bare inline `i` until the theme is updated.
