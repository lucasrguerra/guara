<p align="center"><img src="apps/gui/ui/guara-logo.svg" width="360" alt="Guará"></p>
<h1 align="center">Guará</h1>
<p align="center">Recuperação de arquivos livre e gratuita para HD, SSD e pendrive.<br>Terminal e interface gráfica · Windows, Linux e macOS · somente leitura na origem.</p>

---

O Guará junta, numa única varredura, o que ferramentas como Recuva, PhotoRec, TestDisk e Disk Drill fazem separadamente:

- **Metadados do sistema de arquivos** (NTFS, FAT12/16/32, exFAT): acha arquivos apagados **com nome, pasta e data**.
- **Carving por assinatura/estrutura** no espaço livre, em partições danificadas e no **espaço não particionado**: acha arquivos mesmo quando o sistema de arquivos sumiu. Os detectores leem a estrutura real (JPEG, PNG, GIF, BMP, PDF, ZIP/DOCX/XLSX/PPTX/ODF/EPUB, MP4/MOV/HEIC, AVI/WAV/WebP, SQLite, 7z, OLE (DOC/XLS/PPT), MP3, EXE/DLL) para saber o **tamanho exato** e a **confiança**.
- **Partições perdidas:** lê MBR/GPT e procura volumes (NTFS, FAT, exFAT, ext, APFS, HFS+, BitLocker, LUKS) por assinatura onde a tabela falhou.
- **Fusão sem duplicatas:** um arquivo achado pelos dois métodos aparece uma vez, com a melhor evidência.
- **Confiança por arquivo** (alta / média / baixa): estrutura íntegra, clusters reutilizados, conteúdo zerado (TRIM) ou que não confere com a extensão.

## Segurança dos dados

- A origem é aberta **somente para leitura**. Nada no núcleo escreve no dispositivo recuperado.
- A recuperação **recusa destinos no mesmo disco** da origem (Linux/macOS).
- **Imagem forense** (`guara image`) tolerante a setores ruins, com mapa de erros e SHA-256: trabalhe sempre na cópia de um disco com defeito.
- Cada recuperação grava `guara-manifesto.json` com **MD5, SHA-1 e SHA-256** de todos os arquivos.
- **Volumes cifrados** (BitLocker, LUKS) são **detectados e avisados**; a v1 não desbloqueia nem varre dentro deles.

## Instalação

Baixe o pacote do seu sistema em **[Releases](../../releases)**:

| | CLI | App gráfico |
|---|---|---|
| Windows | `guara-*-windows-x86_64.zip` | instalador `.msi` / `.exe` |
| macOS | `guara-*-macos-{aarch64,x86_64}.tar.gz` | `.dmg` |
| Linux | `guara-*-linux-{x86_64,aarch64}.tar.gz` | `.AppImage` / `.deb` |

Cada arquivo tem um `.sha256` ao lado. Compilar do código: `cargo build --release -p guara-cli` (Rust 1.80+).

> Os binários ainda **não são assinados**: o Windows SmartScreen e o Gatekeeper do macOS vão avisar. Confira o SHA-256.

Ler discos brutos exige privilégios: **administrador** (Windows), `sudo` (Linux/macOS).

## Uso (terminal)

```bash
sudo guara devices                       # discos e pendrives (avisa TRIM e volumes montados)
sudo guara inspect /dev/sdb              # partições, espaço não particionado, cifragem
sudo guara scan /dev/sdb --session s.json --list      # varredura profunda
sudo guara scan /dev/sdb --quick         # só metadados (segundos)
guara scan disco.img --category image,document --min-confidence medium
guara scan disco.img --recover /mnt/outro-disco/Recuperados
guara recover s.json /mnt/outro-disco/Recuperados --category image
guara resume s.json                      # continua após Ctrl+C
sudo guara image /dev/sdb /mnt/outro-disco/sdb.img    # imagem com mapa de erros
sudo guara hash /dev/sdb                 # MD5, SHA-1 e SHA-256 do dispositivo
sudo guara search /dev/sdb "contrato" "cpf"            # texto em todo o disco (UTF-8 e UTF-16)
guara export s.json achados.csv --category image       # planilha dos achados
guara report s.json relatorio.html --case 2026-001 --examiner "Nome" --hash-source
guara ui                                 # interface no navegador (127.0.0.1 + token)
```

`--json` está disponível em `devices`, `inspect` e `scan` para automação.

## Interface gráfica

App desktop (Tauri) no estilo de um gerenciador de arquivos, pensado para quando o Guará acha milhares de itens:

