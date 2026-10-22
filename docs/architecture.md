# Architecture

[English](architecture.md) · [Português](pt-BR/arquitetura.md)

Guará is a Rust workspace with three parts: a core (`guara-core`), a terminal binary (`guara-cli`) and an interface (`apps/gui`) that is the same web page served in two ways.

```
crates/guara-core   library: everything that reads, interprets and recovers
crates/guara-cli    `guara` binary: commands and the local interface server
apps/gui/ui         HTML, CSS and JS of the interface
apps/gui/src-tauri  desktop shell (Tauri) that loads the same interface
```

## Flow of a scan

```
source (device or image)
   │  source / device         read-only access, bad sectors tolerated
   ▼
virtual disk                  (optional)
   │  assemble                joins E01/AFF4 images, RAID, LVM and encrypted volumes
   ▼
partitions and volumes
   │  partition, volume       MBR, GPT, volumes lost and found by signature
   ▼
two searches in parallel
   ├─ fs/*                    metadata: NTFS, FAT, exFAT, ext, HFS+, Btrfs, XFS, APFS
   └─ carve + signatures      file structure in free space and outside partitions
        + formats, jpegscan, defrag, mp4rebuild
   ▼
scan                          merges without duplicates and assigns confidence
   ▼
saved session                 resume, export, report
   ▼
recover                       copies with MD5, SHA-1 and SHA-256 and writes the manifest
```

## Core modules

### Access and assembly
| Module | Role |
|---|---|
| `source`, `device` | Open devices and images read-only; list disks |
| `image` | Raw image tolerant of bad sectors, with an error map |
| `ewf`, `aff4` | Read and write E01/Ex01 and AFF4 |
| `audit` | Hash-chained audit trail of every examiner action |
| `vdisk/` | Read QCOW2, VMDK, VHD, VHDX and DMG as the disk they contain |
| `assemble` | Builds the virtual disk: image, RAID, LVM and encrypted volume |
| `raid`, `raid_detect`, `lvm` | RAID reassembly (with automatic parameter discovery) and LVM2 |
| `crypt/` | BitLocker, LUKS and FileVault 2 |
| `reconnect` | Recognize the same disk that came back under another name |

### Interpretation
| Module | Role |
|---|---|
| `partition`, `volume` | Partition tables and volume detection |
| `fs/` | One reader per file system |
| `signatures`, `formats` | Format detectors based on structure |
| `carve`, `scan` | Signature scan and merging with the metadata |
| `jpegscan`, `defrag`, `mp4rebuild` | Fragmented JPEG and MP4 without `moov` |
| `meta`, `sqlite`, `thumb` | Metadata, SQLite reader, thumbnails |

### Results and forensics
| Module | Role |
|---|---|
| `recover` | Copies the files, checks the hashes, writes the manifest, resumes recovery |
| `hashing`, `search` | Hashes and text search across the whole disk |
| `report`, `explorer` | HTML report, CSV, explorer filters and groupings |
| `health`, `external` | SMART and external tools (`smartctl`) |

### Service
`service`, `http`, `settings`, `notify`, `owner`: the layer the interface uses (local API, settings, notifications, and writing with the right owner when running as administrator).

## Interface

The interface is a single page (`apps/gui/ui`). There are two ways to run it:

- **Desktop app (Tauri):** the window loads the page and talks to the core directly.
- **`guara ui`:** the binary starts a token-protected server on `127.0.0.1` and opens the browser.

Because the front end is the same, everything that exists in one exists in the other. See [interface.md](interface.md) for what it looks like.

<p align="center"><img src="img/explorer-light-en.png" width="720" alt="The Guará explorer"></p>

## Design decisions

- **Read-only on the source, by construction.** All device access goes through `source::open_path`. Code review rejects any write to the source.
- **Refusing a destination on the same disk** as the source (Linux and macOS), to avoid overwriting what you want to recover.
- **Confidence per file.** Each detector declares how much it managed to verify; merging keeps the best evidence.
- **Own readers.** File systems, ciphers, SQLite and image containers are implemented in the core, to tolerate corrupted structures, which is exactly the use case.
- **Secrets out of the session.** Passwords are never written; at most the volume key, and only with `--remember-keys`.
- **Privileges.** Reading a raw disk requires administrator, but everything Guará writes (sessions, reports, recovered files) keeps the owner and permissions of the user who launched it.

## Tests

About a hundred tests, in `crates/guara-core/tests/` (one file per area: `ext4`, `xfs`, `apfs`, `bitlocker`, `raid`, `ewf`, `jpegscan`, `mp4rebuild`, `sqlite` and others).

- **Images generated on the fly** with `mtools`, `ntfs-3g`, `e2fsprogs` and the like (`scripts/make-fixtures.sh`).
- **Reference images** from external projects (cryptsetup, dfvfs, AFF4) downloaded by `scripts/fetch-testdata.sh` into `target/testdata`. Without them, the corresponding tests are skipped.
- The BitLocker tests take about 75 s because of key derivation.
- In CI the tests with real images run only on Linux.
