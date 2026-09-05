# Kelos v0.54.0 qualification experiment (#818)

## Purpose

Provide the bounded live experiment required by the
[research spike](kelos-runtime-spike-818.md). **Status: NOT_RUN.** Commands below
are a future execution plan, reviewed against the pinned source, not a record
of successful Kubernetes operations. The current environment has Docker
29.7.2, but no `kubectl`, `kind`, or `helm` on PATH, no prepared disposable
cluster for this experiment, and no supplied experiment-specific model account.
Docker availability alone does not establish these prerequisites.

The experiment exercises an actual coding-agent image. A credential-free
mock image could test Job plumbing but would not qualify model execution,
conversation persistence, or Gitea compatibility.

## System Topology

Use a dedicated disposable Kubernetes context named `kelos-spike-818`, with
Kubernetes 1.28+ (use a currently maintained release), cert-manager, a working
default StorageClass supporting ReadWriteOnce, and permitted egress to GHCR,
the model endpoint, and the public Git checkout. A working metrics API is
needed for `kubectl top`; without it resource measurements are NOT_RUN.

Supply a dedicated kubeconfig whose current context is `kelos-spike-818`,
matching v0.54.0 Kelos CLI and chart, Helm, kubectl, Python 3 with PyYAML,
and a least-privilege test model Secret named `kelos-spike-model` in namespace
`kelos-spike-818`, with key `ANTHROPIC_API_KEY`. Use the normal secret-management
path to provision it. No production account, Gitea write token, or repository
push permission is required for the core experiment.

Record exact Kubernetes, cert-manager, CNI, CSI, node-runtime, Helm, CLI, chart,
model, and image versions/digests. The API floor is not a compatibility claim
for every newer cluster. Pin image digests for subsequent repetitions after
recording the first resolution of the release tags.

## Procedure

This procedure is intentionally **not idempotent**: unique task names, Pod
replacement, Session deletion, and namespace teardown are test actions. Keep
one record per attempt and stop on a failed precondition.

### 1. Validate the dedicated context and install

Run from the Sandbox checkout. Every Kubernetes operation uses the explicit
context; the shell function is local to this experiment.

```bash
set -euo pipefail
export KELOS_SPIKE_CONTEXT=kelos-spike-818
export KELOS_SPIKE_NS=kelos-spike-818
: "${KELOS_SPIKE_KUBECONFIG:?Set the dedicated kubeconfig file path}"
test "$(kubectl --kubeconfig "$KELOS_SPIKE_KUBECONFIG" config current-context)" = "$KELOS_SPIKE_CONTEXT"
export KELOS_SPIKE_EVIDENCE="$(mktemp -d /tmp/kelos-818-evidence.XXXXXX)"
k() { kubectl --kubeconfig "$KELOS_SPIKE_KUBECONFIG" --context "$KELOS_SPIKE_CONTEXT" "$@"; }
k version -o yaml > "$KELOS_SPIKE_EVIDENCE/kubernetes-version.yaml"
k get nodes -o wide
k get storageclass
k -n cert-manager wait deployment --all --for=condition=Available --timeout=180s
helm version --short
kelos version
helm pull oci://ghcr.io/kelos-dev/charts/kelos --version 0.54.0 \
  --destination "$KELOS_SPIKE_EVIDENCE"
sha256sum "$KELOS_SPIKE_EVIDENCE/kelos-0.54.0.tgz"
helm upgrade --install kelos "$KELOS_SPIKE_EVIDENCE/kelos-0.54.0.tgz" \
  --kubeconfig "$KELOS_SPIKE_KUBECONFIG" --kube-context "$KELOS_SPIKE_CONTEXT" \
  -n kelos-system --create-namespace \
  --set crds.install=true --set image.tag=v0.54.0 \
  --set telemetry.enabled=false --wait --timeout=5m
k -n kelos-system rollout status deployment/kelos-controller-manager --timeout=180s
k get crd tasks.kelos.dev sessions.kelos.dev workspaces.kelos.dev
```

Expected: controller available and CRDs established. If the webhook/certificate
is not ready, stop and diagnose; do not disable conversion or validation.
Provision the namespace and test Secret before applying the workload below.
Check only Secret metadata, never print its contents:

```bash
k create namespace "$KELOS_SPIKE_NS"
# Provision kelos-spike-model through the normal secret-management path here.
k -n "$KELOS_SPIKE_NS" get secret kelos-spike-model -o name
```

### 2. Create a pinned Workspace and finite coding task

Save as `$KELOS_SPIKE_EVIDENCE/workloads.yaml`:

