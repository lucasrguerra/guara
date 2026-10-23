# Roadmap

[English](ROADMAP.md) · [Português](ROADMAP.pt-BR.md)

Where Guará has been, where it is and where it is going. What is marked as done exists in the code and has a test; what is planned is intent, not a promise, and has no date.

Legend: ✅ done · 🟡 partial · 🔜 planned · 🔭 long-term idea · ⛔ out of scope

## Principles that do not change

1. **The source is read-only.** Nothing in the core writes to the device being recovered.
2. **Do not ship without validating.** A feature only lands with a test against a real or reference image. That is why ReFS and Storage Spaces do not exist yet: there is no reference image to check against.
3. **Honest confidence.** If the content could not be checked, confidence is low or medium, never high.
4. **Free and auditable.** Open source (MIT or Apache-2.0), no telemetry (the only thing that touches the network by default is the update check, a single request you can turn off), and every result can be reproduced.

## History

| Version | Date | Milestone |
|---|---|---|
| 0.1.0 | 2026-10-18 | Read-only core, MBR/GPT, NTFS, FAT, exFAT, structure-aware carving, forensic image, sessions, CLI and interface |
| 0.2.0 | 2026-10-18 | Full preview (image, video, audio, PDF, Office, ZIP, hex) and themes |
| 0.3.0 | 2026-10-18 | Explorer with thumbnails, metadata (EXIF, GPS), hashes, disk search, CSV, HTML report and duplicates |
| 0.4.0 | 2026-10-18 | Disk reconnection, automatic sessions, organization when recovering |
| 0.5.0 | 2026-10-18 | SQLite viewer with deleted rows, SMART, English interface, recovery resume |
| 0.6.0 | 2026-10-18 | Full disk health (ATA and NVMe SMART), `guara health`, new logo |
| 1.0.0 | 2026-10-19 | ext4, HFS+, Btrfs, XFS, APFS; BitLocker, LUKS, FileVault 2; RAID and LVM; E01, Ex01, AFF4; fragmented JPEG; MP4 without moov |
| 1.0.1 | 2026-10-20 | Documentation in English with Portuguese translations, complete English interface, screenshots, Docker image of the CLI |
| 1.1.0 | 2026-10-20 | Core messages and dates in English, automated translation check, RustCrypto on digest 0.11, verified Docker device access and Superset workspaces |
| 1.2.0 | 2026-10-22 | QCOW2, VMDK, VHD, VHDX and DMG virtual disks, hash-chained audit trail, PDF report, APFS free-space map |
| 1.3.0 | 2026-10-23 | Update from inside the app, Ed25519 signatures, differencing QCOW2/VMDK and LZFSE DMG, reference images tested on Linux, macOS and Windows, XFS real-deletion test |

Details of each version in the [CHANGELOG](CHANGELOG.md).

## Current status (1.3.0) against what is expected from a forensic suite

### 1. File systems with metadata

| Item | Status | Note |
|---|---|---|
| NTFS, FAT12/16/32, exFAT | ✅ | Since 0.1.0. NTFS skips compressed and EFS files |
| ext2/3/4 | ✅ | Deleted inodes and old copies in the journal |
| HFS+ | ✅ | Tree node leftovers and journal |
| Btrfs | ✅ | Old versions of the trees; zlib, zstd and lzo extents |
| XFS | 🟡 | Freed inodes and names of freed entries work on simulated deletion only. With real kernel deletion (tested in CI) the metadata is wiped: content comes back by carving, names do not. **Journal reading is missing** |
| APFS | 🟡 | Container, object map, free-space map (spaceman and bitmaps), snapshots and old nodes. Deletion tested only in simulation; containers with chunk-info address blocks (large ones) fall back to scanning the whole volume |
| ReFS | 🔜 | Only detected today; recovery depends on carving. Reference image missing |
| Storage Spaces | 🔜 | Not done. Reference image missing |
| NTFS: compressed attributes, EFS, attribute list | 🔜 | Marked as incomplete today |
| XFS: names and folders after real deletion (read the journal) | 🔜 | The CI test exists and is ignored until this lands |
| APFS validation with real deletion | 🔜 | Needs an image from a real Mac |

