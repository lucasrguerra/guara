# Arquitetura

[English](../architecture.md) · **Português**

O Guará é um workspace Rust com três partes: um núcleo (`guara-core`), um binário de terminal (`guara-cli`) e uma interface (`apps/gui`) que é a mesma página web servida de duas formas.

```
crates/guara-core   biblioteca: tudo que lê, interpreta e recupera
crates/guara-cli    binário `guara`: comandos e servidor local da interface
apps/gui/ui         HTML, CSS e JS da interface
apps/gui/src-tauri  casca desktop (Tauri) que carrega a mesma interface
```

## Fluxo de uma varredura

```
origem (dispositivo ou imagem)
   │  source / device         leitura somente leitura, setores ruins tolerados
   ▼
disco virtual                 (opcional)
   │  assemble                junta imagem E01/AFF4, RAID, LVM e volumes cifrados
   ▼
partições e volumes
   │  partition, volume       MBR, GPT, volumes perdidos por assinatura
   ▼
duas buscas em paralelo
   ├─ fs/*                    metadados: NTFS, FAT, exFAT, ext, HFS+, Btrfs, XFS, APFS
   └─ carve + signatures      estrutura dos arquivos no espaço livre e fora de partições
        + formats, jpegscan, defrag, mp4rebuild
   ▼
scan                          funde sem duplicatas e atribui a confiança
   ▼
sessão salva                  retomar, exportar, relatar
   ▼
recover                       copia com MD5, SHA-1 e SHA-256 e grava o manifesto
```

## Módulos do núcleo

### Acesso e montagem
| Módulo | Função |
|---|---|
| `source`, `device` | Abrir dispositivos e imagens somente para leitura; listar discos |
| `image` | Imagem bruta tolerante a setores ruins, com mapa de erros |
| `ewf`, `aff4` | Ler e gravar E01/Ex01 e AFF4 |
| `audit` | Trilha de auditoria encadeada por hash de cada ação do perito |
| `vdisk/` | Ler QCOW2, VMDK, VHD, VHDX e DMG como o disco que contêm |
| `assemble` | Monta o disco virtual: imagem, RAID, LVM e volume cifrado |
| `raid`, `raid_detect`, `lvm` | Remontagem de RAID (com descoberta automática de parâmetros) e LVM2 |
| `crypt/` | BitLocker, LUKS e FileVault 2 |
| `reconnect` | Reconhecer o mesmo disco que voltou com outro nome |

### Interpretação
| Módulo | Função |
|---|---|
| `partition`, `volume` | Tabelas de partição e detecção de volumes |
| `fs/` | Um leitor por sistema de arquivos |
| `signatures`, `formats` | Detectores de formato pela estrutura |
| `carve`, `scan` | Varredura por assinatura e fusão com os metadados |
| `jpegscan`, `defrag`, `mp4rebuild` | JPEG fragmentado e MP4 sem `moov` |
| `meta`, `sqlite`, `thumb` | Metadados, leitor de SQLite, miniaturas |

### Resultado e perícia
| Módulo | Função |
|---|---|
| `recover` | Copia os arquivos, confere os hashes, grava o manifesto, retoma recuperação |
| `hashing`, `search` | Hashes e busca de texto no disco inteiro |
| `report`, `explorer` | Relatório HTML, CSV, filtros e agrupamentos do explorador |
| `health`, `external` | SMART e ferramentas externas (`smartctl`) |

### Serviço
`service`, `http`, `settings`, `notify`, `owner`: a camada que a interface usa (API local, configurações, notificações, e gravação com o dono certo quando roda como administrador).

## Interface

A interface é uma página só (`apps/gui/ui`). Há duas formas de rodá-la:

- **App desktop (Tauri):** a janela carrega a página e fala com o núcleo diretamente.
- **`guara ui`:** o binário sobe um servidor em `127.0.0.1` protegido por token e abre o navegador.

Como o front-end é o mesmo, tudo que existe em um existe no outro. Veja como ele é em [interface.md](interface.md).

<p align="center"><img src="../img/explorer-light-pt.png" width="720" alt="O explorador do Guará"></p>

## Decisões de projeto

- **Somente leitura na origem, por construção.** Todo acesso a dispositivo passa por `source::open_path`. Revisão de código recusa qualquer escrita na origem.
- **Recusa de destino no mesmo disco** da origem (Linux e macOS), para não sobrescrever o que se quer recuperar.
- **Confiança por arquivo.** Cada detector declara o quanto conseguiu conferir; a fusão fica com a melhor evidência.
- **Leitores próprios.** Sistemas de arquivos, cifras, SQLite e contêineres de imagem são implementados no núcleo, para tolerar estruturas corrompidas, que é exatamente o caso de uso.
- **Segredos fora da sessão.** Senhas nunca são gravadas; no máximo a chave do volume, e só com `--remember-keys`.
- **Privilégios.** Ler disco bruto exige administrador, mas tudo que o Guará grava (sessões, relatórios, recuperados) fica com o dono e as permissões do usuário que o abriu.

## Testes

Cerca de cem testes, em `crates/guara-core/tests/` (um arquivo por área: `ext4`, `xfs`, `apfs`, `bitlocker`, `raid`, `ewf`, `jpegscan`, `mp4rebuild`, `sqlite` e outros).

- **Imagens geradas na hora** com `mtools`, `ntfs-3g`, `e2fsprogs` e afins (`scripts/make-fixtures.sh`).
- **Imagens de referência** de projetos externos (cryptsetup, dfvfs, AFF4) baixadas por `scripts/fetch-testdata.sh` para `target/testdata`. Sem elas, os testes correspondentes são pulados.
- Os testes de BitLocker levam cerca de 75 s por causa da derivação de chave.
- No CI os testes com imagens reais rodam só no Linux.