```yaml
apiVersion: kelos.dev/v1alpha2
kind: Workspace
metadata:
  name: spike-source
spec:
  repo: https://github.com/kelos-dev/kelos.git
  ref: 13e06522ae62fd8a4838174a09cb5827a65455d1
---
apiVersion: kelos.dev/v1alpha2
kind: Task
metadata:
  name: spike-task-1
spec:
  prompt: >-
    In /workspace/repo create spike_hello.py containing a function hello()
    that returns 'hello from kelos'. Run a Python assertion on that return
    value. Print the file content and its SHA256 to stdout with the prefix
    SPIKE_RESULT. Do not commit, push, open a PR, or modify other files.
  worker:
    type: claude-code
    image: ghcr.io/kelos-dev/claude-code:v0.54.0
    credentials:
      type: api-key
      secretRef:
        name: kelos-spike-model
    workspaceRef:
      name: spike-source
    podOverrides:
      activeDeadlineSeconds: 600
      resources:
        requests: {cpu: 250m, memory: 512Mi}
        limits: {cpu: '2', memory: 2Gi}
---
apiVersion: kelos.dev/v1alpha2
kind: Session
metadata:
  name: spike-session
spec:
  suspend: true
  initialPrompt: >-
    Create /workspace/repo/spike_session_marker.txt containing exactly
    kelos-818-persistent. If it already exists, leave it unchanged.
    Report its SHA256 and wait for another message. Do not commit or push.
  volumeClaimTemplate:
    accessModes: [ReadWriteOnce]
    resources:
      requests:
        storage: 2Gi
  worker:
    type: claude-code
    image: ghcr.io/kelos-dev/claude-code:v0.54.0
    credentials:
      type: api-key
      secretRef:
        name: kelos-spike-model
    workspaceRef:
      name: spike-source
    podOverrides:
      resources:
        requests: {cpu: 250m, memory: 512Mi}
        limits: {cpu: '2', memory: 2Gi}
```

Perform server-side schema/CEL validation, then submit and capture status/logs:

```bash
k -n "$KELOS_SPIKE_NS" apply --dry-run=server -f "$KELOS_SPIKE_EVIDENCE/workloads.yaml"
date -u +%FT%TZ > "$KELOS_SPIKE_EVIDENCE/task-submitted.txt"
k -n "$KELOS_SPIKE_NS" apply -f "$KELOS_SPIKE_EVIDENCE/workloads.yaml"
kelos --kubeconfig "$KELOS_SPIKE_KUBECONFIG" -n "$KELOS_SPIKE_NS" \
  logs spike-task-1 -f | tee "$KELOS_SPIKE_EVIDENCE/task.log"
k -n "$KELOS_SPIKE_NS" wait task/spike-task-1 \
  --for=jsonpath='{.status.phase}'=Succeeded --timeout=660s
k -n "$KELOS_SPIKE_NS" get task spike-task-1 -o json > "$KELOS_SPIKE_EVIDENCE/task.json"
k -n "$KELOS_SPIKE_NS" get pods -l kelos.dev/task=spike-task-1 -o json \
  > "$KELOS_SPIKE_EVIDENCE/task-pods.json"
```

Expected: a Succeeded Task, a successful assertion, and a SPIKE_RESULT artifact
in the log. Review the generated code, not just the phase. While it runs,
record `k -n "$KELOS_SPIKE_NS" top pod --containers` every five seconds in a
second shell with the same context variables. Capture helper-container usage
and resolved `status.containerStatuses[].imageID`/init-container image IDs.

### 3. Cancel, delete, and check residue

Generate a second Task from the same manifest with Python/PyYAML, name
`spike-cancel`, and replace only its prompt with: "Run sleep 300 as a foreground
shell tool, then print SPIKE_UNEXPECTED_COMPLETION. Do not modify files."
Create and validate the variant:

```bash
python3 - <<'PYTHON'
import os, pathlib, yaml
root = pathlib.Path(os.environ['KELOS_SPIKE_EVIDENCE'])
task = next(x for x in yaml.safe_load_all((root/'workloads.yaml').read_text())
            if x['kind'] == 'Task')
task['metadata']['name'] = 'spike-cancel'
task['spec']['prompt'] = ('Run sleep 300 as a foreground shell tool, then print '
                          'SPIKE_UNEXPECTED_COMPLETION. Do not modify files.')
(root/'cancel.yaml').write_text(yaml.safe_dump(task))
PYTHON
k -n "$KELOS_SPIKE_NS" apply --dry-run=server -f "$KELOS_SPIKE_EVIDENCE/cancel.yaml"
k -n "$KELOS_SPIKE_NS" apply -f "$KELOS_SPIKE_EVIDENCE/cancel.yaml"
k -n "$KELOS_SPIKE_NS" wait task/spike-cancel \
  --for=jsonpath='{.status.phase}'=Running --timeout=180s
```

Confirm its log says the sleep tool has begun, then:

