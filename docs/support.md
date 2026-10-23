# What Guará supports

[English](support.md) · [Português](pt-BR/suporte.md)

Matrix of what exists today (version 1.1.0). For what comes next, see the [ROADMAP](../ROADMAP.md).

## Partition tables

MBR and GPT. When the table fails, Guará searches for volumes by signature (NTFS, FAT, exFAT, ext, APFS, HFS+, BitLocker, LUKS) and also examines the **unpartitioned space**.

## File systems

"Metadata" means Guará reads the file system structure and returns **name, folder and dates**. Without it, recovery depends only on carving, which returns the content without a name.

| File system | Metadata | What is used | Limits |
|---|---|---|---|
| NTFS | ✅ | MFT, deleted records, creation, modification and access dates | Compressed and EFS files are skipped or marked as incomplete; attribute list handled partially |
| FAT12/16/32 | ✅ | Deleted entries, long names, dates | |
| exFAT | ✅ | Deleted entries, dates | Covered by a synthetic image; needs validation on more real media |
| ext2/3/4 | ✅ | Deleted inodes, old copies in the journal (ext4), free block map | |
| HFS+ | ✅ | Leftovers of catalog tree nodes, journal | |
| Btrfs | ✅ | Old versions of the trees; zlib, zstd and lzo extents | |
| XFS | 🟡 | Freed inodes, names of freed entries and free space per group (works on simulated deletion); content by carving | Tested with real kernel deletion in CI: the kernel wipes the inode and extents, so names and folders are lost and only carving recovers content. Journal reading is missing |
| APFS | 🟡 | Container, object map, file tree, snapshots, old nodes | No free-space map; deletion tested only in simulation |
| ReFS | ❌ | Only detected | Recovery through carving |
| Storage Spaces | ❌ | Not supported | |

In every case carving keeps running on the free space and the result is merged without duplicates.

## Encrypted volumes

Guará opens the volume with the **password or key you provide**, mounts a virtual disk and scans inside it. It does not crack passwords.

| Format | Accepted credentials |
|---|---|
| BitLocker | Password, recovery key (8 groups of 6 digits), `.BEK` file, clear key |
| LUKS1 and LUKS2 | Password, key file, volume key |
| FileVault 2 (Core Storage) | Password, recovery key, volume key |

Passwords are **never** written to the session file. With `--remember-keys` the session stores the already derived volume key, not the password. In the terminal, prefer `--ask-password` or the `GUARA_PASSWORD` variable over `--password`, which stays in the shell history.

## RAID and LVM

| Item | Support |
|---|---|
| RAID 0, 1, 4, 5, 6, 10 and linear from mdadm superblocks | ✅ including degraded (use `-` in `--member` for a missing disk) |
| RAID without metadata | ✅ `--raid` discovers level, stripe size, layout and order; `--raid-level`, `--raid-chunk-kb`, `--raid-layout` and `--raid-order` fix what you already know |
| LVM2 | ✅ linear, striped, mirrored and RAID |
| Storage Spaces | ❌ |

## Forensic images

| Format | Read | Write |
|---|---|---|
| Raw (dd, `.img`) | ✅ | ✅ with a bad sector error map |
| E01 and Ex01 (Expert Witness) | ✅ | ✅ with case metadata and segmentation |
| AFF4 | ✅ | ✅ |
| QCOW2 (QEMU), versions 2 and 3 | ✅ including compressed clusters (deflate, zstd) and backing files | ❌ |
| VMDK (VMware, VirtualBox) | ✅ sparse, `streamOptimized`, descriptors with flat/zero extents and snapshots (parent by name or, for VirtualBox, by UUID) | ❌ |
| VHD and VHDX (Hyper-V) | ✅ fixed, dynamic and differencing (follows the parent chain) | ❌ |
| DMG (Apple UDIF) | ✅ zero, raw, ADC, zlib, bzip2 and LZFSE chunks | ❌ |

Virtual disk containers are opened as the disk they hold, with no conversion. Images that cannot be read correctly are refused with the reason: encrypted QCOW2, QCOW2 whose backing file is not a local file, VMDK in SEsparse, and password-protected DMG. Differencing disks (VHD, VHDX, QCOW2 backing file, VMDK snapshot) read the parent for what the child did not write; the parent is found next to the child (VMDK: by name or UUID) and, for VMDK, its CID must match, otherwise the image is refused because the parent changed. The identity of VHD, VHDX and QCOW2 parents is not checked. The VHDX transaction log is not replayed (nothing is ever written to the image).

`guara verify` recomputes the MD5 of an E01 or Ex01 image and compares it with the one stored in it.

## Carving: formats recognized by structure

The detectors read headers, blocks and internal sizes to return the **exact size** and a **confidence**, instead of cutting at a fixed number of bytes.

| Family | Formats |
|---|---|
| Image | JPEG, PNG, GIF, BMP, TIFF, ICO, HEIC, WebP |
| Document | PDF, DOC/XLS/PPT (OLE), DOCX/XLSX/PPTX/ODF/EPUB (ZIP), RTF |
| Video and audio | MP4/MOV, AVI, WAV, MP3, Matroska/WebM, Ogg, MPEG-TS, FLV, ASF/WMV, MIDI |
| Compressed | ZIP, 7z, RAR, gzip, zstd, ar |
| Database and system | SQLite, Windows registry (regf), EVTX, PST, pcap/pcapng, EXE/DLL (PE), ELF, SWF |

### Fragmented files

- **JPEG:** reassembled from up to three fragments (`jpegscan.rs`, `defrag.rs`).
- **MP4/MOV without an index (`moov`):** the index is rebuilt from an intact video from the **same camera or drone**, passed with `--reference`. Works with H.264 and HEVC. Audio is not reconstructed.
- Other formats only come out whole if they are contiguous; otherwise they come out truncated with low confidence.

## Metadata and preview

| Type | What is read |
|---|---|
| Photos | EXIF: camera, lens, serial number, date, GPS |
| PDF | Author, program, dates |
| Office and OpenDocument | Author, last modified by, company |
| MP4/MOV | Recording date, duration, resolution |
| SQLite | Schema, tables, blobs, dates, deleted rows from the free space of pages |

The preview is read straight from the disk, without writing anything: image, video, audio, PDF, text, Office, ZIP and hex.

## Platforms

| | CLI | Interface | Tests with real images |
|---|---|---|---|
| Linux | ✅ | ✅ AppImage and deb | ✅ |
| Windows | ✅ | ✅ msi and exe | build and synthetic tests only |
| macOS | ✅ | ✅ dmg | build and synthetic tests only |

## What Guará does not do

- Recover **overwritten** data. On an SSD with TRIM the disk itself may zero deleted blocks.
- Unlock phones or bypass passwords and security locks.
- Replace a mobile device extraction tool.
- Produce a report with forensic validity by itself: it has not been through external validation (NIST CFTT) yet. See the [ROADMAP](../ROADMAP.md).
