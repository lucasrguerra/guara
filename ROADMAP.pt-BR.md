# Roadmap

[English](ROADMAP.md) · **Português**

Onde o Guará esteve, onde está e para onde vai. O que está marcado como feito existe no código e tem teste; o que está planejado é intenção, não promessa, e não tem data.

Legenda: ✅ feito · 🟡 parcial · 🔜 planejado · 🔭 ideia a longo prazo · ⛔ fora do escopo

## Princípios que não mudam

1. **A origem é somente leitura.** Nada no núcleo escreve no dispositivo recuperado.
2. **Não entregar sem validar.** Um recurso só entra com teste contra imagem real ou de referência. Por isso ReFS e Storage Spaces ainda não existem: não há imagem de referência para conferir.
3. **Confiança honesta.** Se o conteúdo não pôde ser conferido, a confiança é baixa ou média, nunca alta.
4. **Livre e auditável.** Código aberto (MIT ou Apache-2.0), sem telemetria, e cada resultado pode ser reproduzido.

## Histórico

| Versão | Data | Marco |
|---|---|---|
| 0.1.0 | 2026-10-18 | Núcleo somente leitura, MBR/GPT, NTFS, FAT, exFAT, carving por estrutura, imagem forense, sessões, CLI e interface |
| 0.2.0 | 2026-10-18 | Pré-visualização completa (imagem, vídeo, áudio, PDF, Office, ZIP, hexadecimal) e temas |
| 0.3.0 | 2026-10-18 | Explorador com miniaturas, metadados (EXIF, GPS), hashes, busca no disco, CSV, relatório HTML e duplicadas |
| 0.4.0 | 2026-10-18 | Reconexão de disco, sessões automáticas, organização ao recuperar |
| 0.5.0 | 2026-10-18 | Visualizador de SQLite com linhas apagadas, SMART, interface em inglês, retomada de recuperação |
| 0.6.0 | 2026-10-18 | Saúde do disco completa (SMART ATA e NVMe), `guara health`, nova logo |
| 1.0.0 | 2026-10-19 | ext4, HFS+, Btrfs, XFS, APFS; BitLocker, LUKS, FileVault 2; RAID e LVM; E01, Ex01, AFF4; JPEG fragmentado; MP4 sem moov |
| 1.0.1 | 2026-10-20 | Documentação em inglês com tradução em português, interface completa em inglês, telas, imagem Docker do terminal |
| 1.1.0 | 2026-10-20 | Mensagens do núcleo e datas em inglês, verificação automática da tradução, RustCrypto em digest 0.11, acesso a dispositivo no Docker e workspaces do Superset verificados |
| 1.2.0 | 2026-10-22 | Discos virtuais QCOW2, VMDK, VHD, VHDX e DMG, trilha de auditoria encadeada por hash, relatório em PDF, mapa de espaço livre do APFS |

Detalhes de cada versão no [CHANGELOG](CHANGELOG.pt-BR.md).

## Situação atual (1.2.0) contra o que se espera de uma suíte forense

### 1. Sistemas de arquivos com metadados

| Item | Estado | Observação |
|---|---|---|
| NTFS, FAT12/16/32, exFAT | ✅ | Desde a 0.1.0. NTFS ignora arquivos comprimidos e EFS |
| ext2/3/4 | ✅ | Inodes apagados e cópias antigas no diário |
| HFS+ | ✅ | Sobras de nós da árvore e diário |
| Btrfs | ✅ | Versões antigas das árvores; extensões zlib, zstd e lzo |
| XFS | 🟡 | Inodes liberados e nomes de entradas liberadas só funcionam na exclusão simulada. Com exclusão real do kernel (testada no CI) os metadados são zerados: o conteúdo volta por carving, o nome não. **Falta ler o diário** |
| APFS | 🟡 | Contêiner, mapa de objetos, mapa de espaço livre (spaceman e bitmaps), instantâneos e nós antigos. Exclusão testada só em simulação; contêineres com blocos de endereço de chunk-info (grandes) voltam a varrer o volume inteiro |
| ReFS | 🔜 | Hoje só é detectado; a recuperação depende do carving. Falta imagem de referência |
| Storage Spaces | 🔜 | Não feito. Falta imagem de referência |
| NTFS: atributos comprimidos, EFS, lista de atributos | 🔜 | Hoje marcados como incompletos |
| XFS: nomes e pastas após exclusão real (ler o diário) | 🔜 | O teste no CI existe e fica ignorado até isso entrar |
| Validação de APFS com exclusão real | 🔜 | Precisa de imagem de um Mac de verdade |