```bash
k -n "$KELOS_SPIKE_NS" delete task spike-cancel --wait=true --timeout=120s
k -n "$KELOS_SPIKE_NS" wait pod -l kelos.dev/task=spike-cancel \
  --for=delete --timeout=120s
k -n "$KELOS_SPIKE_NS" get jobs,pods -l kelos.dev/task=spike-cancel -o name
```

Expected: no remaining Job/Pod; record cancellation elapsed time and any
finalizer delay. Do not infer success from deletion acceptance alone. Retain
logs before deletion; no native Cancelled Task status survives CR deletion.

### 4. Verify Session persistence and infrastructure readiness

```bash
k -n "$KELOS_SPIKE_NS" patch session spike-session --type=merge \
  -p '{"spec":{"suspend":false}}'
k -n "$KELOS_SPIKE_NS" wait session/spike-session \
  --for=condition=Ready --timeout=300s
kelos --kubeconfig "$KELOS_SPIKE_KUBECONFIG" -n "$KELOS_SPIKE_NS" \
  session connect spike-session
```

In terminal chat, wait for the initial turn to finish, then ask it to read the
marker and report the prior turn. Record the conversation without account data.
Disconnect the client normally. Independently capture the marker checksum:

```bash
KELOS_SPIKE_POD=$(k -n "$KELOS_SPIKE_NS" get session spike-session \
  -o jsonpath='{.status.podName}')
k -n "$KELOS_SPIKE_NS" get pod "$KELOS_SPIKE_POD" \
  -o jsonpath='{.metadata.uid}' > "$KELOS_SPIKE_EVIDENCE/session-pod-uid-before.txt"
k -n "$KELOS_SPIKE_NS" exec "$KELOS_SPIKE_POD" -c kelos-agent -- \
  sha256sum /workspace/repo/spike_session_marker.txt \
  > "$KELOS_SPIKE_EVIDENCE/marker-before.txt"
k -n "$KELOS_SPIKE_NS" delete pod "$KELOS_SPIKE_POD" --wait=true --timeout=120s
```

Poll for replacement with a five-minute bound:

```bash
KELOS_SPIKE_OLD_UID=$(cat "$KELOS_SPIKE_EVIDENCE/session-pod-uid-before.txt")
KELOS_SPIKE_NEW_UID=''
for attempt in $(seq 1 60); do
  KELOS_SPIKE_NEW_UID=$(k -n "$KELOS_SPIKE_NS" get session spike-session -o jsonpath='{.status.podUID}')
  if test -n "$KELOS_SPIKE_NEW_UID" && test "$KELOS_SPIKE_NEW_UID" != "$KELOS_SPIKE_OLD_UID"; then break; fi
  sleep 5
done
test -n "$KELOS_SPIKE_NEW_UID"
test "$KELOS_SPIKE_NEW_UID" != "$KELOS_SPIKE_OLD_UID"
k -n "$KELOS_SPIKE_NS" wait session/spike-session --for=condition=Ready --timeout=300s
KELOS_SPIKE_POD=$(k -n "$KELOS_SPIKE_NS" get session spike-session -o jsonpath='{.status.podName}')
k -n "$KELOS_SPIKE_NS" exec "$KELOS_SPIKE_POD" -c kelos-agent -- \
  sha256sum /workspace/repo/spike_session_marker.txt > "$KELOS_SPIKE_EVIDENCE/marker-after.txt"
cmp "$KELOS_SPIKE_EVIDENCE/marker-before.txt" "$KELOS_SPIKE_EVIDENCE/marker-after.txt"
```

Reconnect after the UID check and Ready observation. Merely observing a stale Ready condition
is insufficient. Compare the marker checksum byte-for-byte, confirm history is
retained, and inspect whether the initial prompt was replayed. Repeat with
Session suspend true (wait Suspended) and false (wait Ready). Record each Pod
UID, PVC UID, status transition, and reconnect result.

Restart the controller in this disposable cluster:

```bash
k -n kelos-system rollout restart deployment/kelos-controller-manager
k -n kelos-system rollout status deployment/kelos-controller-manager --timeout=180s
k -n "$KELOS_SPIKE_NS" get task,session,pods,pvc -o wide
```

Expected: no duplicate Task/Job; existing Session data remains. To test the
negative case, create a separate Session from the fixture with a unique name
and **no** volumeClaimTemplate. Delete its Pod after a completed initial turn;
record lost history/replayed initial prompt. It must not be counted as durable.

### 5. Measure repeat effects and management recovery

Use a separate Task with a result PVC mounted at `/results`. Its Workspace
setupCommand is `["sh", "-c", "echo attempt >> /results/attempts; exit 1"]`.
Observe the Job through terminal failure and count the persisted lines using a
separate reader Pod mounting that PVC. Record Job retries, Pod UIDs, and counter
value. Source inspection predicts up to two attempts with backoffLimit 1;
this test determines the observed behavior. A stable Task UID does not prove a
single workload side effect. Keep this deterministic fault separate from model
behavior and from the successful coding task.