- **Grade de miniaturas** (gerada pelo núcleo, rápida mesmo com milhares de fotos) ou **lista**, com controle de tamanho, agrupamento por dia ou mês e rolagem virtual que aguenta dezenas de milhares de arquivos.
- **Navegação lateral** por tipo, **data real da foto (EXIF)**, pasta, **câmera**, confiança, com localização GPS e ocultando duplicadas.
- **Seleção como no Finder/Explorer:** clique, Ctrl/Cmd, Shift, setas, Ctrl+A. Espaço, Enter ou duplo clique abrem a **visualização rápida** em tela cheia com navegação por ← →.
- **Painel de detalhes** com pré-visualização, datas (tirada, modificada, criada, acessada) e metadados.
- Pausar/continuar, salvar sessão, tema **Auto / Claro / Escuro** (lembrado também ao reiniciar como administrador) e layout responsivo. O mesmo front-end roda em `guara ui`.

**Pré-visualização antes de recuperar** (lida direto do disco, sem gravar nada): imagens, **vídeo (MP4/MOV/WebM)** e áudio com player, **PDF** página a página, texto, documentos Word/Excel/PowerPoint/OpenDocument (texto extraído), conteúdo de ZIP e visão hexadecimal para o resto. Quando um arquivo não abre, o aviso ajuda a julgar se ele está danificado. A reprodução de vídeo depende dos codecs do seu sistema (no Linux, instale os plugins `gstreamer1.0-libav` e `gstreamer1.0-plugins-good`).

## Recursos periciais

Pensados para quem precisa documentar o que fez:

- **Hashes MD5, SHA-1 e SHA-256** de cada arquivo recuperado, da imagem forense e do dispositivo inteiro (`guara hash`).
- **Metadados:** EXIF (câmera, lente, número de série, data, **GPS**), informações de PDF (autor, programa, datas), propriedades de documentos Office e OpenDocument (autor, última modificação por, empresa) e dados de vídeo MP4/MOV (data de gravação, duração, resolução).
- **Linha do tempo:** datas de criação, modificação e acesso lidas do NTFS, FAT e exFAT.
- **Pesquisa de texto no disco inteiro** (`guara search`), inclusive espaço livre e fora de partições, em UTF-8 e UTF-16, mostrando posição, partição e contexto.
- **Exportação CSV** (abre no Excel) e **relatório HTML** pronto para imprimir (`guara report`), com identificação do caso, origem, partições, resumo, hashes e lista dos arquivos.
- **Detecção de duplicadas** por assinatura de conteúdo.

Escopo: o Guará trabalha com **mídias de armazenamento** (HD, SSD, pendrive, cartões e imagens de disco). Ele **não** desbloqueia celulares nem contorna senhas ou travas de segurança, e não substitui ferramentas de extração de dispositivos móveis.

## Limites conhecidos (v0.1)

Seja realista sobre o que dá para esperar:

- **Dados sobrescritos não voltam.** Em **SSD com TRIM** o próprio disco pode zerar blocos apagados; o Guará avisa, mas não faz milagre.
- **Carving só acha arquivos contíguos.** Arquivos fragmentados sem metadados podem sair truncados (marcados com confiança baixa).
- **ext2/3/4, APFS e HFS+:** só varredura por assinatura (no ext, apenas no espaço livre). Não há leitura de inodes/journal ainda.
- **NTFS:** arquivos comprimidos/EFS e registros com lista de atributos (arquivos muito fragmentados) são ignorados ou marcados como incompletos.
- **exFAT:** coberto por teste com imagem sintética; falta validar em mais mídias reais.
- **Cifragem:** somente detecção.
- **Windows/macOS:** o código compila e é testado em CI, mas os testes com imagens reais rodam só no Linux. Relatos de mídias reais são muito bem-vindos.
- **GUI:** no Linux/macOS o app reinicia inteiro como administrador (via `pkexec`/`osascript`) em vez de usar um processo auxiliar separado (melhoria planejada).

## Desenvolvimento

```
crates/guara-core   núcleo: leitura, partições, sistemas de arquivos, carving, fusão, recuperação
crates/guara-cli    binário `guara` (CLI + servidor local da UI)
apps/gui            interface (ui/ = HTML/CSS/JS; src-tauri/ = app desktop)
scripts/            geração de imagens de teste (FAT32, NTFS, ext4, exFAT)
```

```bash
cargo test --workspace        # no Linux gera imagens reais com mtools/ntfs-3g/e2fsprogs (veja scripts/)
cargo clippy --workspace --all-targets
```

Veja [CONTRIBUTING.md](CONTRIBUTING.md). Versões seguem [SemVer](https://semver.org/lang/pt-BR/); mudanças em [CHANGELOG.md](CHANGELOG.md). Para publicar: `git tag vX.Y.Z && git push --tags` (o workflow `release` compila e publica tudo).

## Licença

[MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à sua escolha.
