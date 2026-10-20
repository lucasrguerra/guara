# Running Guará with Docker

[English](docker.md) · [Português](pt-BR/docker.md)

The image contains the `guara` command line tool and `smartctl`. It is useful in three cases:

1. **A Linux server or NAS without a desktop**, with the damaged disk plugged in.
2. **Forensic image files** (`.img`, E01, AFF4) on any system with Docker, because no device access is needed.
3. **A reproducible environment** for scripts and CI.

It is **not** the recommended way for most people. For a desktop, use the packages from [Releases](../../../releases).

## Limits

- **Raw devices only work on Linux hosts.** On Windows and macOS, Docker runs inside a virtual machine and does not pass physical disks through. Use an image file instead.
- **The graphical interface is not served from the container.** `guara ui` listens only on `127.0.0.1` and accepts only that `Host`, by design; opening it to the outside would weaken that protection, so it is not part of the image.
- Files written to a mounted folder belong to root unless you pass `--user` (possible when you work on image files).

## Build

```bash
docker build -t guara .
```

## Image files (any system)

Mount the folder with the image read-only and a separate folder for the results:

```bash
docker run --rm \
  -v "$PWD/images:/images:ro" \
  -v "$PWD/out:/out" \
  --user "$(id -u):$(id -g)" \
  guara scan /images/disk.img --session /out/s.json --list

docker run --rm \
  -v "$PWD/images:/images:ro" -v "$PWD/out:/out" \
  --user "$(id -u):$(id -g)" \
  guara recover /out/s.json /out/recovered
```

## A physical disk (Linux host)

Pass the device **read-only** with the `:r` flag. Docker then blocks writes to it at the kernel level, in addition to Guará never writing to the source:

```bash
docker run --rm \
  --device=/dev/sdb:/dev/sdb:r \
  -v /mnt/other-disk/out:/out \
  guara scan /dev/sdb --session /out/s.json --list
```

- The destination must be on **another disk** than `/dev/sdb`.
- Reading a raw device needs root inside the container, so do not pass `--user` here.
- To read SMART, `--device` is not enough on some systems; `--cap-add=SYS_RAWIO` is the usual extra.
- Avoid `--privileged`: it gives the container access to every device.

## Other commands

Everything in the [terminal reference](cli.md) works the same way, for example:

```bash
docker run --rm --device=/dev/sdb:/dev/sdb:r guara devices
docker run --rm --device=/dev/sdb:/dev/sdb:r guara health /dev/sdb
```
