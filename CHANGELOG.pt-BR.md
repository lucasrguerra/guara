# Changelog

[English](CHANGELOG.md) · **Português**

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/); versões seguem SemVer.

## [1.2.0] - 2026-10-22

### Adicionado
- **Contêineres de disco virtual (somente leitura):** QCOW2 (v2 e v3, clusters comprimidos com deflate ou zstd), VMDK (esparso, `streamOptimized` e descritores de texto com extensões flat e zero), VHD e VHDX (fixo, dinâmico e diferencial, seguindo a cadeia de pais) e DMG (UDIF com chunks zerados, brutos, ADC, zlib e bzip2). Abrem pelo mesmo caminho do E01 e do AFF4, então partições, sistemas de arquivos e volumes cifrados funcionam dentro deles. Imagens que seriam lidas errado são recusadas com o motivo: QCOW2 com arquivo de apoio ou cifrado, VMDK com pai ou em SEsparse, DMG protegido por senha, DMG com chunks LZFSE. O log do VHDX não é reproduzido.
- **Trilha de auditoria** (`auditoria.jsonl`, `guara audit`): toda ação sobre a evidência (`inspect`, `scan`, `recover`, `image`, `hash`, `search`, `verify`, exportações e sessões) é registrada com hora UTC, usuário do sistema, máquina, versão e detalhes, encadeada por SHA-256. `guara audit --verify` aponta a entrada onde a cadeia quebra. Defina `GUARA_EXAMINER` para registrar o perito; senhas e chaves nunca são gravadas.
- **Relatório em PDF:** `guara report saida.pdf` (e o botão de relatório da interface, pela extensão do arquivo) gera um PDF A4 com as mesmas seções do HTML, com gerador próprio, fontes padrão e nenhuma dependência nova. Os dois formatos passam a mostrar a situação da trilha de auditoria e o hash da última entrada.
- **Mapa de espaço livre do APFS:** o gerenciador de espaço do contêiner (spaceman), os chunk-info blocks e os bitmaps são lidos, e a varredura profunda de um volume APFS cobre só os blocos não alocados. Contêineres que usam blocos de endereço de chunk-info voltam a varrer o volume todo, com aviso.
- `scripts/fetch-testdata.sh` também baixa as imagens de disco virtual do dfvfs usadas nos testes dos contêineres.

### Alterado
- O aviso da partição APFS deixa de dizer que não há suporte a metadados; informa que o mapa de espaço livre é lido e que volumes cifrados (FileVault) precisam da chave.
- O diálogo de relatório da interface explica que a extensão do arquivo escolhe PDF ou HTML.

### Notas
- Os contêineres foram conferidos por hash contra um leitor independente (7-Zip) em imagens reais do dfvfs e do VirtualBox, inclusive VHD e VHDX diferenciais; os fixtures pequenos de `tests/fixtures/vdisk` rodam no CI.
- A exclusão no APFS continua testada só em simulação, por isso o APFS segue 🟡 no roadmap.
- A saída da CLI continua em português.

## [1.1.0] - 2026-10-20

### Adicionado
- **Verificação automática da tradução** (`scripts/check-ui-i18n.mjs`): varre o JS e o HTML da interface e os rótulos emitidos pelo núcleo em Rust, falha quando há texto em português sem entrada em inglês e quando há chave duplicada no dicionário. Roda no job de lint do CI.
- **Grupo RustCrypto no Dependabot:** `sha2`, `sha1`, `md-5`, `hmac`, `pbkdf2`, `ripemd`, `aes`, `argon2`, `cipher` e `digest` passam a chegar numa única atualização, porque as traits só são compatíveis quando as versões andam juntas.

