# O que o Guará suporta

[English](../support.md) · **Português**

Matriz do que existe hoje (versão 1.1.0). Para o que vem a seguir, veja o [ROADMAP](../../ROADMAP.pt-BR.md).

## Tabelas de partição

MBR e GPT. Quando a tabela falha, o Guará procura volumes por assinatura (NTFS, FAT, exFAT, ext, APFS, HFS+, BitLocker, LUKS) e também examina o **espaço não particionado**.

## Sistemas de arquivos

"Metadados" significa que o Guará lê a estrutura do sistema de arquivos e devolve **nome, pasta e datas**. Sem isso, a recuperação depende só do carving, que devolve o conteúdo sem nome.

| Sistema | Metadados | O que é aproveitado | Limites |
|---|---|---|---|
| NTFS | ✅ | MFT, registros apagados, datas de criação, modificação e acesso | Comprimidos e EFS são ignorados ou marcados como incompletos; lista de atributos tratada de forma parcial |
| FAT12/16/32 | ✅ | Entradas apagadas, nomes longos, datas | |
| exFAT | ✅ | Entradas apagadas, datas | Coberto por imagem sintética; falta validar em mais mídias reais |
| ext2/3/4 | ✅ | Inodes apagados, cópias antigas no diário (ext4), mapa de blocos livres | |
| HFS+ | ✅ | Sobras de nós da árvore de catálogo, diário | |
| Btrfs | ✅ | Versões antigas das árvores; extensões zlib, zstd e lzo | |
| XFS | ✅ | Inodes liberados, nomes de entradas liberadas, espaço livre por grupo | Exclusão testada só em simulação |
| APFS | 🟡 | Contêiner, mapa de objetos, árvore de arquivos, instantâneos, nós antigos | Sem mapa de espaço livre; exclusão testada só em simulação |
| ReFS | ❌ | Apenas detectado | Recuperação por carving |
| Storage Spaces | ❌ | Não suportado | |

Em todos os casos o carving continua rodando no espaço livre e o resultado é fundido sem duplicatas.

## Volumes cifrados

O Guará abre o volume com a **senha ou a chave que você fornece**, monta um disco virtual e varre por dentro. Ele não quebra senhas.

| Formato | Credenciais aceitas |
|---|---|
| BitLocker | Senha, chave de recuperação (8 grupos de 6 dígitos), arquivo `.BEK`, chave limpa |
| LUKS1 e LUKS2 | Senha, arquivo de chave, chave do volume |
| FileVault 2 (Core Storage) | Senha, chave de recuperação, chave do volume |

As senhas **nunca** vão para o arquivo de sessão. Com `--remember-keys` a sessão guarda a chave do volume já derivada, não a senha. No terminal, prefira `--ask-password` ou a variável `GUARA_PASSWORD` a `--password`, que fica no histórico do shell.

## RAID e LVM

| Item | Suporte |
|---|---|
| RAID 0, 1, 4, 5, 6, 10 e linear a partir de superblocos do mdadm | ✅ inclusive degradado (use `-` em `--member` para um disco ausente) |
| RAID sem metadados | ✅ `--raid` descobre nível, faixa, layout e ordem; `--raid-level`, `--raid-chunk-kb`, `--raid-layout` e `--raid-order` fixam o que você já sabe |
| LVM2 | ✅ linear, em faixas, espelhado e RAID |
| Storage Spaces | ❌ |

## Imagens forenses

| Formato | Ler | Gravar |
|---|---|---|
| Bruta (dd, `.img`) | ✅ | ✅ com mapa de erros de setores ruins |
| E01 e Ex01 (Expert Witness) | ✅ | ✅ com metadados do caso e segmentação |
| AFF4 | ✅ | ✅ |
| QCOW2 (QEMU), versões 2 e 3 | ✅ inclusive clusters comprimidos (deflate, zstd) | ❌ |
| VMDK (VMware, VirtualBox) | ✅ esparso, `streamOptimized` e descritores com extensões flat/zero | ❌ |
| VHD e VHDX (Hyper-V) | ✅ fixo, dinâmico e diferencial (segue a cadeia de pais) | ❌ |
| DMG (Apple UDIF) | ✅ chunks zerados, brutos, ADC, zlib e bzip2 | ❌ |

Os contêineres de disco virtual são abertos como o disco que contêm, sem conversão. Imagens que não podem ser lidas corretamente são recusadas com o motivo: QCOW2 com arquivo de apoio ou cifrado, VMDK com pai (snapshot) ou em SEsparse, DMG protegido por senha e DMG com chunks LZFSE. O log de transações do VHDX não é reproduzido (nada é gravado na imagem).

`guara verify` recalcula o MD5 de uma imagem E01 ou Ex01 e compara com o gravado nela.

## Carving: formatos reconhecidos pela estrutura

Os detectores leem cabeçalhos, blocos e tamanhos internos para devolver o **tamanho exato** e uma **confiança**, em vez de cortar em um número fixo de bytes.

| Família | Formatos |
|---|---|
| Imagem | JPEG, PNG, GIF, BMP, TIFF, ICO, HEIC, WebP |
| Documento | PDF, DOC/XLS/PPT (OLE), DOCX/XLSX/PPTX/ODF/EPUB (ZIP), RTF |
| Vídeo e áudio | MP4/MOV, AVI, WAV, MP3, Matroska/WebM, Ogg, MPEG-TS, FLV, ASF/WMV, MIDI |
| Compactado | ZIP, 7z, RAR, gzip, zstd, ar |
| Banco e sistema | SQLite, registro do Windows (regf), EVTX, PST, pcap/pcapng, EXE/DLL (PE), ELF, SWF |

### Arquivos fragmentados

- **JPEG:** remontado a partir de até três fragmentos (`jpegscan.rs`, `defrag.rs`).
- **MP4/MOV sem índice (`moov`):** o índice é refeito a partir de um vídeo íntegro da **mesma câmera ou drone**, passado com `--reference`. Funciona com H.264 e HEVC. O áudio não é reconstruído.
- Os demais formatos só saem inteiros se estiverem contíguos; do contrário saem truncados com confiança baixa.

## Metadados e pré-visualização

| Tipo | O que é lido |
|---|---|
| Fotos | EXIF: câmera, lente, número de série, data, GPS |
| PDF | Autor, programa, datas |
| Office e OpenDocument | Autor, última modificação por, empresa |
| MP4/MOV | Data de gravação, duração, resolução |
| SQLite | Esquema, tabelas, blobs, datas, linhas apagadas do espaço livre das páginas |

A pré-visualização é lida direto do disco, sem gravar nada: imagem, vídeo, áudio, PDF, texto, Office, ZIP e hexadecimal.

## Plataformas

| | CLI | Interface | Testes com imagens reais |
|---|---|---|---|
| Linux | ✅ | ✅ AppImage e deb | ✅ |
| Windows | ✅ | ✅ msi e exe | só compilação e testes sintéticos |
| macOS | ✅ | ✅ dmg | só compilação e testes sintéticos |

## O que o Guará não faz

- Recuperar dados **sobrescritos**. Em SSD com TRIM o próprio disco pode zerar blocos apagados.
- Desbloquear celulares ou contornar senhas e travas de segurança.
- Substituir uma ferramenta de extração de dispositivos móveis.
- Produzir laudo com validade pericial por si só: ainda não passou por validação externa (NIST CFTT). Veja o [ROADMAP](../../ROADMAP.pt-BR.md).