### 2. Volumes cifrados

| Item | Estado |
|---|---|
| BitLocker (senha, chave de recuperação, .BEK, chave limpa) | ✅ |
| LUKS1 e LUKS2 | ✅ |
| FileVault 2 (Core Storage) | ✅ |
| FileVault 2 em APFS com volume cifrado | 🔜 |
| Chave a partir de memória (hibernação, despejo de RAM) | 🔭 |
| Ataque de dicionário e força bruta (BitLocker, LUKS, FileVault, RAR, ZIP e demais volumes/arquivos cifrados) | 🔜 |

Hoje o Guará abre volumes quando você **já tem** a senha ou a chave. Recuperação de senha por dicionário e força bruta (com GPU, como Hashcat) é um recurso planejado, central no uso pericial.

### 3. Imagens forenses

| Item | Estado |
|---|---|
| Imagem bruta (dd) tolerante a setores ruins, com mapa de erros | ✅ |
| E01 e Ex01: leitura, gravação e `guara verify` | ✅ |
| AFF4: leitura e gravação | ✅ |
| Contêineres de disco virtual: QCOW2, VMDK, VHD, VHDX e DMG (leitura) | ✅ |
| QCOW2 com arquivo de apoio, snapshots de VMDK, DMG com LZFSE ou com senha | 🔜 |
| Imagem de dispositivo móvel (ver item 7) | 🔭 |

### 4. Montagem de discos soltos

| Item | Estado |
|---|---|
| RAID 0, 1, 4, 5, 6, 10 e linear (mdadm), inclusive degradado | ✅ |
| Descoberta automática de nível, faixa, layout e ordem sem metadados | ✅ |
| LVM2 (linear, em faixas, espelhado e RAID) | ✅ |
| Storage Spaces | 🔜 |
| Btrfs e ZFS multidisco (RAID do próprio sistema de arquivos) | 🔭 |
| RAID de controladoras proprietárias (Dell PERC, HP Smart Array) | 🔭 |

### 5. Carving

| Item | Estado |
|---|---|
| Detecção pela estrutura real, com tamanho exato e confiança | ✅ Desde a 0.1.0 (JPEG, PNG, GIF, BMP, PDF, ZIP e derivados, MP4/MOV/HEIC, AVI/WAV/WebP, SQLite, 7z, OLE, MP3, PE) e, na 1.0.0, mais 24 (gzip, zstd, Matroska/WebM, Ogg, MPEG-TS, FLV, ASF, MIDI, RAR, ar, SWF, ELF, TIFF, ICO, registro do Windows, EVTX, PST, RTF, pcap e outros) |
| JPEG fragmentado remontado (até três fragmentos) | ✅ |
| MP4/MOV sem `moov` reconstruído (H.264 e HEVC) a partir de um vídeo de referência | ✅ |
| Fragmentos: mais de três, e formatos além de JPEG (PNG, DOC, ZIP, SQLite) | 🔜 |
| Reconstrução de vídeo sem referência, áudio em MP4 sem `moov`, vídeos de drone e câmeras de ação em outros contêineres | 🔜 |
| Mais formatos validados por estrutura | 🔜 contínuo |

### 6. Interpretação de artefatos

O Guará **identifica e recupera** esses arquivos, mas só interpreta alguns.

| Item | Estado |
|---|---|
| Banco SQLite: esquema, tabelas, blobs, datas e linhas apagadas | ✅ |
| Metadados: EXIF e GPS, PDF, Office e OpenDocument, MP4/MOV | ✅ |
| Carving de registro do Windows (`regf`), EVTX, PST, pcap | 🟡 só reconhece e recupera, não interpreta |
| Interpretar o registro do Windows (usuários, programas, USB, histórico de execução) | 🔜 |
| Histórico, cookies e downloads de navegadores (Chrome, Firefox, Safari) | 🔜 base no leitor de SQLite |
| Mensagens, contatos e localização de apps (WhatsApp, Telegram, Signal e outros) | 🔜 base no leitor de SQLite |
| Logs do sistema e eventos do Windows (EVTX) | 🔜 |
| **Linha do tempo unificada** de todos os artefatos | 🔜 |

### 7. Extração de dispositivos móveis

Aqui o salto é de natureza, não de recurso. Hoje o Guará trabalha com **mídias de armazenamento**.

