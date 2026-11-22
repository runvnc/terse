#!/bin/sh
# Made the fixed certificates in this folder, for the tests of wss://
# relays only (their keys are public, so they prove nothing):
#   ca.pem                  a private CA (TERSE_RELAY_CA in the tests)
#   localhost.pem/.key      from it, for localhost, 127.0.0.1 and ::1
#   wrong.pem/.key          from it, for wrong.example only
#   untrusted.pem/.key      self-signed, for localhost and 127.0.0.1
# Valid for 100 years. To make them again: sh make.sh .
set -eu
out=$1
S=$(mktemp -d)
cd "$out"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$S/ca.key" -out ca.pem -days 36500 \
    -subj "/CN=Terse test CA (not secret, tests only)" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
for n in localhost wrong; do
    if [ $n = localhost ]; then san="DNS:localhost,IP:127.0.0.1,IP:::1"; else san="DNS:wrong.example"; fi
    openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$S/$n.key0" -out "$S/$n.csr" -subj "/CN=$n"
    openssl pkcs8 -topk8 -nocrypt -in "$S/$n.key0" -out $n.key
    printf "subjectAltName=$san\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n" >"$S/$n.ext"
    openssl x509 -req -in "$S/$n.csr" -CA ca.pem -CAkey "$S/ca.key" -CAserial "$S/ca.srl" -CAcreateserial -out $n.pem -days 36500 -extfile "$S/$n.ext"
done
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout "$S/untrusted.key0" -out untrusted.pem -days 36500 \
    -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1" \
    -addext "basicConstraints=critical,CA:FALSE" -addext "extendedKeyUsage=serverAuth"
openssl pkcs8 -topk8 -nocrypt -in "$S/untrusted.key0" -out untrusted.key
rm -rf "$S"
ls -la