### 2. Encrypted volumes

| Item | Status |
|---|---|
| BitLocker (password, recovery key, .BEK, clear key) | ✅ |
| LUKS1 and LUKS2 | ✅ |
| FileVault 2 (Core Storage) | ✅ |
| FileVault 2 on APFS with an encrypted volume | 🔜 |
| Key from memory (hibernation, RAM dump) | 🔭 |
| Dictionary and brute-force attacks (BitLocker, LUKS, FileVault, RAR, ZIP and other encrypted volumes/files) | 🔜 |

Today Guará opens volumes when you **already have** the password or key. Password recovery by dictionary and brute force (with a GPU, like Hashcat) is a planned feature, central to forensic use.

### 3. Forensic images

| Item | Status |
|---|---|
| Raw image (dd) tolerant of bad sectors, with an error map | ✅ |
| E01 and Ex01: read, write and `guara verify` | ✅ |
| AFF4: read and write | ✅ |
| Virtual disk containers: QCOW2, VMDK, VHD, VHDX and DMG (read) | ✅ |
| QCOW2 with a backing file, VMDK snapshots (by name or VirtualBox UUID), DMG with LZFSE | ✅ |
| Password-protected DMG, parent identity check for VHD, VHDX and QCOW2 | 🔜 |
| Mobile device image (see item 7) | 🔭 |

### 4. Assembling loose disks

