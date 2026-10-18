# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/); versões seguem SemVer.

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
