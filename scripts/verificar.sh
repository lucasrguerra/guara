#!/usr/bin/env bash
# Verificação local: roda o mesmo que o CI roda, antes de você dar push.
#
#   scripts/verificar.sh            rápido (~1 min): formatação, clippy, tradução, scripts, workflows
#   scripts/verificar.sh --completo  inclui os testes do workspace e o cargo audit
#
# O CI leva de 5 a 30 minutos por execução e roda em 3 sistemas; quase tudo que o faz falhar
# aparece aqui em segundos. Se isto passa, o CI só deve revelar o que é próprio dele (Windows,
# macOS, imagens baixadas, apt do runner).
set -uo pipefail
cd "$(dirname "$0")/.."
completo=0
[ "${1:-}" = "--completo" ] && completo=1
falhas=()
passo() { # passo "nome" comando...
  local nome="$1"; shift
  printf '\n\033[1m== %s\033[0m\n' "$nome"
  if "$@"; then printf '\033[32mok\033[0m %s\n' "$nome"; else printf '\033[31mFALHOU\033[0m %s\n' "$nome"; falhas+=("$nome"); fi
}

passo "formatação (cargo fmt)" cargo fmt --all -- --check
# as mesmas flags do CI: avisos viram erro, inclusive nos testes
passo "clippy (workspace, todos os alvos)" env RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets
passo "cobertura da tradução da interface" node scripts/check-ui-i18n.mjs
passo "teste do latest.json" python3 -B scripts/test_make_latest_json.py
passo "sintaxe dos scripts bash" bash -c 'for f in scripts/*.sh .githooks/*; do bash -n "$f" || exit 1; done'
passo "workflows (YAML válido)" python3 -c "
import glob, yaml
for f in glob.glob('.github/workflows/*.yml'):
    yaml.safe_load(open(f))
"
if [ -f apps/gui/src-tauri/Cargo.toml ] && pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
  passo "app desktop (cargo check)" env RUSTFLAGS="" cargo check --manifest-path apps/gui/src-tauri/Cargo.toml
else
  printf '\n(app desktop: pulado, faltam as bibliotecas do WebKitGTK; o CI compila)\n'
fi

if [ "$completo" = 1 ]; then
  passo "testes (cargo test --workspace)" cargo test --workspace
  passo "auditoria de dependências (cargo audit)" cargo audit
fi

printf '\n'
if [ ${#falhas[@]} -eq 0 ]; then
  printf '\033[32mtudo certo\033[0m: pode dar push.\n'
else
  printf '\033[31mfalhou:\033[0m %s\n' "${falhas[*]}"
  exit 1
fi