| Item | Estado |
|---|---|
| Android por ADB e backups | 🔭 |
| iOS por backups do iTunes ou Finder (inclusive cifrados com senha) | 🔭 |
| Acesso físico por exploits de bootloader e de chip | 🔭 |

O acesso físico é o diferencial de ferramentas comerciais como a Cellebrite. Depende de equipes de pesquisa de vulnerabilidades em tempo integral e muitas dessas técnicas não são publicáveis em código aberto, o que torna esse item o mais distante do roadmap. A direção mais próxima é ler o que o usuário ou o perito consegue exportar de forma legítima (backups e ADB) e interpretar esses dados com o item 6; o acesso físico por exploit fica como ideia de longo prazo, a perseguir conforme o projeto crescer.

### 8. Cadeia de custódia e validade em tribunal

| Item | Estado |
|---|---|
| MD5, SHA-1 e SHA-256 de cada arquivo, da imagem e do dispositivo | ✅ |
| Manifesto `guara-manifesto.json` em toda recuperação | ✅ |
| Relatório HTML com caso, examinador, origem, partições, resumo e hashes | ✅ |
| Verificação de imagem E01/Ex01 contra o hash gravado | ✅ |
| **Trilha de auditoria** de cada ação do perito (quem, quando, o quê), encadeada por hash, com `guara audit --verify` | ✅ |
| **Relatórios assinados** digitalmente e manifesto com carimbo de tempo | 🔜 |
| **Reprodutibilidade:** repetir o procedimento gravado e obter o mesmo resultado | 🔜 |
| Relatório em PDF (A4, próprio) | ✅ |
| **Validação externa:** testes do NIST CFTT (Computer Forensic Tool Testing) e publicação dos resultados | 🔭 |
| Binários assinados (Windows e macOS) | 🔜 |

A validação externa é o que faz um laudo ser aceito. Sem ela, o Guará serve para triagem, recuperação e apoio, e o perito deve conferir os resultados por outro meio.

### 9. Análise

| Item | Estado |
|---|---|
| Busca por texto em todo o disco, UTF-8 e UTF-16, inclusive espaço livre | ✅ `guara search` |
| Detecção de duplicadas | ✅ |
| Busca por expressão regular e por palavra-chave em **conteúdo de arquivos** (Office, PDF, e-mail) | 🔜 |
| Hashes conhecidos: **NSRL** para ignorar arquivos de sistema | 🔜 |
| Listas de hashes de material ilícito (importadas pelo perito) | 🔜 |
| Detecção de imagens por conteúdo (similaridade, classificação) | 🔭 |
| **OCR** em imagens e PDF digitalizados | 🔭 |

### 10. Plataforma e distribuição

| Item | Estado |
|---|---|
| CLI e interface para Windows, Linux e macOS | ✅ |
| CI em três sistemas, release automática por tag | ✅ |
| Testes com imagens reais em Windows e macOS (hoje só Linux) | 🔜 |
| GUI: processo auxiliar privilegiado em vez de reiniciar o app inteiro como administrador | 🔜 |
| Binários assinados e notarizados | 🔜 |
| Pacotes em gerenciadores (winget, Homebrew, Flatpak) | 🔭 |

## Próximos marcos sugeridos

Sem datas, na ordem em que fazem mais sentido.

**1.3: confiança e custódia**
Manifesto assinado, leitura do diário do XFS, validação de APFS com exclusões reais, testes com imagens reais no Windows e macOS.

**1.4: análise**
NSRL e listas de hashes, busca em conteúdo de arquivos, linha do tempo unificada.

**1.5: artefatos**
Registro do Windows, navegadores, EVTX e os apps de mensagem mais comuns, apoiados no leitor de SQLite.

**1.6: mais estrutura**
ReFS e Storage Spaces (quando houver imagem de referência), os casos de disco virtual ainda recusados (QCOW2 com arquivo de apoio, snapshots de VMDK, DMG com LZFSE ou senha), carving fragmentado ampliado.

**2.0: dispositivos móveis e validação**
Backups do iTunes, ADB e backups do Android; submissão aos testes do NIST CFTT.

## Como ajudar

O gargalo mais comum é **imagem de referência**: um volume ReFS, um pool de Storage Spaces ou um XFS com exclusão real, de preferência pequeno e sem dados pessoais. Se você tem um, abra uma issue. Veja também o [CONTRIBUTING](CONTRIBUTING.pt-BR.md).