### Alterado
- **Interface em inglês para mensagens do núcleo:** os erros mais comuns do núcleo agora têm padrões com `{n}` no dicionário, então caminhos e tamanhos também são traduzidos.
- **Notificações no idioma escolhido:** o núcleo monta as notificações de varredura e recuperação usando o idioma salvo da interface (`ui_lang` segue `lang` ou o locale do sistema quando não definido).
- **Datas em ISO 8601:** as datas de metadados EXIF, PDF e MP4 saem como `YYYY-MM-DDTHH:MM:SS` (MP4 com `Z`) e são formatadas pela interface no locale escolhido, em vez de uma data brasileira já pronta.
- **RustCrypto em digest 0.11 / cipher 0.5:** `aes` 0.9, `sha2` 0.11, `sha1` 0.11, `md-5` 0.11, `hmac` 0.13, `pbkdf2` 0.13, `ripemd` 0.2 e `argon2` 0.6. BitLocker, LUKS e FileVault 2 foram conferidos contra as imagens de referência.
- **Fluxo de release:** `tauri-action` atualizado para v1; o job de publicação restaura o nome e as notas do CHANGELOG depois dos builds do desktop, para a v1 não sobrescrevê-las.
- **Documentação do Docker:** `--device=...:r` foi verificado com um dispositivo de loop (`inspect`, `scan`, `recover` leem; escritas são recusadas). O texto sobre SMART/`SYS_RAWIO` agora diz o que foi e o que não foi verificado.
- **Workspace do Superset:** setup, Run e teardown foram verificados em workspaces reais; duas interfaces recebem portas diferentes, e apagar um workspace encerra o `guara ui`.

### Corrigido
- Oito chaves duplicadas em `apps/gui/ui/i18n-en.js` (a última vencia em silêncio).
- Trechos sem tradução separados por tags inline, rótulos de metadados e mensagens da pré-visualização que a passada manual anterior não pegou.

### Notas
- Nenhuma mudança na lógica de varredura ou recuperação. Os testes de volumes cifrados passam com `GUARA_REQUIRE_FIXTURES=1`.
- A saída do terminal continua em português.

## [1.0.1] - 2026-10-20

