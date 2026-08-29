#!/usr/bin/env bash
set -euo pipefail

version=3.3.1
expected_sha256=71031feb288e53c4c86c06eeb75df9cc94b7ceb1b927b45644d4f9d230c80734
download_url="https://dl.gitea.com/gitea-runner/${version}/gitea-runner-${version}-linux-amd64"
staging_file=$(mktemp)
trap 'rm -f "$staging_file"' EXIT

curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$staging_file" "$download_url"
printf '%s  %s\n' "$expected_sha256" "$staging_file" | sha256sum --check -
chmod 0755 "$staging_file"
"$staging_file" --version

sudo install -o root -g root -m 0755 "$staging_file" /usr/local/bin/gitea-runner
sudo install -o root -g root -m 0644 \
  deploy/systemd/gitea-act-runner-build01.service \
  /etc/systemd/system/gitea-act-runner-build01.service
sudo systemctl daemon-reload

echo "Installed Gitea Runner ${version}."
echo "Drain the runner, then restart gitea-act-runner-build01.service."
