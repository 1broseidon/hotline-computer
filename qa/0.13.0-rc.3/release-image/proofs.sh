#!/bin/sh
# The release image's proofs: no probe-roots code, HOTLINE_KEYPROXY_PROBE_ROOT
# ignored, no test CA policy; the probe build of the same commit as the
# control. Only containers named hl-relcheck-* are started, and removed here.
set -u
CT=/usr/local/bin/container
SHA=$(cat ~/Projects/scratch/rc3-release/sha)
REPO=~/Projects/worktrees/hotline-computer/rc3-review-fixes
LOGS=$REPO/qa/0.13.0-rc.3/release-image
REL=hotline-computer:0.13.0-rc.3-relcheck
PROBE=$REL-probe
stamp() { date -u +%Y-%m-%dT%H:%M:%SZ; }
INSIDE='
b=/usr/bin/hotline-computer
echo "binary sha256: $(sha256sum $b | cut -d" " -f1)"
echo "revision $1 in the binary: $(grep -a -c "$1" $b) line(s)"
for s in HOTLINE_KEYPROXY_PROBE_ROOT "TEST BUILD: trusting" "probe root(s) upstream"; do
  echo "string \"$s\" in the binary: $(grep -a -c "$s" $b) line(s)"
done
echo "--- /etc/chromium/policies (every file):"
find /etc/chromium/policies /etc/opt/chrome /etc/chromium.d -type f 2>/dev/null | sort | while read f; do echo "$f: keys $(jq -c keys "$f" 2>/dev/null || echo "(not json)")"; done
echo "policy files naming CACertificates or CAHint: $(grep -rlE "CACertificates|CAPlatformIntegration|CACertificatesWithConstraints|CADistrustedCertificates|CAHintCertificates" /etc/chromium /etc/opt 2>/dev/null | wc -l)"
echo "--- system trust store:"
echo "/usr/local/share/ca-certificates: $(find /usr/local/share/ca-certificates -type f 2>/dev/null | wc -l) file(s)"
echo "certificates in /etc/ssl/certs/ca-certificates.crt: $(grep -c "BEGIN CERTIFICATE" /etc/ssl/certs/ca-certificates.crt)"
echo "test or fixture CAs in the bundle: $(openssl crl2pkcs7 -nocrl -certfile /etc/ssl/certs/ca-certificates.crt 2>/dev/null | openssl pkcs7 -print_certs -noout 2>/dev/null | grep -ciE "fixture|test only|hl-point1|hl-secrets|probe")" 2>/dev/null
echo "NSS databases on the image: $(find / -xdev \( -name cert9.db -o -name cert8.db \) 2>/dev/null | wc -l)"
echo "managed/recommended policy dirs:"; ls -la /etc/chromium/policies/managed /etc/chromium/policies/recommended 2>&1
'
for which in release probe; do
  img=$REL; [ $which = probe ] && img=$PROBE
  out=$LOGS/image-$which.txt
  {
    echo "# $(stamp) $img (commit $SHA)"
    $CT image inspect $img | jq -c '.[0] | {id: .id, created: .index?.annotations?, env: (.variants[0].config.config.Env // .variants?[0]?.config?.config?.Env)}' 2>/dev/null || $CT image inspect $img | head -c 2000
    echo "image env naming PROBE: $($CT image inspect $img | grep -c PROBE)"
    $CT run --rm --name hl-relcheck-look-$which --user 0 --entrypoint /bin/sh $img -c "$INSIDE" sh "$SHA" 2>&1
    echo "# exit $?"
  } > "$out" 2>&1
  echo "$(stamp) wrote $out"
done