### Adicionado
- **Imagem Docker do terminal** (`Dockerfile`, [docs/pt-BR/docker.md](https://github.com/lucasrguerra/guara/blob/main/docs/pt-BR/docker.md)): para hospedeiros Linux, servidores sem desktop e arquivos de imagem forense. A interface gráfica não é servida pelo container.
- **Telas da interface** no README e na nova [docs/pt-BR/interface.md](https://github.com/lucasrguerra/guara/blob/main/docs/pt-BR/interface.md), tiradas da interface real sobre uma imagem de teste sintética.
- **Configuração de workspaces do Superset** (`.superset/`): scripts de setup, run e teardown para workspaces em paralelo.

### Alterado
- **Documentação:** o inglês passa a ser o idioma principal. As versões em português ficam em `*.pt-BR.md` e `docs/pt-BR/`, com seletor de idioma no topo de cada arquivo.
- **Interface em inglês completada:** opções de varredura, diálogo de imagem de disco, rótulos de metadados (EXIF, PDF, MP4), tipos de arquivo, campos de saúde do disco, contadores de seleção e mensagens da pré-visualização ainda apareciam em português.
- **Troca de idioma:** o explorador agora se redesenha ao mudar o idioma em Configurações, e as datas e os títulos de mês acompanham.
- Dependências: `getrandom` 0.4 e `windows-sys` 0.61. GitHub Actions atualizadas (`checkout` 7, `setup-node` 7, `github-script` 9).

### Corrigido
- Texto branco sobre fundo branco nas listas de Idioma e Ordenar com o tema escuro (inclusive Auto com o sistema em escuro).
- O campo "Formato" do diálogo de imagem de disco era traduzido como "Form factor" em inglês.

### Notas
- Nenhuma mudança na lógica de varredura ou de recuperação. As atualizações de dependências tocam o token do servidor local da interface (`getrandom`) e as ligações de dispositivo do Windows (`windows-sys`).
- Mensagens de erro do núcleo, notificações e a saída do terminal continuam em português.

## [1.0.0] - 2026-10-19

### Adicionado
- **Sistemas de arquivos com metadados:** ext4 (inodes apagados e cópias antigas no diário), HFS+ (sobras de nós e diário), Btrfs (versões antigas das árvores; extensões zlib, zstd e lzo), XFS (inodes liberados e nomes de entradas liberadas) e APFS (instantâneos e nós antigos). ReFS ainda é só detectado.
- **Volumes cifrados:** BitLocker (senha, chave de recuperação, .BEK, chave limpa), LUKS1/LUKS2 e FileVault 2 (Core Storage) abrem com a senha ou a chave e são varridos por dentro. As senhas nunca vão para o arquivo de sessão.
- **RAID e LVM:** RAID 0, 1, 4, 5, 6, 10 e linear a partir de superblocos do mdadm, inclusive degradado; descoberta automática de nível, faixa, layout e ordem quando não há metadados; LVM2 (linear, em faixas, espelhado e RAID).
- **Imagens forenses:** leitura de E01/Ex01 e AFF4; gravação de E01 e AFF4; comando `guara verify`.
- **Carving:** JPEG fragmentado remontado (até três fragmentos), reconstrução do índice de MP4/MOV sem moov (H.264/HEVC, com vídeo de referência) e 24 formatos reconhecidos pela estrutura.
- **Interface:** campos de senha, RAID, vídeo de referência e formato da imagem.

### Corrigido
- Abrir de novo uma sessão recente já aberta não cria mais cópias iguais na aba de sessões.

### Limitações
- Storage Spaces e ReFS (metadados) não suportados; XFS e APFS testados com exclusões simuladas; áudio não é reconstruído em MP4 sem moov.

## [0.6.0] - 2026-10-18

### Adicionado
- **Saúde do disco no estilo do CrystalDiskInfo:** a tela do dispositivo mostra o estado (Bom, Atenção, Ruim) com a % de vida útil restante em SSD, temperatura, horas ligado, vezes ligado, total lido e total gravado. Também traz a ficha do dispositivo (modelo, firmware, nº de série, interface, modo de transferência, capacidade, rotação, formato e recursos como TRIM) e a tabela completa de atributos SMART, com ID, valor atual, pior, limite e bruto, e um indicador por atributo. Funciona com discos ATA/SATA e NVMe (nos NVMe, os campos do log de saúde viram a tabela).
- **Partições:** ao abrir uma partição (`/dev/sda1`, `/dev/nvme0n1p2`, `/dev/disk2s1`), o SMART é lido do disco que a contém, e a tela avisa isso.
- **Comando `guara health`** no terminal, com saída legível ou `--json`.

### Alterado
- Um atributo SMART com valor normalizado no limite de falha (ou abaixo dele) agora marca o disco como "Ruim", mesmo que a autoavaliação geral ainda passe.
- **Nova logo:** a cabeça do lobo-guará recortada das trilhas de um disco, inteiras de um lado (recuperado) e quebradas do outro (perdido), nas cores do app. Na interface ela acompanha o tema escolhido (claro ou escuro).

## [0.5.0] - 2026-10-18

### Adicionado
- **Visualizador de bancos de dados:** arquivos SQLite (`.db`, `.sqlite`, `.sqlite3`, `.db3`, `.sdb`, `.sqlitedb`, `.storedata`) abrem na pré-visualização com esquema, tabelas paginadas, blobs e datas reconhecidas. Leitor próprio, só leitura, tolerante a arquivos truncados ou corrompidos. Também **recupera linhas apagadas** a partir do espaço livre das páginas.
- **Saúde do disco (SMART):** antes de varrer, o Guará consulta o `smartctl` (quando instalado) e o contador de erros do kernel. Se o disco parece com defeito, recomenda criar a imagem primeiro e pede confirmação antes da varredura profunda.
- **Notificação do sistema** quando uma varredura longa ou uma recuperação termina, ou pausa por desconexão.
- **Retomar recuperação interrompida:** um diário na pasta de destino faz o Guará pular o que já foi copiado e conferido; o resultado mostra quantos foram pulados.
- **Mapa das fotos com GPS** no painel de detalhes (opcional, usa a internet) e botão para abrir no OpenStreetMap.
- **Interface em inglês**, com escolha de idioma (Auto, Português, English) na nova janela de Configurações, que também controla notificações e mapas online.
- **Fechar a janela durante a varredura** pergunta se você quer pausar e sair, sair sem pausar ou continuar.

### Notas
- Mensagens de erro vindas do núcleo e o terminal continuam em português.

## [0.4.0] - 2026-10-18

### Adicionado
- **Reconexão do disco:** se o disco foi desconectado e voltou com outro nome (por exemplo `/dev/sda` virando `/dev/sdb`), o Guará o reconhece sozinho pelo tamanho e pela impressão digital do conteúdo (primeiro e último MiB) e religa a sessão. Se não conseguir, a tela mostra "Reconectar o disco" e "Escolher a origem…", com a lista dos discos conectados e o selo "mesmo disco". Uma origem com conteúdo diferente é recusada, a menos que você marque "usar mesmo assim" (útil para cópias).
- **Desconexão no meio da varredura:** se as leituras falharem várias vezes seguidas, a varredura pausa sozinha com o ponto exato salvo e avisa para reconectar e continuar, em vez de seguir pulando blocos e terminar com buracos.
- **Sessões salvas automaticamente** de tempos em tempos durante a varredura e ao terminar ou pausar, na pasta de configuração do usuário, e listadas em "Recentes" na barra lateral (`guara sessions` no terminal). Gravação atômica, sem risco de corromper a sessão se o programa fechar no meio.
- **Organização ao recuperar:** pastas originais, por tipo, por data (ano e mês) ou tudo na mesma pasta (`--organize` no terminal).
- **Verificação de espaço livre** no destino antes de copiar, com mensagem clara se não couber.

### Corrigido
- Em sessões pausadas não havia como escolher outra origem; só "Continuar", que falhava sem o disco.

## [0.3.2] - 2026-10-18

### Corrigido
- **Travamento ao clicar em um vídeo no app para Linux.** O AppImage traz o WebKitGTK sem os plugins de mídia do GStreamer: ao criar um `<video>` ou `<audio>` o processo da página morria e a janela ficava congelada. Agora o app para Linux nunca cria esses elementos; vídeo e áudio têm o botão "Abrir no player do sistema", que copia o arquivo para uma pasta temporária do usuário e o abre no player padrão (inclusive quando o Guará roda como administrador). No navegador (`guara ui`) e nos demais sistemas o vídeo continua tocando dentro do app, mas só na tela grande, nunca no painel pequeno.

### Alterado
- Clicar na pré-visualização do painel de detalhes abre a visualização grande (imagem, PDF, documento e demais tipos), agora maior (até 98% da largura e 95% da altura da janela).

## [0.3.1] - 2026-10-18

### Adicionado
- Filtros "Ocultar arquivos pequenos (menos de 30 KB)" e "Mais de 1 megapixel" no explorador, para separar fotos de ícones e miniaturas quando há milhares de imagens.

### Corrigido
- Abrir uma sessão salva falhava por inteiro quando a origem (por exemplo `/dev/sdb`) não estava acessível, e a mensagem de erro era vaga. Agora a sessão sempre abre: dá para navegar, exportar e gerar relatório, e o botão "Escolher a origem…" reconecta o disco ou uma imagem para voltar a pré-visualizar e recuperar.
- Mensagens claras quando o arquivo de sessão não pode ser lido (por exemplo, criado como administrador em `/root`) ou não é uma sessão válida.
- Teste automático garante que sessões gravadas pela v0.1.0 continuam abrindo.
- Carving de PNG: texto ASCII logo após um PNG truncado era lido como um bloco de ~1,9 GB e gerava "fotos" gigantes de confiança baixa. Agora o bloco precisa ser plausível (tipo válido, tamanho razoável, dentro do disco) e o arquivo termina no último bloco válido.
- Retomar uma varredura de sessão sem a origem disponível agora avisa na hora, em vez de falhar depois.
- Rodando como administrador (sudo/pkexec), as pastas sugeridas passam a ser as do usuário que abriu o Guará e tudo que ele grava (sessões, CSV, relatórios, imagens e arquivos recuperados, com as pastas criadas) fica com o dono e as permissões desse usuário, e não do root.

## [0.3.0] - 2026-10-18

### Adicionado
- **Explorador de arquivos:** grade de miniaturas e lista com rolagem virtual (milhares de itens), agrupamento por dia/mês, navegação lateral por tipo, data da foto, pasta, câmera e confiança, seleção com Ctrl/Shift/teclado, painel de detalhes e visualização rápida em tela cheia.
- **Miniaturas rápidas** geradas no núcleo (JPEG com redução na decodificação, PNG, GIF, BMP, WebP), com orientação EXIF e uso da miniatura embutida quando a imagem está danificada.
- **Metadados:** EXIF (câmera, lente, datas, GPS), PDF, Office/OpenDocument e MP4/MOV; datas de criação, modificação e acesso do NTFS, FAT e exFAT.
- **Perícia:** hashes MD5, SHA-1 e SHA-256, `guara hash`, `guara search` (texto no disco inteiro em UTF-8 e UTF-16), `guara export` (CSV), `guara report` (relatório HTML com cadeia de custódia) e detecção de duplicadas. Tudo também na interface gráfica.
- Nova identidade visual: logo de pixels que se juntam num lobo-guará de silhueta lisa.

### Alterado
- A barra inferior e o painel de progresso não são mais recriados a cada atualização.
- Textos sem travessões.

## [0.2.0] - 2026-10-18

### Adicionado
- **Pré-visualização completa:** imagem (zoom), vídeo (MP4 e outros), áudio, PDF (pdf.js incluso, funciona offline), texto, Word/Excel/PowerPoint/OpenDocument, conteúdo de ZIP e hexadecimal; tela cheia com navegação por teclado (↑ ↓ ← → Espaço Esc). O conteúdo é lido sob demanda com suporte a Range, sem gravar em disco.
- Seletor de tema **Auto / Claro / Escuro**; a escolha é salva e vale também no app reiniciado como administrador.
- Nova identidade visual em verde-azulado, com logo de pixels que se juntam até formar um lobo-guará.

### Corrigido
- Reiniciar como administrador no Linux: no AppImage o executável fica num ponto de montagem inacessível ao root; agora usa o próprio `.AppImage`, mostra o motivo se falhar e fecha o app antigo só quando o novo abre.
- Barra de ações inferior piscava a cada atualização; agora é criada uma vez e só atualiza o texto. O painel de progresso também deixou de ser recriado a cada ciclo.
- FAT32 em mídias pequenas (menos de 65 525 clusters) não era reconhecido.
- O tema não acompanhava a escolha ao abrir como root (o WebView do root usava outro tema do sistema).

## [0.1.0] - 2026-10-18

### Adicionado
- Núcleo em Rust, somente leitura na origem: acesso a dispositivos e imagens (Windows, Linux, macOS).
- Leitura de MBR/GPT, detecção de partições perdidas e de espaço não particionado.
- Metadados de NTFS, FAT12/16/32 e exFAT: arquivos apagados com nome, pasta e data.
- Carving por estrutura de JPEG, PNG, GIF, BMP, PDF, ZIP e derivados (DOCX, XLSX, PPTX, ODF, EPUB, APK, JAR), MP4/MOV/HEIC, AVI/WAV/WebP, SQLite, 7z, OLE, MP3 e PE.
- Mapa de blocos livres do ext2/3/4 para varrer apenas o espaço livre.
- Fusão sem duplicatas e confiança (alta/média/baixa) por arquivo.
- Detecção de volumes cifrados (BitLocker, LUKS) e aviso de TRIM em SSD.
- Imagem forense tolerante a setores ruins, pausar/retomar varredura e sessões salvas.
- Recuperação com SHA-256 e manifesto; recusa de destino no disco de origem.
- CLI `guara` e interface gráfica (Tauri) com tema claro/escuro e layout responsivo; modo `guara ui` no navegador.
- CI (Linux/Windows/macOS) e releases automáticas por tag.
