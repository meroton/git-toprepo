#!/usr/bin/env bash

curl \
  -u admin:secret \
  -X POST \
  -H "Content-Type: text/plain" \
  -d "$(cat ~/.ssh/id_ed25519.pub)" \
  http://localhost:8080/a/accounts/admin/sshkeys
