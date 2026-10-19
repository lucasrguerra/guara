<p align="center"><img src="apps/gui/ui/guara-logo.svg" width="360" alt="Guará"></p>
<h1 align="center">Guará</h1>
<p align="center">Recuperação de arquivos livre e gratuita para HD, SSD e pendrive.<br>Terminal e interface gráfica · Windows, Linux e macOS · somente leitura na origem.</p>

<p align="center"><a href="docs/suporte.md">O que suporta</a> · <a href="docs/cli.md">Terminal</a> · <a href="docs/arquitetura.md">Arquitetura</a> · <a href="ROADMAP.md">Roadmap</a> · <a href="CHANGELOG.md">Changelog</a></p>

---

O Guará junta, numa única varredura, o que ferramentas como Recuva, PhotoRec, TestDisk e Disk Drill fazem separadamente:

- **Metadados do sistema de arquivos:** acha arquivos apagados **com nome, pasta e data** em NTFS, FAT12/16/32, exFAT, ext2/3/4, HFS+, Btrfs, XFS e APFS.
- **Carving por estrutura** no espaço livre, em partições danificadas e no espaço não particionado. Os detectores leem a estrutura real de dezenas de formatos para saber o **tamanho exato** e a **confiança**.
- **Volumes cifrados:** abre BitLocker, LUKS e FileVault 2 com a sua senha ou chave e varre por dentro.
- **RAID e LVM:** remonta arrays a partir de discos soltos (RAID 0, 1, 4, 5, 6, 10, LVM2), descobrindo nível, faixa e ordem sozinho quando não há metadados.
- **Arquivos fragmentados:** remonta JPEG em pedaços e refaz o índice de vídeos MP4/MOV sem `moov`.
- **Imagens forenses:** lê e grava imagem bruta, **E01/Ex01** e **AFF4**.
- **Partições perdidas:** lê MBR/GPT e procura volumes por assinatura onde a tabela falhou.
- **Fusão sem duplicatas:** um arquivo achado por mais de um método aparece uma vez, com a melhor evidência.
- **Confiança por arquivo** (alta, média, baixa): estrutura íntegra, clusters reutilizados, conteúdo zerado (TRIM) ou que não confere com a extensão.

Matriz completa, com limites de cada item, em [docs/suporte.md](docs/suporte.md).

## Segurança dos dados

- A origem é aberta **somente para leitura**. Nada no núcleo escreve no dispositivo recuperado.
- A recuperação **recusa destinos no mesmo disco** da origem (Linux e macOS).
- **Imagem forense** tolerante a setores ruins, com mapa de erros e hash: trabalhe sempre na cópia de um disco com defeito.
- Cada recuperação grava `guara-manifesto.json` com **MD5, SHA-1 e SHA-256** de todos os arquivos.
- **Senhas nunca vão para o arquivo de sessão.**

## Instalação

Baixe o pacote do seu sistema em **[Releases](../../releases)**:

| | CLI | App gráfico |
|---|---|---|
| Windows | `guara-*-windows-x86_64.zip` | instalador `.msi` / `.exe` |
| macOS | `guara-*-macos-{aarch64,x86_64}.tar.gz` | `.dmg` |
| Linux | `guara-*-linux-{x86_64,aarch64}.tar.gz` | `.AppImage` / `.deb` |

Cada arquivo tem um `.sha256` ao lado. Compilar do código: `cargo build --release -p guara-cli` (Rust 1.80+).

> Os binários ainda **não são assinados**: o Windows SmartScreen e o Gatekeeper do macOS vão avisar. Confira o SHA-256.

Ler discos brutos exige privilégios: **administrador** (Windows), `sudo` (Linux e macOS). Mesmo como root, o Guará grava sessões, relatórios e arquivos recuperados com o dono e as permissões do usuário que o abriu.

## Começando

```bash
sudo guara devices                                  # que discos existem
sudo guara scan /dev/sdb --session s.json --list    # varredura profunda
guara recover s.json /mnt/outro-disco/Recuperados   # recupera, em OUTRO disco
```

Disco com defeito? Faça a imagem primeiro e trabalhe na cópia:

```bash
sudo guara image /dev/sdb /mnt/outro-disco/sdb.img
guara scan /mnt/outro-disco/sdb.img --session s.json
```

Outros comandos (`inspect`, `health`, `hash`, `search`, `export`, `report`, `verify`, `ui`, volumes cifrados, RAID) estão em **[docs/cli.md](docs/cli.md)**.

## Interface gráfica

App desktop (Tauri) no estilo de um gerenciador de arquivos, pensado para quando o Guará acha milhares de itens. O mesmo front-end roda no navegador com `guara ui`.

- **Grade de miniaturas** ou **lista**, agrupamento por dia ou mês e rolagem virtual para dezenas de milhares de arquivos.
- **Navegação lateral** por tipo, data real da foto (EXIF), pasta, câmera, confiança e GPS, com opção de ocultar duplicadas.
- **Seleção como no Finder ou Explorer** e **visualização rápida** em tela cheia (Espaço, Enter ou duplo clique).
- **Painel de detalhes** com pré-visualização, datas e metadados.
- Campos para senha, RAID, vídeo de referência e formato da imagem; saúde do disco (SMART); pausar e continuar; temas Auto, Claro e Escuro; interface em português e inglês.

