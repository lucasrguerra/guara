# Contribuindo com o Guará

[English](CONTRIBUTING.md) · **Português**

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

Para as imagens de referência externas (BitLocker, FileVault 2, dfvfs, AFF4), rode `scripts/fetch-testdata.sh`; sem elas esses testes são pulados. O teste de BitLocker leva cerca de 75 s.

O app desktop assina os pacotes de atualização: o `tauri.conf.json` tem `createUpdaterArtifacts` ligado, então um `tauri build` local precisa de `TAURI_SIGNING_PRIVATE_KEY` (qualquer chave feita com `tauri signer generate`; a chave da release fica só nos segredos do repositório). Para um pacote local rápido, passe `--config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## Onde mexer

Mapa dos módulos em [docs/pt-BR/arquitetura.md](docs/pt-BR/arquitetura.md). O que falta e o que é bem-vindo está no [ROADMAP](ROADMAP.pt-BR.md).

## Adicionando um formato de arquivo

Detectores simples ficam em `crates/guara-core/src/signatures.rs`; os mais novos, em `formats.rs`. Acrescente o detector em `detect`, o primeiro byte em `might_start`, a extensão em `category_for_ext`/`matches_ext` e um arquivo real ao `scripts/make-fixtures.sh` com teste de bit-exatidão.

## Adicionando um sistema de arquivos

Crie o leitor em `crates/guara-core/src/fs/`, registre-o em `fs/mod.rs` e traga um teste com imagem real ou de referência. **Não entregamos leitor sem validação**: é por isso que ReFS e Storage Spaces ainda não existem. Imagens pequenas e sem dados pessoais são o melhor presente que se pode dar ao projeto.

## Documentação

A versão principal é em inglês e cada arquivo tem uma tradução em português (`*.pt-BR.md`, `docs/pt-BR/`). Sem travessões. Ao mudar um recurso, atualize as duas versões (inglês e português) de [docs/support.md](docs/support.md), do [CHANGELOG](CHANGELOG.md) e, se for o caso, do [ROADMAP](ROADMAP.md).

## Commits

Mensagens curtas no imperativo (`corrige leitura de runs esparsos no NTFS`). Uma mudança lógica por PR.
