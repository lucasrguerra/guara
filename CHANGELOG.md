# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/); versões seguem SemVer.

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
