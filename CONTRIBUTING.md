# Contributing to Guará

[English](CONTRIBUTING.md) · [Português](CONTRIBUTING.pt-BR.md)

Thanks for helping! Golden rules:

1. **Never write to the source.** Any code that opens a device must use `source::open_path` (read-only). PRs that write to the source disk are rejected.
2. **Test with real data.** New detectors/readers need a test that recovers a known file bit for bit (see `crates/guara-core/tests/` and `scripts/make-fixtures.sh`).
3. **Be honest about confidence.** If the content cannot be validated, confidence is at most medium.

## Workflow

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

On Linux the tests build images with `mtools`, `ntfs-3g`, `e2fsprogs`, `exfatprogs`, `imagemagick`, `zip` and `python3`. Without these tools the image tests are skipped (set `GUARA_REQUIRE_FIXTURES=1` to make them fail).

For the external reference images (BitLocker, FileVault 2, dfvfs, AFF4), run `scripts/fetch-testdata.sh`; without them those tests are skipped. The BitLocker test takes about 75 s.

The desktop app signs its update packages: `tauri.conf.json` has `createUpdaterArtifacts` on, so a local `tauri build` needs `TAURI_SIGNING_PRIVATE_KEY` (any key made with `tauri signer generate`; the release key lives only in the repository secrets). For a quick local bundle, pass `--config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## Where to make changes

A map of the modules is in [docs/architecture.md](docs/architecture.md). What is missing and what is welcome is in the [ROADMAP](ROADMAP.md).

## Adding a file format

Simple detectors live in `crates/guara-core/src/signatures.rs`; the newer ones in `formats.rs`. Add the detector to `detect`, the first byte to `might_start`, the extension to `category_for_ext`/`matches_ext` and a real file to `scripts/make-fixtures.sh` with a bit-exactness test.

## Adding a file system

Create the reader in `crates/guara-core/src/fs/`, register it in `fs/mod.rs` and bring a test with a real or reference image. **We do not ship a reader without validation**: that is why ReFS and Storage Spaces do not exist yet. Small images without personal data are the best gift you can give the project.

## Documentation

The main version is in English and every file has a Portuguese translation (`*.pt-BR.md`, `docs/pt-BR/`). No em dashes or en dashes. When you change a feature, update both versions (English and Portuguese) of [docs/support.md](docs/support.md), the [CHANGELOG](CHANGELOG.md) and, if applicable, the [ROADMAP](ROADMAP.md).

## Commits

Short messages in the imperative (`fix reading of sparse runs in NTFS`). One logical change per PR.
