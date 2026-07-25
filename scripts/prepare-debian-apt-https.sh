#!/bin/sh
# Require a trusted CA bundle, then move official Debian apt sources to HTTPS.
#
# Slim Debian images intentionally use HTTP for their initial package-index
# fetch because they may not contain ca-certificates. Callers using a slim
# runtime stage must copy the public /etc/ssl/certs/ trust store from an
# already pinned builder stage before invoking this script.
set -eu

root="${1:-/}"
case "$root" in
    /*) ;;
    *)
        echo "apt source root must be absolute" >&2
        exit 2
        ;;
esac

ca_bundle="${root%/}/etc/ssl/certs/ca-certificates.crt"
if [ ! -s "$ca_bundle" ]; then
    echo "trusted CA bundle is required before enabling Debian HTTPS sources" >&2
    exit 1
fi

found=0
for source in \
    "${root%/}/etc/apt/sources.list" \
    "${root%/}/etc/apt/sources.list.d/debian.sources"
do
    if [ ! -f "$source" ]; then
        continue
    fi
    found=1
    sed -i \
        -e 's|http://deb.debian.org|https://deb.debian.org|g' \
        -e 's|http://security.debian.org|https://security.debian.org|g' \
        "$source"
    if grep -Ev '^[[:space:]]*#' "$source" \
        | grep -Eq 'http://[^[:space:]]*debian\.org'
    then
        echo "unconverted Debian HTTP source remains in $source" >&2
        exit 1
    fi
done

if [ "$found" -ne 1 ]; then
    echo "no supported Debian apt source file found below $root" >&2
    exit 1
fi

apt_config_dir="${root%/}/etc/apt/apt.conf.d"
if [ ! -d "$apt_config_dir" ]; then
    echo "apt configuration directory is missing below $root" >&2
    exit 1
fi
cat > "${apt_config_dir}/99agentic-verified-https" <<'APT_CONFIG'
Acquire::https::CaInfo "/etc/ssl/certs/ca-certificates.crt";
Acquire::https::Verify-Peer "true";
Acquire::https::Verify-Host "true";
APT_CONFIG

echo "Debian apt sources require verified HTTPS"
