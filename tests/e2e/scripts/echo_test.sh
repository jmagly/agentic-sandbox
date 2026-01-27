#!/bin/bash
# Deterministic test script: outputs to stdout and stderr, exits 0
echo "[STDOUT] test-output-marker-$$"
echo "[STDERR] test-error-marker-$$" >&2
exit 0
