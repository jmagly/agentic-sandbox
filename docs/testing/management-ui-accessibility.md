# Management dashboard accessibility acceptance

This checklist is the manual WCAG 2.2 AA smoke lane for the management dashboard. Run it against the exact candidate commit after the deterministic `test-management-ui` gate passes. Record the commit, browser and version, assistive technology and version, viewport, result, and any issue links.

The shipped CSP is also a release contract: scripts are same-origin only, objects and framing are denied, form submissions are same-origin, and `unsafe-eval` is not allowed. `connect-src` permits same-origin HTTP/WebSocket traffic plus the documented loopback development endpoints. The deterministic test asserts these directives; the browser smoke injects hostile API/log/event metadata and fails if executable markup appears.

## Keyboard-only smoke

1. Start at the address bar, then use only Tab, Shift+Tab, Enter, Space, arrow keys, and Escape.
2. Traverse Console, Fleet, Celld, Config, and Access. Confirm that every actionable control has a visible focus indicator and an understandable accessible name.
3. Open create-instance, create-session, detail, confirmation, authorization, and deprecation dialogs. Confirm focus enters the dialog, Tab and Shift+Tab wrap inside it, Escape closes it, and focus returns to the invoking control.
4. Exercise tab sets and disclosure controls. Confirm arrow-key behavior where the control uses a composite widget, and normal Tab order where it uses ordinary buttons.
5. Enter a terminal. Confirm its interactive or read-only state is announced. Press Ctrl+Shift+Escape, activate **Leave terminal focus**, and confirm focus moves to the pane controls without sending input to the PTY.
6. Review destructive operations. Confirm review text identifies the target and effect, the apply control is unavailable before review, and an unknown result leads to reconciliation rather than an automatic replay.

## Screen-reader smoke

Use one supported pairing (NVDA + Firefox/Chrome on Windows, VoiceOver + Safari on macOS, or Orca + Firefox on Linux).

1. Confirm the page title, management navigation, headings, status regions, operation queue, and current workspace are discoverable by landmarks/headings.
2. Confirm connection, operation, audit-unavailable, rate-limit, and toast updates are announced once without repeatedly reading an entire live region.
3. Confirm icon-only controls announce an action and target, not a symbol or an unlabeled “button.”
4. Confirm each dialog announces its title and, for confirmations, its description.
5. Confirm credential and SSH views expose metadata only. Secret values, backend references, public keys, and certificate bodies must never be announced or available through page search, copy, or developer-visible DOM text.
6. Confirm terminal observer mode is announced as read-only and controller mode as interactive.

## Zoom, contrast, motion, and reflow

1. At 200% browser zoom and at a 320 CSS-pixel viewport, confirm content reflows without two-dimensional page scrolling; terminal content may retain its own viewport.
2. Check normal text, large text, controls, focus indicators, status chips, and error states against WCAG AA contrast requirements.
3. Enable the operating system’s reduced-motion preference. Confirm decorative animation and transitions are effectively disabled and no information depends on motion.
4. Confirm status is not conveyed by color alone: labels or text must distinguish ready, degraded, forbidden, stale, rate-limited, expired, revoked, and unknown states.

## Evidence record

Store the completed checklist with the release evidence and include: exact commit SHA, tester, date/time, browser/AT versions, pass/fail per section, screenshots only when they contain no credential or terminal secrets, and issue links for every failure. A failure blocks the accessibility gate; do not waive it without an explicit, documented release decision.
