# Changelog

[English](CHANGELOG.md) · **Português**

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/); versões seguem SemVer.

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
