#!/bin/bash
if [ -f /tmp/.capsule-git-identity ]; then
  . /tmp/.capsule-git-identity
fi
exec /usr/bin/git "$@"
