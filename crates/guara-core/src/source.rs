//! Acesso somente leitura a discos, partições e imagens.
//!
//! Nenhum tipo deste módulo expõe escrita: a origem nunca é modificada.

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// Origem de bytes legível por posição absoluta.
pub trait Source: Send + Sync {
    fn size(&self) -> u64;
    /// Lê até `buf.len()` bytes a partir de `offset`. Retorna 0 no fim.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

impl<T: Source + ?Sized> Source for Arc<T> {
    fn size(&self) -> u64 {
        (**self).size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read_at(offset, buf)
    }
}

/// Lê exatamente `buf.len()` bytes, ou falha com `UnexpectedEof`.
pub fn read_exact_at(src: &dyn Source, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let n = src.read_at(offset, buf)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "fim da origem",
            ));
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

/// Lê o máximo possível (pode devolver menos que `len` no fim da origem).
pub fn read_vec(src: &dyn Source, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; len];
    let mut got = 0;
    while got < len {
        let n = src.read_at(offset + got as u64, &mut v[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    v.truncate(got);
    Ok(v)
}

/// Arquivo de imagem ou dispositivo de blocos aberto somente para leitura.
pub struct FileSource {
    file: File,
    size: u64,
    /// Dispositivos brutos (Windows/macOS) exigem leituras alinhadas ao setor.
    align: u64,
}

impl FileSource {
    pub fn open(path: &Path) -> io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let is_device = is_device(&meta);
        let file = open_readonly(path, is_device)?;
        let size = if is_device {
            device_size(&file, path)?
        } else {
            meta.len()
        };
        let align = if is_device && cfg!(any(windows, target_os = "macos")) {
            4096
        } else {
            1
        };
        Ok(Self { file, size, align })
    }

    fn raw_read(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
        }
    }
}

impl Source for FileSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf.len().min((self.size - offset) as usize);
        if self.align <= 1 {
            return self.raw_read(offset, &mut buf[..want]);
        }
        // Leitura alinhada para dispositivos brutos.
        let a = self.align;
        let start = offset / a * a;
        let end = ((offset + want as u64).div_ceil(a) * a).min(self.size.div_ceil(a) * a);
        let mut tmp = vec![0u8; (end - start) as usize];
        let mut got = 0;
        while got < tmp.len() {
            let n = self.raw_read(start + got as u64, &mut tmp[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        let skip = (offset - start) as usize;
        if got <= skip {
            return Ok(0);
        }
        let n = (got - skip).min(want);
        buf[..n].copy_from_slice(&tmp[skip..skip + n]);
        Ok(n)
    }
}

/// Janela [start, start+len) de outra origem (uma partição, por exemplo).
pub struct SubSource {
    inner: Arc<dyn Source>,
    start: u64,
    len: u64,
}

impl SubSource {
    pub fn new(inner: Arc<dyn Source>, start: u64, len: u64) -> Self {
        let len = len.min(inner.size().saturating_sub(start));
        Self { inner, start, len }
    }
}

impl Source for SubSource {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len {
            return Ok(0);
        }
        let want = buf.len().min((self.len - offset) as usize);
        self.inner.read_at(self.start + offset, &mut buf[..want])
    }
}

/// Origem em memória (testes e dados pequenos).
pub struct MemSource(pub Vec<u8>);

impl Source for MemSource {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let off = offset as usize;
        if off >= self.0.len() {
            return Ok(0);
        }
        let n = buf.len().min(self.0.len() - off);
        buf[..n].copy_from_slice(&self.0[off..off + n]);
        Ok(n)
    }
}

fn is_device(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let ft = meta.file_type();
        ft.is_block_device() || ft.is_char_device()
    }
    #[cfg(windows)]
    {
        let _ = meta;
        false
    }
}

#[cfg(unix)]
fn open_readonly(path: &Path, _device: bool) -> io::Result<File> {
    std::fs::OpenOptions::new().read(true).open(path)
}

#[cfg(windows)]
fn open_readonly(path: &Path, _device: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE: não bloqueia outros processos.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(path)
}

#[cfg(unix)]
fn device_size(file: &File, _path: &Path) -> io::Result<u64> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
        const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;
        let fd = file.as_raw_fd();
        let (mut bs, mut bc) = (0u32, 0u64);
        // SAFETY: ioctls de leitura com ponteiros para variáveis locais válidas.
        let ok = unsafe {
            libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut bs) == 0
                && libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut bc) == 0
        };
        if ok {
            return Ok(bs as u64 * bc);
        }
    }
    use std::io::{Seek, SeekFrom};
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::End(0))
}

/// Abre um dispositivo Windows (`\\.\PhysicalDriveN`) e obtém o tamanho.
#[cfg(windows)]
pub fn open_windows_device(path: &Path) -> io::Result<FileSource> {
    let file = open_readonly(path, true)?;
    let size = device_size(&file, path)?;
    Ok(FileSource {
        file,
        size,
        align: 4096,
    })
}

#[cfg(windows)]
fn device_size(file: &File, _path: &Path) -> io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Ioctl::{GET_LENGTH_INFORMATION, IOCTL_DISK_GET_LENGTH_INFO};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    let mut info = GET_LENGTH_INFORMATION { Length: 0 };
    let mut ret = 0u32;
    // SAFETY: buffer de saída válido com o tamanho exato da estrutura.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle() as _,
            IOCTL_DISK_GET_LENGTH_INFO,
            std::ptr::null(),
            0,
            &mut info as *mut _ as *mut _,
            std::mem::size_of::<GET_LENGTH_INFORMATION>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.Length as u64)
}

/// Abre um caminho (imagem ou dispositivo) como origem somente leitura.
pub fn open_path(path: &Path) -> io::Result<Arc<dyn Source>> {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if s.starts_with(r"\\.\") {
            return Ok(Arc::new(open_windows_device(path)?));
        }
    }
    Ok(Arc::new(FileSource::open(path)?))
}

/// Identidade do conteúdo de um disco: SHA-256 do tamanho + primeiro e último MiB
/// (tabela de partições, cabeçalhos e backup). Serve para reconhecer o mesmo disco
/// quando ele volta com outro nome (`/dev/sda` virando `/dev/sdb`).
pub fn fingerprint(src: &dyn Source) -> Option<String> {
    use sha2::{Digest, Sha256};
    const MIB: u64 = 1024 * 1024;
    let size = src.size();
    let mut h = Sha256::new();
    h.update(size.to_le_bytes());
    let mut feed = |off: u64, len: u64| -> bool {
        let mut buf = vec![0u8; len as usize];
        let mut got = 0;
        while got < buf.len() {
            match src.read_at(off + got as u64, &mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(_) => return false,
            }
        }
        h.update(&buf[..got]);
        true
    };
    if !feed(0, size.min(MIB)) {
        return None;
    }
    if size > 2 * MIB && !feed(size - MIB, MIB) {
        return None;
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
