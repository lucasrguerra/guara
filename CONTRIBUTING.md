# Contribuindo com o Guará

Obrigado por ajudar! Regras de ouro:

1. **Nunca escreva na origem.** Qualquer código que abra um dispositivo deve usar `source::open_path` (somente leitura). PRs que escrevem no disco de origem são recusados.
2. **Teste com dados reais.** Novos detectores/leitores precisam de teste que recupera um arquivo conhecido bit a bit (veja `crates/guara-core/tests/` e `scripts/make-fixtures.sh`).
3. **Seja honesto sobre confiança.** Se o conteúdo não puder ser validado, a confiança é no máximo média.

## Fluxo

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

No Linux os testes geram imagens com `mtools`, `ntfs-3g`, `e2fsprogs`, `exfatprogs`, `imagemagick`, `zip` e `python3`. Sem essas ferramentas os testes com imagem são ignorados (defina `GUARA_REQUIRE_FIXTURES=1` para falhar).

## Adicionando um formato de arquivo

Edite `crates/guara-core/src/signatures.rs`: acrescente o detector em `detect`, o primeiro byte em `might_start`, a extensão em `category_for_ext`/`matches_ext` e um arquivo real ao `scripts/make-fixtures.sh` com teste de bit-exatidão.

## Commits

Mensagens curtas no imperativo (`corrige leitura de runs esparsos no NTFS`). Uma mudança lógica por PR.
