#!/usr/bin/env bash
# Liga o hook de pre-push do repositório (roda scripts/verificar.sh antes de cada push).
set -euo pipefail
cd "$(dirname "$0")/.."
git config core.hooksPath .githooks
echo "hook ligado: git push passa a rodar scripts/verificar.sh (desligue com: git config --unset core.hooksPath)"
