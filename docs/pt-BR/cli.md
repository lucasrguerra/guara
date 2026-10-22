# Referência do terminal

[English](../cli.md) · **Português**

Ler disco bruto exige privilégios: administrador no Windows, `sudo` no Linux e no macOS. Use `guara <comando> --help` para a lista completa de opções.

## Comandos

| Comando | Para que serve |
|---|---|
| `devices` | Lista discos e pendrives, avisa TRIM e volumes montados |
| `inspect <origem>` | Partições, espaço não particionado, cifragem |
| `health <origem>` | Saúde do disco (SMART); requer `smartmontools` |
| `scan <origem>` | Varredura de arquivos apagados e perdidos |
| `resume <sessão>` | Continua uma varredura pausada |
| `recover <sessão> <destino>` | Recupera os arquivos de uma sessão |
| `image <origem> <destino>` | Imagem forense (bruta, E01 ou AFF4) |
| `verify <imagem>` | Confere uma imagem E01 ou Ex01 contra o hash gravado |
| `hash <origem>` | MD5, SHA-1 e SHA-256 do dispositivo |
| `search <origem> <termos...>` | Texto no disco inteiro, UTF-8 e UTF-16 |
| `export <sessão> <csv>` | Planilha dos achados |
| `report <sessão> <arquivo>` | Relatório imprimível, HTML ou PDF (pela extensão do arquivo) |
| `audit` | Trilha de auditoria de cada ação (quem, quando, o quê); `--verify` confere o encadeamento por hash |
| `sessions` | Sessões salvas automaticamente |
| `ui` | Interface no navegador (127.0.0.1, com token) |

`--json` existe em `devices`, `inspect`, `health` e `scan`.

## Varredura

```bash
sudo guara scan /dev/sdb --session s.json --list     # profunda, com lista
sudo guara scan /dev/sdb --quick                     # só metadados, em segundos
guara scan disco.img --category image,document --min-confidence medium
guara scan disco.img --recover /mnt/outro-disco/Recuperados
```

Filtros (valem em `scan`, `recover`, `export` e `report`):

| Opção | Efeito |
|---|---|
| `--category` | `image`, `document`, `video`, `audio`, `archive`, `database`, `other` (separe por vírgula) |
| `--min-confidence` | `low`, `medium` ou `high` |
| `--name` | só nomes e caminhos que contenham o texto |
| `--organize` | como gravar: `folders` (pastas originais), `type`, `date` ou `flat` |

## Recuperação

```bash
guara recover s.json /mnt/outro-disco/Recuperados --category image
guara resume s.json
```

O destino precisa estar em **outro disco**. Cada recuperação grava `guara-manifesto.json` com MD5, SHA-1 e SHA-256 de todos os arquivos. Se a recuperação for interrompida, repetir o comando pula o que já foi copiado e conferido.

## Volumes cifrados

```bash
sudo guara scan /dev/sdb1 --ask-password
sudo guara scan /dev/sdb1 --recovery-key 111111-222222-333333-444444-555555-666666-777777-888888
sudo guara scan /dev/sdb1 --key-file chave.BEK
sudo guara scan /dev/sdb1 --volume-key 4f2a...
```

| Opção | Uso |
|---|---|
| `--ask-password` | Pergunta no terminal, sem mostrar. **Preferida** |
| `GUARA_PASSWORD` | Variável de ambiente com a senha |
| `--password` | Senha na linha de comando (fica no histórico do shell) |
| `--recovery-key` | Chave numérica do BitLocker |
| `--key-file` | `.BEK` do BitLocker ou keyfile do LUKS |
| `--volume-key` | Chave do volume em hexadecimal (a que o Guará mostra ao abrir um volume) |
| `--remember-keys` | Guarda na sessão as chaves dos volumes abertos (nunca a senha) |

## RAID e LVM

Passe o disco principal e os outros membros com `--member`, repetido. Use `-` para um disco ausente.

```bash
# Com metadados do mdadm ou do LVM: o Guará lê tudo sozinho
guara inspect disco1.img --member disco2.img --member disco3.img

# Sem metadados: descobre nível, faixa, layout e ordem
guara scan disco1.img --member disco2.img --member disco3.img --raid

# Fixando o que você já sabe
guara scan disco1.img --member disco2.img --member - --raid \
  --raid-level 5 --raid-chunk-kb 64 --raid-layout left-symmetric --raid-order 0,1,2
```

## Imagem forense

```bash
sudo guara image /dev/sdb /mnt/outro-disco/sdb.img                      # bruta, com mapa de erros
sudo guara image /dev/sdb /mnt/outro-disco/sdb --format e01 \
  --case 2026-001 --examiner "Nome" --evidence E1 --description "HD do notebook"
sudo guara image /dev/sdb /mnt/outro-disco/sdb --format aff4
guara verify /mnt/outro-disco/sdb.E01
```

Para E01: `--segment-mib` (tamanho de cada segmento, padrão 1500, 0 para um arquivo só) e `--compression` (0 a 9, padrão 1).

## Trilha de auditoria

Toda ação que mexe na evidência é acrescentada a `auditoria.jsonl` na pasta de configuração (ou ao arquivo indicado em `GUARA_AUDIT_LOG`): abrir uma origem (`inspect`, `scan`), `recover`, `image`, `hash`, `search`, `verify`, exportações e sessões. Cada linha traz a hora (UTC), o usuário do sistema (o que está por trás do `sudo`), a máquina, a versão do Guará e os detalhes; defina `GUARA_EXAMINER` para registrar também o nome do perito. Senhas e chaves nunca são gravadas.

```bash
guara audit                  # lista as entradas
guara audit --tail 20 --json
guara audit --verify         # confere a cadeia; sai com erro se estiver quebrada
```

As linhas são encadeadas: cada uma leva o SHA-256 do hash anterior mais o próprio conteúdo, então editar, remover ou reordenar uma entrada é detectado e o `--verify` diz qual. Cortar o fim do arquivo não é detectável só pelo arquivo: guarde o hash que o `--verify` imprime em outro lugar (por exemplo nas notas do caso).

## Vídeo sem índice (MP4 sem moov)

Passe um vídeo **íntegro da mesma câmera ou drone** para o Guará refazer o índice:

```bash
guara scan cartao.img --reference VIDEO_BOM.MP4 --category video
```

`--reference` pode ser repetido. O áudio não é reconstruído.

## Busca de texto

```bash
sudo guara search /dev/sdb "contrato" "cpf" --case-sensitive --max 500
```

Procura em UTF-8 e UTF-16 (`--no-utf16` desliga), inclusive no espaço livre e fora de partições. Mostra posição, partição e contexto. `--max` limita os resultados (padrão 200).

## Relatório

```bash
guara report s.json relatorio.html --case 2026-001 --examiner "Nome" --hash-source
```

Traz identificação do caso, origem, partições, resumo, hashes, a situação da trilha de auditoria e a lista dos arquivos. `--hash-source` inclui o hash da origem inteira. O formato segue a extensão: `relatorio.pdf` gera um PDF A4 (fontes padrão, nada externo); qualquer outra gera HTML.
