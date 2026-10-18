# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/); versões seguem SemVer.

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
