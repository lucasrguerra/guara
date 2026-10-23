# Graphical interface

[English](interface.md) · [Português](pt-BR/interface.md)

Guará has a desktop app (Tauri) and the same front end in the browser with `guara ui`. Everything below exists in both.

<p align="center"><img src="img/explorer-light-en.png" width="900" alt="The Guará explorer showing recovered photos grouped by month, with a sidebar to filter and a details panel on the right"></p>

## The explorer

After a scan, results open in a file-manager style explorer built for thousands of items:

- **Left:** navigation by type, real photo date (EXIF, by year and month), folder, camera, confidence and GPS, with an option to hide small files and duplicates.
- **Center:** thumbnail grid or list, grouped by day or month, with virtual scrolling. A filter chip shows what is active and clears it in one click.
- **Right:** details panel with the preview, folder, dates (taken, modified, created, accessed), camera, resolution and the file metadata. The badge shows the **confidence** of the file (high, medium or low).
- **Bottom bar:** selection count and the recovery buttons. Recovery always goes to another folder and writes a manifest with MD5, SHA-1 and SHA-256.

Arrow keys move through the files, and Space opens the full-screen quick look. Selection works like Finder or Explorer (Ctrl, Shift, keyboard).

## Themes

The switch at the bottom of the sidebar offers **Auto** (follows the system), **Light** and **Dark**. The choice is saved.

<p align="center"><img src="img/explorer-dark-en.png" width="900" alt="The same explorer in the dark theme"></p>

## Language

**Settings** (the gear at the bottom of the sidebar) has a **Language** selector: **Auto** follows the system language, or you can pick **Português** or **English**. The change applies right away. The same dialog controls automatic updates (below), notifications for long tasks and online maps for photos with GPS (off by default, because it downloads OpenStreetMap tiles).

## Updates

When the desktop app opens it asks GitHub for the release's `latest.json`. If a newer version exists, a card appears at the bottom right with the versions, **What's new** (the CHANGELOG notes) and **Update now**. The app downloads the package, verifies its signature against the public key built into the app, installs and reopens by itself. On Linux this works for the AppImage (the file is replaced in place); installs from `.deb` or `.rpm` only show the notice with the release address. **Settings > Automatic updates** turns the startup check off and has a **Check now** button. The check sends nothing besides what any browser sends to GitHub. `guara ui` in the browser does not update itself.

<p align="center"><img src="img/settings-language-en.png" width="420" alt="The Settings dialog with the Language selector set to English"></p>

Error messages that come from the core, and the terminal output, are still in Portuguese.

## About these screenshots

They show the real interface, not a mock-up, running on a synthetic test image: 72 generated JPEGs with fake EXIF (cameras, dates, GPS) that were deleted from a FAT32 image by `scripts/make-photo-fixture.py` and found by a normal `guara scan`. No real data appears. The device name in the sidebar was replaced with a generic one.
