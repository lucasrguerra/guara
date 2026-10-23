# Terminal reference

[English](cli.md) · [Português](pt-BR/cli.md)

Reading a raw disk requires privileges: administrator on Windows, `sudo` on Linux and macOS. Use `guara <command> --help` for the full list of options.

## Commands

| Command | What it does |
|---|---|
| `devices` | Lists disks and USB sticks, warns about TRIM and mounted volumes |
| `inspect <source>` | Partitions, unpartitioned space, encryption |
| `health <source>` | Disk health (SMART); requires `smartmontools` |
| `scan <source>` | Scan for deleted and lost files |
| `resume <session>` | Continues a paused scan |
| `recover <session> <destination>` | Recovers the files of a session |
| `image <source> <destination>` | Forensic image (raw, E01 or AFF4) |
| `verify <image>` | Checks an E01 or Ex01 image against the stored hash |
| `hash <source>` | MD5, SHA-1 and SHA-256 of the device |
| `search <source> <terms...>` | Text across the whole disk, UTF-8 and UTF-16 |
| `export <session> <csv>` | Spreadsheet of the findings |
| `report <session> <file>` | Printable report, HTML or PDF (by the file extension) |
| `sign keygen\|file\|verify\|recovery` | Ed25519 signatures of manifests, reports and files; checks a whole recovery folder |
| `audit` | Audit trail of every action (who, when, what); `--verify` checks the hash chain |
| `sessions` | Automatically saved sessions |
| `ui` | Interface in the browser (127.0.0.1, with token) |

`--json` exists on `devices`, `inspect`, `health` and `scan`.

## Scanning

```bash
sudo guara scan /dev/sdb --session s.json --list     # deep, with listing
sudo guara scan /dev/sdb --quick                     # metadata only, in seconds
guara scan disk.img --category image,document --min-confidence medium
guara scan disk.img --recover /mnt/other-disk/Recovered
```

Filters (valid on `scan`, `recover`, `export` and `report`):

| Option | Effect |
|---|---|
| `--category` | `image`, `document`, `video`, `audio`, `archive`, `database`, `other` (comma separated) |
| `--min-confidence` | `low`, `medium` or `high` |
| `--name` | only names and paths that contain the text |
| `--organize` | how to write: `folders` (original folders), `type`, `date` or `flat` |

## Recovery

```bash
guara recover s.json /mnt/other-disk/Recovered --category image
guara resume s.json
```

The destination must be on **another disk**. Every recovery writes `guara-manifesto.json` with the MD5, SHA-1 and SHA-256 of every file. If the recovery is interrupted, repeating the command skips what was already copied and checked.

## Encrypted volumes

```bash
sudo guara scan /dev/sdb1 --ask-password
sudo guara scan /dev/sdb1 --recovery-key 111111-222222-333333-444444-555555-666666-777777-888888
sudo guara scan /dev/sdb1 --key-file key.BEK
sudo guara scan /dev/sdb1 --volume-key 4f2a...
```

| Option | Use |
|---|---|
| `--ask-password` | Asks in the terminal, without echo. **Preferred** |
| `GUARA_PASSWORD` | Environment variable with the password |
| `--password` | Password on the command line (stays in the shell history) |
| `--recovery-key` | BitLocker numeric key |
| `--key-file` | BitLocker `.BEK` or LUKS keyfile |
| `--volume-key` | Volume key in hexadecimal (the one Guará shows when it opens a volume) |
| `--remember-keys` | Stores the keys of opened volumes in the session (never the password) |

## RAID and LVM

Pass the main disk and the other members with `--member`, repeated. Use `-` for a missing disk.

```bash
# With mdadm or LVM metadata: Guará reads everything by itself
guara inspect disk1.img --member disk2.img --member disk3.img

# Without metadata: discovers level, stripe size, layout and order
guara scan disk1.img --member disk2.img --member disk3.img --raid

# Fixing what you already know
guara scan disk1.img --member disk2.img --member - --raid \
  --raid-level 5 --raid-chunk-kb 64 --raid-layout left-symmetric --raid-order 0,1,2
```

## Forensic image

```bash
sudo guara image /dev/sdb /mnt/other-disk/sdb.img                      # raw, with error map
sudo guara image /dev/sdb /mnt/other-disk/sdb --format e01 \
  --case 2026-001 --examiner "Name" --evidence E1 --description "Laptop HDD"
sudo guara image /dev/sdb /mnt/other-disk/sdb --format aff4
guara verify /mnt/other-disk/sdb.E01
```

For E01: `--segment-mib` (size of each segment, default 1500, 0 for a single file) and `--compression` (0 to 9, default 1).

## Digital signatures

```bash
guara sign keygen                                # creates the key pair (never overwrites one)
guara sign file report.pdf                       # writes report.pdf.assinatura.json
guara sign verify report.pdf --trust 1a2b:3c4d:5e6f:7a8b:9c0d
guara sign recovery /mnt/other-disk/Recovered    # manifest signature and the hash of every file
```

With a key configured, `recover` and `report` sign the manifest and the report on their own. The signature covers the SHA-256, size, name, time and signer; anyone can check it with Guará or with `openssl` (it is plain Ed25519 over the message documented in `crates/guara-core/src/sign.rs`). A valid signature proves the file did not change since it was signed **by whoever holds the key**; to know whose key it is, compare the fingerprint with the one the examiner published outside the machine (`--trust`). The private key lives in `assinatura.key` in the configuration folder (or where `GUARA_SIGNING_KEY` points), mode 0600; keep a copy and consider keeping it on an encrypted volume. The time is the signing machine's clock, not a third-party timestamp.

## Audit trail

Every action that touches evidence is appended to `auditoria.jsonl` in the configuration folder (or to the file named by `GUARA_AUDIT_LOG`): opening a source (`inspect`, `scan`), `recover`, `image`, `hash`, `search`, `verify`, exports and sessions. Each line has the time (UTC), the system user (the one behind `sudo`), the host, the Guará version and the details; set `GUARA_EXAMINER` to also record the examiner's name. Passwords and keys are never written.

```bash
guara audit                  # lists the entries
guara audit --tail 20 --json
guara audit --verify         # checks the chain; exits with an error if it is broken
```

The lines are chained: each one carries the SHA-256 of the previous hash plus its own content, so editing, removing or reordering an entry is detected and `--verify` says which one. Cutting the end of the file cannot be detected by the file alone: keep the hash printed by `--verify` somewhere else (for example in the case notes).

## Video without an index (MP4 without moov)

Pass an **intact video from the same camera or drone** so Guará can rebuild the index:

```bash
guara scan card.img --reference GOOD_VIDEO.MP4 --category video
```

`--reference` can be repeated. Audio is not reconstructed.

## Text search

```bash
sudo guara search /dev/sdb "contract" "ssn" --case-sensitive --max 500
```

Searches in UTF-8 and UTF-16 (`--no-utf16` turns it off), including free space and areas outside partitions. Shows position, partition and context. `--max` limits the results (default 200).

## Report

```bash
guara report s.json report.html --case 2026-001 --examiner "Name" --hash-source
```

Includes case identification, source, partitions, summary, hashes, the audit trail status and the list of files. `--hash-source` adds the hash of the whole source. The format follows the extension: `report.pdf` writes an A4 PDF (standard fonts, nothing external), anything else writes HTML.
