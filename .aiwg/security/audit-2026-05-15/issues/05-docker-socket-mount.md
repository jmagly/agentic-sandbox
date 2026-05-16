# [BLOCK] docker-compose.dev.yaml bind-mounts /var/run/docker.sock into mgmt container

**Labels**: `priority: critical`, `area: security`, `area: containers`, `type: incident`

## Summary

`docker-compose.dev.yaml:21` mounts the host's Docker socket into the management container:

```yaml
volumes:
  - /var/run/docker.sock:/var/run/docker.sock
```

Anyone with code execution inside the management container (or anyone who can exploit a vulnerability in management code) gets full root on the host via `docker run --privileged -v /:/host` from inside. This is the canonical container escape primitive (MITRE T1611).

While the compose file does set `cap_drop: ALL` + `no-new-privileges:true` + only `NET_ADMIN` added (good defense in depth), the docker.sock mount nullifies all of that — anyone in the container can spawn a *new* container with arbitrary capabilities.

## Impact

CRITICAL for dev environments that get used in CI or shared developer machines. The dev compose file is the easiest way to run the system, so this is the default exposure.

## Remediation

Pick one:

**Option A — Drop the socket mount entirely.** If management only needs Docker for the agent runtime, expose Docker via TCP socket with TLS + client cert auth, and configure `SANDBOX_MANAGER_DOCKER_HOST=tcp://docker:2376` with mounted certs. Reference: <https://docs.docker.com/engine/security/protect-access/>.

**Option B — Use a socket proxy.** Run [`linuxserver/docker-socket-proxy`](https://hub.docker.com/r/linuxserver/docker-socket-proxy) as a sidecar with only the API endpoints management actually uses (containers, images, networks) and DENY everything else (exec, run with privileges, host mounts). Management talks to the proxy instead of docker.sock.

**Option C — Rootless Docker.** If the dev environment can run rootless Docker, the impact is reduced (still bad — gives root-equivalent in the user's namespace — but no host root).

Update `runtimes/docker/docker-compose.yml` similarly if it has the same pattern (audit pending).

## References

- MITRE ATT&CK T1611 (Escape to Host)
- Docker security cheat sheet — "Don't expose the Docker daemon socket"
- Internal audit finding B5 (container review)
