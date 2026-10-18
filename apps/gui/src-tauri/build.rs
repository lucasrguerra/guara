fn main() {
    #[cfg(windows)]
    {
        // Ler discos brutos no Windows exige administrador: o manifesto pede elevação (UAC).
        let manifest = include_str!("windows-app-manifest.xml");
        let attrs = tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new().app_manifest(manifest));
        tauri_build::try_build(attrs).expect("falha no tauri-build");
    }
    #[cfg(not(windows))]
    tauri_build::build();
}
