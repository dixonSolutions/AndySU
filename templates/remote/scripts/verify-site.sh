#!/bin/sh
# Verify every channel of the local site exactly as a client would (signature, expiry, blobs).
set -eu
for m in site/*/update.json; do
  [ -e "$m" ] || continue
  andysu verify "$m" --pub andysu-public.pem --blobs
done