There is currently no Sandbox Kelos adapter to restart. Mark management-restart
qualification **NOT_IMPLEMENTED / NOT_RUN**. Once an adapter exists, interrupt
it after Kubernetes accepts a create but before its response is recorded;
restart it, resend the same operation ID/hash, and require one CR UID and one
provider submission. Also test changed hash rejection, stale Sandbox generation,
and a deleted/recreated CR with the same name. Do not equate controller restart
with management-ledger recovery.

### 6. Collect comparison results and clean up

Use ten serial Tasks with unique names, then four concurrent Tasks. Report
submission→container-start, submission→first-output, and total completion time
separately, with median/p95, image-cache state, model, tokens/cost when available,
and failures. Distinguish controller idle usage, idle Session usage, and active
workload usage. Use the same node/image/workload/cache conditions when comparing
[Sandbox runtimes](../runtime-benchmark.md); do not compare model response time
to VM boot time. WorkerPool latency/concurrency requires a separate pool with
matching image/workspace and PVC; report NOT_RUN until measured.

Before teardown, capture resource identities and redacted diagnostics. Delete
only this experiment's namespace while Kelos is still running:

```bash
k -n "$KELOS_SPIKE_NS" get task,session,pods,jobs,statefulsets,pvc -o name \
  > "$KELOS_SPIKE_EVIDENCE/resources-before-delete.txt"
k delete namespace "$KELOS_SPIKE_NS" --wait=true --timeout=300s
k get namespace "$KELOS_SPIKE_NS" --ignore-not-found -o name
k get pv -o json > "$KELOS_SPIKE_EVIDENCE/pv-after-delete.json"
```

Expected: namespace absent. Inspect PV claimRef/reclaim policy for the test
namespace; retained PV data is a documented residue, not successful secure
erasure. Do not delete unrelated PVs. Uninstall Kelos only in the dedicated
cluster after custom resources/finalizers are gone, then retire the disposable
cluster through its provisioning tool. Keep failure evidence if cleanup stalls.

## Verification

| Case | Required observation | Current result |
| --- | --- | --- |
| Schema admission | Server accepts pinned fixture and rejects invalid variants | NOT_RUN |
| Coding Task | Succeeded, asserted code, visible logs/artifact | NOT_RUN |
| Cancellation | Running process ends; owned Job/Pod disappear | NOT_RUN |
| Persistent Session | New Pod UID, same PVC/data/history, no unintended initial replay | NOT_RUN |
| Ephemeral negative control | History loss/replay accurately recorded | NOT_RUN |
| Controller restart | No extra Job or lost persistent workspace | NOT_RUN |
| Job failure side effects | Counter and Pod UIDs expose repeat attempts | NOT_RUN |
| Management restart | Adapter ledger reconciles accepted create without duplicate submission | NOT_IMPLEMENTED / NOT_RUN |
| Resource/latency comparison | Samples, boundaries, image digests, failures and cache state recorded | NOT_RUN |
| Gitea | Separate test repo: HTTPS PAT checkout plus Gitea result API; no GitHub API fallback | NOT_RUN |
| Cleanup | No test workloads; retained PVs identified explicitly | NOT_RUN |

## Troubleshooting

- Pending PVC: record StorageClass/events; do not switch to emptyDir and claim
  persistent success.
- ImagePullBackOff: record the requested immutable version and registry error;
  do not substitute `latest`.
- Ready with no successful turn: inspect model access and runtime logs; Ready
  only proves infrastructure readiness.
- Deletion timeout: inspect ownerReferences, finalizers, controller availability,
  and PVC reclaim policy. Preserve evidence; do not force-remove finalizers.
- Missing metrics or cluster resources: mark the specific case NOT_RUN.

## House Rules for Agents

Use the explicit disposable context for every action. Keep prompts, tokens,
source contents, and history out of public diagnostic uploads. Only sanitized
status, hashes of test artifacts, timings, and version metadata belong in the
issue. The experiment does not authorize a production runtime rollout.

## What NOT to Fix

Do not modify production clusters, Sandbox runtime defaults, existing workload
storage, CA enrollment, host network rules, or unrelated cluster resources.
Do not weaken validation or change the tested version to get a passing result.

## Audit Trail

The research report records the immutable upstream commit and Sandbox baseline.
Store commands, exit codes, timestamps, rendered manifests, image IDs, resource
UIDs, and per-case outcomes with the experiment record. This document records
only a reviewed plan; replace NOT_RUN only with actual observations and link the
resulting evidence from issue #818 or the subsequent qualification issue.