| Item | Status |
|---|---|
| RAID 0, 1, 4, 5, 6, 10 and linear (mdadm), including degraded | ✅ |
| Automatic discovery of level, stripe size, layout and order without metadata | ✅ |
| LVM2 (linear, striped, mirrored and RAID) | ✅ |
| Storage Spaces | 🔜 |
| Multi-disk Btrfs and ZFS (the file system's own RAID) | 🔭 |
| Proprietary controller RAID (Dell PERC, HP Smart Array) | 🔭 |

### 5. Carving

| Item | Status |
|---|---|
| Detection by real structure, with exact size and confidence | ✅ Since 0.1.0 (JPEG, PNG, GIF, BMP, PDF, ZIP and derivatives, MP4/MOV/HEIC, AVI/WAV/WebP, SQLite, 7z, OLE, MP3, PE) and, in 1.0.0, 24 more (gzip, zstd, Matroska/WebM, Ogg, MPEG-TS, FLV, ASF, MIDI, RAR, ar, SWF, ELF, TIFF, ICO, Windows registry, EVTX, PST, RTF, pcap and others) |
| Fragmented JPEG reassembled (up to three fragments) | ✅ |
| MP4/MOV without `moov` rebuilt (H.264 and HEVC) from a reference video | ✅ |
| Fragments: more than three, and formats beyond JPEG (PNG, DOC, ZIP, SQLite) | 🔜 |
| Video reconstruction without a reference, audio in MP4 without `moov`, drone and action camera videos in other containers | 🔜 |
| More formats validated by structure | 🔜 ongoing |

### 6. Artifact interpretation

Guará **identifies and recovers** these files, but only interprets some of them.

| Item | Status |
|---|---|
| SQLite database: schema, tables, blobs, dates and deleted rows | ✅ |
| Metadata: EXIF and GPS, PDF, Office and OpenDocument, MP4/MOV | ✅ |
| Carving of Windows registry (`regf`), EVTX, PST, pcap | 🟡 only recognizes and recovers, does not interpret |
| Interpret the Windows registry (users, programs, USB, execution history) | 🔜 |
| Browser history, cookies and downloads (Chrome, Firefox, Safari) | 🔜 built on the SQLite reader |
| Messages, contacts and location from apps (WhatsApp, Telegram, Signal and others) | 🔜 built on the SQLite reader |
| System logs and Windows events (EVTX) | 🔜 |
| **Unified timeline** of all artifacts | 🔜 |

### 7. Mobile device extraction

Here the leap is one of kind, not of feature. Today Guará works with **storage media**.

| Item | Status |
|---|---|
| Android through ADB and backups | 🔭 |
| iOS through iTunes or Finder backups (including password-encrypted ones) | 🔭 |
| Physical access through bootloader and chip exploits | 🔭 |

Physical access is the differentiator of commercial tools like Cellebrite. It depends on full-time vulnerability research teams, and many of those techniques cannot be published as open source, which makes this the most distant item on the roadmap. The closest direction is to read what the user or examiner can legitimately export (backups and ADB) and interpret that data with item 6; physical access by exploit stays as a long-term idea, to pursue as the project grows.

### 8. Chain of custody and validity in court

| Item | Status |
|---|---|
| MD5, SHA-1 and SHA-256 of every file, of the image and of the device | ✅ |
| `guara-manifesto.json` manifest in every recovery | ✅ |
| HTML report with case, examiner, source, partitions, summary and hashes | ✅ |
| Verification of an E01/Ex01 image against the stored hash | ✅ |
| **Audit trail** of every examiner action (who, when, what), chained by hash, with `guara audit --verify` | ✅ |
| **Digital signatures** (Ed25519) of manifests, reports and any file; `guara sign recovery` checks a whole recovery folder | ✅ |
| Trusted timestamp (RFC 3161) on signatures | 🔜 |
| **Reproducibility:** replay the recorded procedure and get the same result | 🔜 |
| PDF report (A4, built in) | ✅ |
| **External validation:** NIST CFTT (Computer Forensic Tool Testing) tests and publication of the results | 🔭 |
| Signed binaries (Windows and macOS) | 🔜 |

External validation is what makes a report accepted. Without it, Guará is good for triage, recovery and support, and the examiner must check the results by other means.

### 9. Analysis

| Item | Status |
|---|---|
| Text search across the whole disk, UTF-8 and UTF-16, including free space | ✅ `guara search` |
| Duplicate detection | ✅ |
| Regular expression and keyword search in **file content** (Office, PDF, e-mail) | 🔜 |
| Known hashes: **NSRL** to ignore system files | 🔜 |
| Hash lists of illicit material (imported by the examiner) | 🔜 |
| Image detection by content (similarity, classification) | 🔭 |
| **OCR** on scanned images and PDFs | 🔭 |

### 10. Platform and distribution

| Item | Status |
|---|---|
| CLI and interface for Windows, Linux and macOS | ✅ |
| CI on three systems, automatic release by tag | ✅ |
| Tests with real reference images on Linux, macOS and Windows | ✅ |
| Update from inside the desktop app (signed packages, one click) | ✅ |
| GUI: privileged helper process instead of restarting the whole app as administrator | 🔜 |
| Signed and notarized binaries | 🔜 |
| Packages in package managers (winget, Homebrew, Flatpak) | 🔭 |

## Suggested next milestones

No dates, in the order that makes the most sense.

**1.4: trust and custody**
Trusted timestamps, XFS journal reading, APFS validation with real deletions.

**1.5: analysis**
NSRL and hash lists, search in file content, unified timeline.

**1.6: artifacts**
Windows registry, browsers, EVTX and the most common messaging apps, built on the SQLite reader.

**1.7: more structure**
ReFS and Storage Spaces (once there is a reference image), password-protected DMG, identity check of VHD, VHDX and QCOW2 parents, expanded fragmented carving.

**2.0: mobile devices and validation**
iTunes backups, ADB and Android backups; submission to the NIST CFTT tests.

## How to help

The most common bottleneck is a **reference image**: a ReFS volume, a Storage Spaces pool or an XFS with real deletion, preferably small and without personal data. If you have one, open an issue. See also [CONTRIBUTING](CONTRIBUTING.md).