**Pré-visualização antes de recuperar**, lida direto do disco sem gravar nada: imagens, vídeo, áudio, PDF, texto, Word/Excel/PowerPoint/OpenDocument, conteúdo de ZIP, **bancos SQLite** (com linhas apagadas) e hexadecimal. No Linux, a reprodução de vídeo depende dos plugins `gstreamer1.0-libav` e `gstreamer1.0-plugins-good`; sem eles, o botão "Abrir no player do sistema" cobre o caso.

## Recursos periciais

- **Hashes** MD5, SHA-1 e SHA-256 de cada arquivo recuperado, da imagem e do dispositivo.
- **Metadados:** EXIF (câmera, lente, número de série, GPS), PDF, Office e OpenDocument, MP4/MOV.
- **Linha do tempo:** datas de criação, modificação e acesso do NTFS, FAT e exFAT.
- **Busca de texto no disco inteiro**, inclusive espaço livre, em UTF-8 e UTF-16.
- **CSV** (abre no Excel) e **relatório HTML** imprimível com caso, examinador, origem, partições, resumo e hashes.
- **Sessões automáticas** e reconexão: o Guará reconhece o mesmo disco que volta com outro nome.
- **Detecção de duplicadas** por assinatura de conteúdo.

> **Atenção:** o Guará ainda **não foi validado externamente** (por exemplo, nos testes do NIST CFTT) e não tem registro de auditoria nem relatório assinado. Serve bem para recuperação e triagem; para uso em juízo, confira os resultados por outro meio. O caminho para isso está no [ROADMAP](ROADMAP.md).

## Limites conhecidos

- **Dados sobrescritos não voltam.** Em SSD com TRIM o disco pode zerar blocos apagados; o Guará avisa, mas não faz milagre.
- **Fragmentos:** só JPEG (até três pedaços) e MP4 sem índice são remontados. Os demais formatos, se fragmentados, saem truncados com confiança baixa.
- **ReFS e Storage Spaces** não são suportados (ReFS só é detectado).
- **APFS:** sem mapa de espaço livre. **XFS e APFS:** recuperação de exclusão testada só em simulação.
- **NTFS:** arquivos comprimidos, EFS e registros muito fragmentados são ignorados ou marcados como incompletos.
- **exFAT:** validado com imagem sintética; faltam mídias reais.
- **Celulares:** o Guará trabalha com mídias de armazenamento e **não** desbloqueia aparelhos nem contorna senhas.
- **Windows e macOS:** o código compila e é testado em CI, mas os testes com imagens reais rodam só no Linux. Relatos de mídias reais são muito bem-vindos.
- **GUI:** no Linux e macOS o app reinicia inteiro como administrador (`pkexec` ou `osascript`) em vez de usar um processo auxiliar.

## Para onde vai

Resumo do [ROADMAP](ROADMAP.md):

| | |
|---|---|
| **Feito** | Metadados de 8 sistemas de arquivos, BitLocker, LUKS, FileVault 2, RAID, LVM, E01, Ex01, AFF4, JPEG fragmentado, MP4 sem índice, leitor de SQLite, SMART |
| **Próximo** | Registro de auditoria, relatório assinado e em PDF, mapa livre do APFS, testes em Windows e macOS |
| **Depois** | NSRL e listas de hashes, busca em conteúdo, linha do tempo unificada, interpretação de navegadores, registro do Windows e apps de mensagem, ataque de dicionário e força bruta a volumes cifrados |
| **Sem imagem de referência** | ReFS e Storage Spaces: só entram com validação |
| **Longo prazo** | Backups de iOS e Android (iTunes, ADB), OCR, validação NIST CFTT, acesso físico a celulares por exploit de bootloader/chip |

## Desenvolvimento

```
crates/guara-core   núcleo: leitura, partições, sistemas de arquivos, cifra, RAID, carving, recuperação
crates/guara-cli    binário `guara` (CLI + servidor local da UI)
apps/gui            interface (ui/ = HTML, CSS e JS; src-tauri/ = app desktop)
scripts/            imagens de teste e dados de referência
docs/               suporte, terminal e arquitetura
```

```bash
cargo test --workspace        # no Linux gera imagens reais com mtools, ntfs-3g, e2fsprogs
scripts/fetch-testdata.sh     # opcional: imagens de referência (BitLocker, FileVault 2, dfvfs, AFF4)
cargo clippy --workspace --all-targets
```

Veja [CONTRIBUTING.md](CONTRIBUTING.md) e [docs/arquitetura.md](docs/arquitetura.md). Versões seguem [SemVer](https://semver.org/lang/pt-BR/); mudanças em [CHANGELOG.md](CHANGELOG.md). Para publicar: `git tag vX.Y.Z && git push --tags` (o workflow `release` compila e publica tudo). Vulnerabilidades: [SECURITY.md](SECURITY.md).

## Licença

[MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à sua escolha.
