#!/usr/bin/env bash
# Creates the self-signed code signing certificate the release workflow signs with.
# Run it once. Every release signed with it keeps one identity, so macOS keeps the
# Keychain access granted to xummary across updates. It does not satisfy Gatekeeper.
set -euo pipefail

NAME="${1:-Xummary Self-Signed}"
OUT="${XUMMARY_CERT_DIR:-$HOME/.config/xummary/signing}"

if [ -e "$OUT/xummary.p12" ]; then
    echo "$OUT/xummary.p12 already exists. A new key would give releases a new identity." >&2
    exit 1
fi

mkdir -p "$OUT"
chmod 700 "$OUT"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cat > "$work/cert.conf" <<CONF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
CONF

password="$(/usr/bin/openssl rand -base64 24)"

# /usr/bin/openssl is LibreSSL: its .p12 encryption is the one `security import` reads.
/usr/bin/openssl req -x509 -newkey rsa:3072 -nodes -days 7300 \
    -config "$work/cert.conf" -keyout "$work/key.pem" -out "$OUT/xummary.cer" 2>/dev/null
/usr/bin/openssl pkcs12 -export -name "$NAME" -passout "pass:$password" \
    -inkey "$work/key.pem" -in "$OUT/xummary.cer" -out "$OUT/xummary.p12"
printf '%s' "$password" > "$OUT/password"
chmod 600 "$OUT"/*

echo "created $OUT/xummary.p12 for \"$NAME\""
echo
echo "Store it as GitHub secrets:"
echo "  base64 -i \"$OUT/xummary.p12\" | gh secret set XUMMARY_CERT_P12_BASE64"
echo "  gh secret set XUMMARY_CERT_PASSWORD < \"$OUT/password\""
echo
echo "Keep a backup of $OUT: losing the key changes the identity once."
