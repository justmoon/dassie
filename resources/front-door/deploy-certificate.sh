#!/bin/sh
# certbot's deploy hook for a Dassie front door: copies the renewed
# certificate where the front door reads it, readable by its user. The
# front door reads the files again within a minute, and keeps serving the
# certificate before until the new one loads. Install it as
# /etc/letsencrypt/renewal-hooks/deploy/dassie-front-door.sh.
set -eu
TARGET=/etc/dassie-front-door/tls
install -d -m 0750 -o root -g dassie-front-door "$TARGET"
for file in fullchain.pem privkey.pem; do
  install -m 0640 -o root -g dassie-front-door "$RENEWED_LINEAGE/$file" "$TARGET/.$file.new"
  mv -f "$TARGET/.$file.new" "$TARGET/$file"
done
