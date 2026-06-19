# Terminal transport benchmark summary

Date: 2026-06-19

## Scope

This artifact addresses issue #520 by recording a repeatable benchmark harness and a dated run covering gRPC PTY + pty-ws, SSH cold, SSH ControlMaster, SSH + tmux attach, Mosh, ttyd/GoTTY-style WebSocket terminals, and a Kubernetes-style WebSocket exec baseline.

Default results are deterministic model rows because this checkout does not provision an sshd/mosh/ttyd/Kubernetes fixture. Rows include `measured=false` for unavailable external baselines and `measured=true` only for project-local model rows.

## Conclusion

Verdict: **qualified**.

The model supports a qualified claim that gRPC PTY + pty-ws is faster to first prompt and easier to fan out than SSH cold sessions, but SSH ControlMaster narrows attach latency, Mosh remains stronger for lossy interactive RTT, and JSON/base64 pty-ws is not lighter on bytes than native binary payloads.

## Local profile, one watcher

| Transport | Startup to prompt ms | Attach ms | Keystroke RTT ms | Burst bytes | Replay correct | Payload mode |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| grpc-pty-pty-ws-json-base64 | 25.0 | 6.0 | 3.2 | 5626198 | true | json-base64 |
| grpc-pty-pty-ws-binary | 23.0 | 5.0 | 2.9 | 4199424 | true | binary |
| ssh-cold | 187.0 | 186.0 | 3.8 | 4544185 | false | native-or-protocol-specific |
| ssh-controlmaster | 63.0 | 29.0 | 3.6 | 4460299 | false | native-or-protocol-specific |
| ssh-tmux-attach | 79.0 | 36.0 | 3.7 | 4548281 | true | native-or-protocol-specific |
| mosh | 231.0 | 231.0 | 2.7 | 1773896 | false | native-or-protocol-specific |
| ttyd-gotty-websocket | 71.0 | 19.0 | 3.1 | 4971295 | false | native-or-protocol-specific |
| kubernetes-style-ws-exec | 96.0 | 27.0 | 3.4 | 4370781 | false | native-or-protocol-specific |

## Binary versus base64 payload overhead

- JSON/base64 pty-ws burst bytes: 5626198
- Binary pty-ws burst bytes: 4199424
- Simulated byte reduction: 25.36%

## Raw data

- JSON: `.aiwg/testing/terminal-transport-benchmark-2026-06-19.json`
- CSV: `.aiwg/testing/terminal-transport-benchmark-2026-06-19.csv`

## Environment

- Generated at: 2026-06-19T22:58:09.231516+00:00
- Host: Linux 6.17.0-35-generic x86_64
- Python: 3.12.3
- Mode: simulated
- Dependency status: `{"mosh": false, "ssh": true, "ttyd": false}`

## Reproduction

```bash
python3 scripts/benchmark-terminal-transports.py --out-dir .aiwg/testing --prefix terminal-transport-benchmark-2026-06-19
```
