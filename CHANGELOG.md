# Changelog

[English](CHANGELOG.md) · [Português](CHANGELOG.pt-BR.md)

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow SemVer.

## [1.3.1] - 2026-10-23

### Changed
- **New official logo:** the maned wolf (guará) inside the orange ring, drawn by the project's author, with a transparent background so the same art works in the light and dark themes. It replaces the old track-disc logo in the app (sidebar, top bar and home screen), the favicon, the desktop app icons and the README.
- `guara ui` serves the new `guara.png` and `guara-s.png`; `scripts/gerar-logo.py` derives the small logo and the app icon from the official art.
- Development: `scripts/verificar.sh` runs locally what the CI runs, with an optional pre-push hook (`scripts/instalar-hooks.sh`), and the CI no longer runs on documentation-only changes.

### Notes
- No change to scanning, recovery, signing or any other behavior. This is the first release after the in-app updater: an app on 1.3.0 should offer **Update now**.
- The screenshots in `docs/img/` still show the old logo and will be redone.

## [1.3.0] - 2026-10-23

### Added
- **Update from inside the desktop app.** On startup the app asks GitHub for the release's `latest.json`; if there is a newer version, a card offers **Update now**. It downloads the package, verifies its minisign signature against the public key built into the app, installs and reopens (the AppImage is replaced in place; Windows and macOS use the installer). Installs from `.deb` or `.rpm` only show the notice. **Settings** can turn the startup check off and has a **Check now** button. The check is one HTTP request and sends nothing of yours.
- **Digital signatures (Ed25519):** `guara sign keygen | file | verify | recovery`. With a key configured, `recover` and `report` sign the manifest and the report by themselves. `guara sign recovery` checks the manifest signature and the hash of every recovered file (and refuses manifest paths that leave the folder). The signature is plain Ed25519 over a documented message, so `openssl` verifies it too. Comparing the key fingerprint (`--trust`) says whose key it is; the signed time is the local clock, not a third-party timestamp.
- **Virtual disks:** QCOW2 with a backing file, VMDK snapshots (parent by name or, in VirtualBox, by UUID; the child is refused if the parent's CID changed) and DMG with LZFSE chunks.
- **CI with real reference images** on Linux, macOS and Windows (they were skipped before), and an XFS image deleted by the real kernel generated with `sudo` on the Linux runner.
- Release pipeline: update packages are signed with a key kept in the repository's `release` environment (tags `v*` only), and `latest.json` is built in one step by `scripts/make-latest-json.py`.

### Changed
- **XFS is now 🟡.** On a real XFS (v5, kernel deletion) the inode and extent list are wiped, so the metadata path finds nothing; the content of files with a recognizable structure comes back by carving, without name or folder. The earlier path only worked on simulated deletions. The name-and-folder test is kept `ignore`d until the journal is read.
- CI `apt` steps use cached `.deb`s, a time limit per attempt and retries (they took up to 28 minutes on a slow mirror day).
- Dependencies: `ctutils` 0.4.5 (RUSTSEC-2026-0385); new `ed25519-compact`, `lzfse_rust`, `tauri-plugin-updater`.
- The README and ROADMAP now say the update check is the only network use by default.

### Notes
- Versions before 1.3.0 do not have the updater: from 1.2.0, replace the AppImage by hand one last time. The first update inside the app is 1.3.0 to the next release.
- Still missing: password-protected DMG, an identity check for VHD, VHDX and QCOW2 parents, third-party timestamps (RFC 3161), XFS journal reading, and APFS validation with real deletion (needs an image from a real Mac).
- CLI output remains in Portuguese.

## [1.2.0] - 2026-10-22

### Added
- **Virtual disk containers (read-only):** QCOW2 (v2 and v3, compressed clusters with deflate or zstd), VMDK (sparse, `streamOptimized`, and text descriptors with flat and zero extents), VHD and VHDX (fixed, dynamic and differencing, following the parent chain) and DMG (UDIF with zero, raw, ADC, zlib and bzip2 chunks). They open through the same path as E01 and AFF4, so partitions, file systems and encrypted volumes work inside them. Images that would read wrong are refused with the reason: QCOW2 with a backing file or encryption, VMDK with a parent or in SEsparse, password-protected DMG, DMG with LZFSE chunks. The VHDX log is not replayed.
- **Audit trail** (`auditoria.jsonl`, `guara audit`): every action on evidence (`inspect`, `scan`, `recover`, `image`, `hash`, `search`, `verify`, exports and sessions) is recorded with UTC time, system user, host, version and details, chained by SHA-256. `guara audit --verify` reports the entry where the chain breaks. Set `GUARA_EXAMINER` to record the examiner; passwords and keys are never written.
- **PDF report:** `guara report out.pdf` (and the interface's report button, by file extension) writes an A4 PDF with the same sections as the HTML, using a built-in writer with the standard fonts and no new dependency. Both formats now show the audit trail status and the hash of its last entry.
- **APFS free-space map:** the container's space manager (spaceman), chunk-info blocks and bitmaps are read, so deep scans of an APFS volume cover only unallocated blocks. Containers that use chunk-info address blocks fall back to the whole volume, with a warning.
- `scripts/fetch-testdata.sh` also downloads the dfvfs virtual disk images used by the container tests.

### Changed
- The APFS partition warning no longer says metadata is unsupported; it says the free-space map is read and that encrypted (FileVault) volumes need the key.
- The interface's report dialog explains that the file extension chooses PDF or HTML.

### Notes
- Containers were checked by hash against an independent reader (7-Zip) on real images from dfvfs and VirtualBox, including differencing VHD and VHDX; the small fixtures in `tests/fixtures/vdisk` run in CI.
- APFS deletion is still tested only in simulation, so APFS stays 🟡 in the roadmap.
- CLI output remains in Portuguese.

## [1.1.0] - 2026-10-20

### Added
- **Automated translation check** (`scripts/check-ui-i18n.mjs`): scans the interface JS, HTML and the labels emitted by the Rust core, fails on Portuguese text without an English entry and on duplicate dictionary keys. It runs in the CI lint job.
- **Dependabot group for RustCrypto:** `sha2`, `sha1`, `md-5`, `hmac`, `pbkdf2`, `ripemd`, `aes`, `argon2`, `cipher` and `digest` now arrive in one update, because their traits are only compatible when the versions move together.

### Changed
- **English interface for core messages:** the most common errors from the core now have `{n}` patterns in the dictionary, so paths and sizes are translated too.
- **Notifications in the chosen language:** the core formats scan/recovery notifications using the saved interface language (`ui_lang` follows `lang` or the system locale when unset).
- **ISO 8601 dates:** EXIF, PDF and MP4 metadata dates are emitted as `YYYY-MM-DDTHH:MM:SS` (MP4 with `Z`) and formatted by the interface in the selected locale, instead of a pre-formatted Brazilian date.
- **RustCrypto on digest 0.11 / cipher 0.5:** `aes` 0.9, `sha2` 0.11, `sha1` 0.11, `md-5` 0.11, `hmac` 0.13, `pbkdf2` 0.13, `ripemd` 0.2 and `argon2` 0.6. BitLocker, LUKS and FileVault 2 were checked against the reference images.
- **Release workflow:** `tauri-action` updated to v1; the publish job restores the release name and the CHANGELOG notes after the desktop builds, so v1 cannot overwrite them.
- **Docker documentation:** `--device=...:r` was verified with a loop device (`inspect`, `scan`, `recover` read; writes are refused). The SMART/`SYS_RAWIO` wording now states what was and was not verified.
- **Superset workspace:** setup, Run and teardown were verified in real workspaces; two interfaces get different ports, and deleting a workspace stops its `guara ui`.

### Fixed
- Eight duplicated keys in `apps/gui/ui/i18n-en.js` (the last one won silently).
- Untranslated fragments split by inline tags, metadata labels and preview messages that the previous manual pass missed.

### Notes
- No change to scanning or recovery logic. The encrypted-volume tests pass with `GUARA_REQUIRE_FIXTURES=1`.
- CLI output remains in Portuguese.

## [1.0.1] - 2026-10-20

### Added
- **Docker image of the command line tool** (`Dockerfile`, [docs/docker.md](https://github.com/lucasrguerra/guara/blob/main/docs/docker.md)): for Linux hosts, servers without a desktop and forensic image files. The graphical interface is not served from the container.
- **Interface screenshots** in the README and in the new [docs/interface.md](https://github.com/lucasrguerra/guara/blob/main/docs/interface.md), taken from the real interface on a synthetic test image.
- **Superset workspace configuration** (`.superset/`): setup, run and teardown scripts for parallel workspaces.

### Changed
- **Documentation:** English is now the main language. Portuguese versions live in `*.pt-BR.md` and `docs/pt-BR/`, with a language switch at the top of each file.
- **English interface completed:** scan options, the disk image dialog, metadata labels (EXIF, PDF, MP4), file kinds, disk health fields, selection counters and preview messages were still showing in Portuguese.
- **Language switch:** the explorer now redraws after changing the language in Settings, so dates and month headings follow it.
- Dependencies: `getrandom` 0.4 and `windows-sys` 0.61. GitHub Actions updated (`checkout` 7, `setup-node` 7, `github-script` 9).

### Fixed
- White text on a white background in the Language and Sort drop-down lists with the dark theme (including Auto with the system in dark mode).
- The "Format" field of the disk image dialog was translated as "Form factor" in English.

### Notes
- No change to scanning or recovery logic. The dependency updates touch the token of the local interface server (`getrandom`) and the Windows device bindings (`windows-sys`).
- Error messages from the core, notifications and the terminal output are still in Portuguese.

## [1.0.0] - 2026-10-19

### Added
- **File systems with metadata:** ext4 (deleted inodes and old copies in the journal), HFS+ (node leftovers and journal), Btrfs (old versions of the trees; zlib, zstd and lzo extents), XFS (freed inodes and names of freed entries) and APFS (snapshots and old nodes). ReFS is still only detected.
- **Encrypted volumes:** BitLocker (password, recovery key, .BEK, clear key), LUKS1/LUKS2 and FileVault 2 (Core Storage) open with the password or key and are scanned inside. Passwords are never written to the session file.
- **RAID and LVM:** RAID 0, 1, 4, 5, 6, 10 and linear from mdadm superblocks, including degraded; automatic discovery of level, stripe size, layout and order when there is no metadata; LVM2 (linear, striped, mirrored and RAID).
- **Forensic images:** reading of E01/Ex01 and AFF4; writing of E01 and AFF4; `guara verify` command.
- **Carving:** fragmented JPEG reassembled (up to three fragments), index reconstruction for MP4/MOV without moov (H.264/HEVC, with a reference video) and 24 formats recognized by structure.
- **Interface:** fields for password, RAID, reference video and image format.

### Fixed
- Reopening a recent session that is already open no longer creates identical copies in the sessions tab.

### Limitations
- Storage Spaces and ReFS (metadata) are not supported; XFS and APFS tested with simulated deletions; audio is not reconstructed in MP4 without moov.

## [0.6.0] - 2026-10-18

### Added
- **CrystalDiskInfo-style disk health:** the device screen shows the status (Good, Caution, Bad) with the % of remaining life on SSDs, temperature, power-on hours, power cycles, total read and total written. It also shows the device sheet (model, firmware, serial number, interface, transfer mode, capacity, rotation, form factor and features such as TRIM) and the full table of SMART attributes, with ID, current value, worst, threshold and raw value, and an indicator per attribute. Works with ATA/SATA and NVMe disks (on NVMe, the health log fields become the table).
- **Partitions:** when opening a partition (`/dev/sda1`, `/dev/nvme0n1p2`, `/dev/disk2s1`), SMART is read from the disk that contains it, and the screen says so.
- **`guara health` command** in the terminal, with readable output or `--json`.

### Changed
- A SMART attribute with a normalized value at or below the failure threshold now marks the disk as "Bad", even if the overall self-assessment still passes.
- **New logo:** the head of a maned wolf cut out of the tracks of a disk, whole on one side (recovered) and broken on the other (lost), in the app colors. In the interface it follows the chosen theme (light or dark).

## [0.5.0] - 2026-10-18

### Added
- **Database viewer:** SQLite files (`.db`, `.sqlite`, `.sqlite3`, `.db3`, `.sdb`, `.sqlitedb`, `.storedata`) open in the preview with schema, paginated tables, blobs and recognized dates. Own reader, read-only, tolerant of truncated or corrupted files. It also **recovers deleted rows** from the free space of pages.
- **Disk health (SMART):** before scanning, Guará queries `smartctl` (when installed) and the kernel error counter. If the disk looks faulty, it recommends making the image first and asks for confirmation before the deep scan.
- **System notification** when a long scan or a recovery finishes, or pauses because of a disconnection.
- **Resume an interrupted recovery:** a journal in the destination folder makes Guará skip what was already copied and checked; the result shows how many were skipped.
- **Map of photos with GPS** in the details panel (optional, uses the internet) and a button to open it in OpenStreetMap.
- **English interface**, with a language choice (Auto, Português, English) in the new Settings window, which also controls notifications and online maps.
- **Closing the window during a scan** asks whether you want to pause and quit, quit without pausing, or continue.

### Notes
- Error messages coming from the core and the terminal are still in Portuguese.

## [0.4.0] - 2026-10-18

### Added
- **Disk reconnection:** if the disk was disconnected and came back under another name (for example `/dev/sda` becoming `/dev/sdb`), Guará recognizes it by itself by its size and a fingerprint of the content (first and last MiB) and reattaches the session. If it cannot, the screen shows "Reconnect the disk" and "Choose the source...", with the list of connected disks and a "same disk" badge. A source with different content is refused, unless you tick "use anyway" (useful for copies).
- **Disconnection in the middle of a scan:** if reads fail several times in a row, the scan pauses by itself with the exact point saved and tells you to reconnect and continue, instead of skipping blocks and finishing with holes.
- **Sessions saved automatically** from time to time during the scan and when it finishes or pauses, in the user's configuration folder, and listed under "Recent" in the sidebar (`guara sessions` in the terminal). Atomic writes, with no risk of corrupting the session if the program closes midway.
- **Organization when recovering:** original folders, by type, by date (year and month) or everything in the same folder (`--organize` in the terminal).
- **Free space check** at the destination before copying, with a clear message if it will not fit.

### Fixed
- In paused sessions there was no way to choose another source; only "Continue", which failed without the disk.

## [0.3.2] - 2026-10-18

### Fixed
- **Freeze when clicking a video in the Linux app.** The AppImage ships WebKitGTK without the GStreamer media plugins: when a `<video>` or `<audio>` was created the page process died and the window froze. Now the Linux app never creates those elements; video and audio have an "Open in system player" button, which copies the file to a temporary folder of the user and opens it in the default player (even when Guará runs as administrator). In the browser (`guara ui`) and on other systems the video still plays inside the app, but only in the large view, never in the small panel.

### Changed
- Clicking the preview in the details panel opens the large view (image, PDF, document and other types), now larger (up to 98% of the window width and 95% of its height).

## [0.3.1] - 2026-10-18

### Added
- "Hide small files (under 30 KB)" and "Over 1 megapixel" filters in the explorer, to separate photos from icons and thumbnails when there are thousands of images.

### Fixed
- Opening a saved session failed entirely when the source (for example `/dev/sdb`) was not accessible, and the error message was vague. Now the session always opens: you can browse, export and generate a report, and the "Choose the source..." button reconnects the disk or an image to go back to previewing and recovering.
- Clear messages when the session file cannot be read (for example, created as administrator in `/root`) or is not a valid session.
- An automatic test ensures that sessions written by v0.1.0 still open.
- PNG carving: ASCII text right after a truncated PNG was read as a block of about 1.9 GB and produced giant low-confidence "photos". Now the block must be plausible (valid type, reasonable size, inside the disk) and the file ends at the last valid block.
- Resuming a scan of a session without the source available now warns immediately, instead of failing later.
- When running as administrator (sudo/pkexec), the suggested folders are now those of the user who launched Guará, and everything it writes (sessions, CSV, reports, images and recovered files, with the folders created) keeps the owner and permissions of that user, not root.

## [0.3.0] - 2026-10-18

### Added
- **File explorer:** thumbnail grid and list with virtual scrolling (thousands of items), grouping by day/month, sidebar navigation by type, photo date, folder, camera and confidence, selection with Ctrl/Shift/keyboard, details panel and full-screen quick look.
- **Fast thumbnails** generated in the core (JPEG with downscaling during decoding, PNG, GIF, BMP, WebP), with EXIF orientation and use of the embedded thumbnail when the image is damaged.
- **Metadata:** EXIF (camera, lens, dates, GPS), PDF, Office/OpenDocument and MP4/MOV; creation, modification and access dates from NTFS, FAT and exFAT.
- **Forensics:** MD5, SHA-1 and SHA-256 hashes, `guara hash`, `guara search` (text across the whole disk in UTF-8 and UTF-16), `guara export` (CSV), `guara report` (HTML report with chain of custody) and duplicate detection. All of it also in the graphical interface.
- New visual identity: a pixel logo that comes together into a maned wolf with a smooth silhouette.

### Changed
- The bottom bar and the progress panel are no longer recreated on every update.
- Text without em dashes.

## [0.2.0] - 2026-10-18

### Added
- **Full preview:** image (zoom), video (MP4 and others), audio, PDF (pdf.js included, works offline), text, Word/Excel/PowerPoint/OpenDocument, ZIP contents and hex; full screen with keyboard navigation (↑ ↓ ← → Space Esc). Content is read on demand with Range support, without writing to disk.
- **Auto / Light / Dark** theme selector; the choice is saved and also applies to the app restarted as administrator.
- New teal visual identity, with a pixel logo that comes together into a maned wolf.

### Fixed
- Restarting as administrator on Linux: in the AppImage the executable sits on a mount point inaccessible to root; now it uses the `.AppImage` itself, shows the reason if it fails and closes the old app only when the new one opens.
- The bottom action bar flickered on every update; now it is created once and only the text updates. The progress panel also stopped being recreated on every cycle.
- FAT32 on small media (fewer than 65,525 clusters) was not recognized.
- The theme did not follow the choice when opened as root (root's WebView used another system theme).

## [0.1.0] - 2026-10-18

### Added
- Rust core, read-only on the source: access to devices and images (Windows, Linux, macOS).
- Reading of MBR/GPT, detection of lost partitions and unpartitioned space.
- Metadata from NTFS, FAT12/16/32 and exFAT: deleted files with name, folder and date.
- Structure-aware carving of JPEG, PNG, GIF, BMP, PDF, ZIP and derivatives (DOCX, XLSX, PPTX, ODF, EPUB, APK, JAR), MP4/MOV/HEIC, AVI/WAV/WebP, SQLite, 7z, OLE, MP3 and PE.
- Free block map of ext2/3/4 to scan only the free space.
- Merging without duplicates and per-file confidence (high/medium/low).
- Detection of encrypted volumes (BitLocker, LUKS) and TRIM warning on SSDs.
- Forensic image tolerant of bad sectors, pause/resume of scans and saved sessions.
- Recovery with SHA-256 and manifest; refusal of a destination on the source disk.
- `guara` CLI and graphical interface (Tauri) with light/dark theme and responsive layout; `guara ui` mode in the browser.
- CI (Linux/Windows/macOS) and automatic releases by tag.
