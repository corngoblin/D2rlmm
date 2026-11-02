//! Bindings to [CascLib](https://github.com/ladislav-zezula/CascLib), the storage
//! reader behind Ladik's CASC Viewer, compiled from the vendored source in
//! `vendor/CascLib`.
//!
//! This is a thin layer: it owns handles and turns C returns into
//! [`std::io::Error`], and does nothing else. The policy on top — which files a
//! D2R installation cares about, where they go, what a manifest records — lives in
//! `cairn-core`.
//!
//! # Why the raw API and not a rebuild of the format
//!
//! CASC is not one format. A Battle.net install is TACT: `.build.info`, an index
//! file per data archive, an ENCODING manifest mapping content keys to encoding
//! keys, BLTE-encoded blobs, and a ROOT file that carries the names. A Steam
//! install since build 93236 is a *static* storage: `data/.build.config`, a 40-bit
//! file offset, no ENCODING manifest at all. CascLib 3.x reads both, so Cairn reads
//! both by using it rather than by growing a second and third reader of its own.
//!
//! # Layout
//!
//! `CASC_FIND_DATA` is mirrored field for field. The sizes that matter on this
//! platform: `MAX_PATH` is **1024** outside Windows (`CascPort.h`), not 260, so a
//! struct laid out for Windows reads garbage here — which is exactly the bug this
//! comment exists to prevent.

use std::ffi::{CStr, CString};
use std::fmt;
use std::os::raw::{c_char, c_void};
use std::path::Path;

/// Size of `char szFileName[]` inside `CASC_FIND_DATA`. 1024 off Windows.
pub const MAX_PATH: usize = 1024;
/// Size of a content/encoding key (an MD5 digest).
pub const MD5_HASH_SIZE: usize = 16;

/// `CASC_INVALID_ID` — no file data ID / locale flags for this entry.
pub const CASC_INVALID_ID: u32 = 0xFFFF_FFFF;

/// Open by virtual path (the default).
pub const CASC_OPEN_BY_NAME: u32 = 0x0000_0000;
/// Open by content key.
pub const CASC_OPEN_BY_CKEY: u32 = 0x0000_0002;

/// `CASC_NAME_TYPE` values.
pub const CASC_NAME_FULL: u32 = 0;
pub const CASC_NAME_DATA_ID: u32 = 1;
pub const CASC_NAME_CKEY: u32 = 2;
pub const CASC_NAME_EKEY: u32 = 3;

pub type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CascFindData {
    /// Full virtual path, e.g. `data:data/global/excel/...` on this platform.
    pub file_name: [c_char; MAX_PATH],
    /// MD5 of the decoded content — the thing a manifest records.
    pub ckey: [u8; MD5_HASH_SIZE],
    /// MD5 of the encoded (BLTE) blob.
    pub ekey: [u8; MD5_HASH_SIZE],
    pub tag_bit_mask: u64,
    pub file_size: u64,
    /// Points into `file_name`; recomputed in Rust rather than dereferenced.
    pub plain_name: *mut c_char,
    pub file_data_id: u32,
    pub locale_flags: u32,
    pub content_flags: u32,
    pub span_count: u32,
    /// A `DWORD bFileAvailable:1` bit-field — non-zero means present locally.
    pub file_available: u32,
    pub name_type: u32,
}

impl Default for CascFindData {
    fn default() -> Self {
        Self {
            file_name: [0; MAX_PATH],
            ckey: [0; MD5_HASH_SIZE],
            ekey: [0; MD5_HASH_SIZE],
            tag_bit_mask: 0,
            file_size: 0,
            plain_name: std::ptr::null_mut(),
            file_data_id: 0,
            locale_flags: 0,
            content_flags: 0,
            span_count: 0,
            file_available: 0,
            name_type: 0,
        }
    }
}

impl CascFindData {
    /// The virtual path as a `String`, or `None` if it is empty / not UTF-8.
    pub fn path(&self) -> Option<String> {
        let bytes: Vec<u8> = self
            .file_name
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as u8)
            .collect();
        if bytes.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
    }

    /// Whether CascLib had a real name for this entry, rather than a key or a
    /// file data ID standing in for one. Only named entries can be extracted to a
    /// path the game would ever ask for.
    pub fn has_real_name(&self) -> bool {
        self.name_type == CASC_NAME_FULL
    }

    /// Whether the bytes are on this disk. A CDN-only entry enumerates but cannot
    /// be read offline.
    pub fn is_available_locally(&self) -> bool {
        self.file_available != 0
    }

    /// The content key as lowercase hex.
    pub fn ckey_hex(&self) -> String {
        hex(&self.ckey)
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[link(name = "casclib", kind = "static")]
extern "C" {
    fn CascOpenStorage(sz_params: *const c_char, locale_mask: u32, storage: *mut Handle) -> bool;
    fn CascCloseStorage(storage: Handle) -> bool;
    fn CascGetStorageInfo(
        storage: Handle,
        info_class: u32,
        info: *mut c_void,
        info_size: usize,
        needed: *mut usize,
    ) -> bool;

    fn CascOpenFile(
        storage: Handle,
        file_name: *const c_void,
        locale_flags: u32,
        open_flags: u32,
        file: *mut Handle,
    ) -> bool;
    fn CascReadFile(file: Handle, buffer: *mut c_void, to_read: u32, read: *mut u32) -> bool;
    fn CascGetFileSize64(file: Handle, size: *mut u64) -> bool;
    fn CascCloseFile(file: Handle) -> bool;

    fn CascFindFirstFile(
        storage: Handle,
        mask: *const c_char,
        find_data: *mut CascFindData,
        list_file: *const c_char,
    ) -> Handle;
    fn CascFindNextFile(find: Handle, find_data: *mut CascFindData) -> bool;
    fn CascFindClose(find: Handle) -> bool;
}

/// An open CASC storage. Closes itself on drop.
pub struct Storage {
    handle: Handle,
    /// The path that actually opened, which may not be the one asked for.
    opened_at: std::path::PathBuf,
}

impl fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Storage")
            .field("opened_at", &self.opened_at)
            .finish()
    }
}

impl Storage {
    /// Opens the storage reachable from `path`.
    ///
    /// CascLib looks for `.build.info` / `.build.db` / `versions` / `data/.build.config`
    /// in the directory it is given and **walks up its parents**, but never down into
    /// a subdirectory. D2R's layout differs by store — Battle.net keeps `.build.info`
    /// beside the executable, a Steam static install keeps its metadata under
    /// `Data/` — so the folder itself and those two subdirectories are tried, and
    /// the first that opens wins.
    ///
    /// The parent directories are deliberately *not* tried here: CascLib already
    /// walks up, and a caller who picked the wrong folder should be told so rather
    /// than silently given whatever storage happens to live one level above.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let mut candidates = vec![path.to_path_buf()];
        for sub in ["Data", "data"] {
            let candidate = path.join(sub);
            if candidate.is_dir() && candidate != path {
                candidates.push(candidate);
            }
        }

        let mut attempts: Vec<String> = Vec::new();
        for candidate in &candidates {
            match Self::open_exact(candidate) {
                Ok(storage) => return Ok(storage),
                Err(e) => attempts.push(format!("{}: {e}", candidate.display())),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "no CASC storage found at {} ({})",
                path.display(),
                attempts.join("; ")
            ),
        ))
    }

    /// Opens exactly this path, with no fallback.
    pub fn open_exact(path: &Path) -> std::io::Result<Self> {
        let c_path = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("path contains a NUL byte: {}", path.display()),
            )
        })?;

        let mut handle: Handle = std::ptr::null_mut();
        // Locale mask 0 lets CascLib use the storage's own default, which for D2R
        // is enUS. Asking for every locale would pull in dubbed audio at open time.
        let ok = unsafe { CascOpenStorage(c_path.as_ptr(), 0, &mut handle) };
        if !ok || handle.is_null() {
            // The caller adds the path; the OS error alone keeps the message from
            // naming it twice.
            return Err(std::io::Error::other(format!(
                "CascOpenStorage failed ({})",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self {
            handle,
            opened_at: path.to_path_buf(),
        })
    }

    /// The directory whose metadata opened the storage.
    pub fn opened_at(&self) -> &Path {
        &self.opened_at
    }

    /// The product code name CascLib detected (`d2r`), when it has one.
    pub fn product(&self) -> Option<String> {
        const CASC_STORAGE_PRODUCT: u32 = 7;
        let mut buf = [0u8; 32];
        let mut needed = 0usize;
        let ok = unsafe {
            CascGetStorageInfo(
                self.handle,
                CASC_STORAGE_PRODUCT,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut needed,
            )
        };
        if !ok {
            return None;
        }
        let s = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
        let s = s.to_string_lossy().into_owned();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// Reads a whole file by virtual path. `None` means CascLib could not open it —
    /// a CDN-only entry, or a name it does not know.
    pub fn read_file(&self, virtual_path: &str) -> std::io::Result<Option<Vec<u8>>> {
        let c_name = CString::new(virtual_path.as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "virtual path contains a NUL byte",
            )
        })?;

        let mut file: Handle = std::ptr::null_mut();
        let opened = unsafe {
            CascOpenFile(
                self.handle,
                c_name.as_ptr() as *const c_void,
                0,
                CASC_OPEN_BY_NAME,
                &mut file,
            )
        };
        if !opened || file.is_null() {
            return Ok(None);
        }

        let mut size: u64 = 0;
        let sized = unsafe { CascGetFileSize64(file, &mut size) };
        if !sized {
            unsafe { CascCloseFile(file) };
            return Err(std::io::Error::other(format!(
                "CascGetFileSize failed for {virtual_path}"
            )));
        }

        let mut data = vec![0u8; size as usize];
        let mut read_total = 0usize;
        while read_total < data.len() {
            let want = ((data.len() - read_total).min(1024 * 1024)) as u32;
            let mut got: u32 = 0;
            let ok = unsafe {
                CascReadFile(
                    file,
                    data[read_total..].as_mut_ptr() as *mut c_void,
                    want,
                    &mut got,
                )
            };
            if !ok || got == 0 {
                unsafe { CascCloseFile(file) };
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "CascReadFile stalled at {read_total} of {} bytes for {virtual_path}",
                        data.len()
                    ),
                ));
            }
            read_total += got as usize;
        }
        unsafe { CascCloseFile(file) };
        Ok(Some(data))
    }

    /// Streams a file into `write` in 1 MiB chunks, never holding the whole thing
    /// in memory. D2R's largest assets are tens of megabytes and there are ~150k
    /// of them, so this is the path the extractor uses.
    ///
    /// Returns the number of bytes written, or `Ok(None)` when the entry cannot be
    /// opened.
    pub fn copy_file<W: std::io::Write + ?Sized>(
        &self,
        virtual_path: &str,
        out: &mut W,
    ) -> std::io::Result<Option<u64>> {
        let c_name = CString::new(virtual_path.as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "virtual path contains a NUL byte",
            )
        })?;

        let mut file: Handle = std::ptr::null_mut();
        let opened = unsafe {
            CascOpenFile(
                self.handle,
                c_name.as_ptr() as *const c_void,
                0,
                CASC_OPEN_BY_NAME,
                &mut file,
            )
        };
        if !opened || file.is_null() {
            return Ok(None);
        }

        let mut size: u64 = 0;
        if !unsafe { CascGetFileSize64(file, &mut size) } {
            unsafe { CascCloseFile(file) };
            return Err(std::io::Error::other(format!(
                "CascGetFileSize failed for {virtual_path}"
            )));
        }

        let mut buf = vec![0u8; 1024 * 1024];
        let mut total: u64 = 0;
        while total < size {
            let want = ((size - total) as usize).min(buf.len()) as u32;
            let mut got: u32 = 0;
            let ok = unsafe { CascReadFile(file, buf.as_mut_ptr() as *mut c_void, want, &mut got) };
            if !ok || got == 0 {
                unsafe { CascCloseFile(file) };
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("CascReadFile stalled at {total} of {size} bytes for {virtual_path}"),
                ));
            }
            out.write_all(&buf[..got as usize])?;
            total += got as u64;
        }
        unsafe { CascCloseFile(file) };
        Ok(Some(total))
    }

    /// Begins an enumeration of every entry whose virtual path matches `mask`.
    /// `"*"` walks the whole storage.
    pub fn find(&self, mask: &str) -> std::io::Result<Find> {
        let c_mask = CString::new(mask.as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "mask contains a NUL byte")
        })?;
        let mut data = CascFindData::default();
        let handle =
            unsafe { CascFindFirstFile(self.handle, c_mask.as_ptr(), &mut data, std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::other(format!(
                "CascFindFirstFile({mask}) found nothing: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Find {
            handle,
            current: Some(data),
            done: false,
        })
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CascCloseStorage(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// An in-progress storage enumeration.
pub struct Find {
    handle: Handle,
    current: Option<CascFindData>,
    done: bool,
}

impl Iterator for Find {
    type Item = CascFindData;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // The first entry came back from CascFindFirstFile.
        if let Some(first) = self.current.take() {
            return Some(first);
        }
        let mut data = CascFindData::default();
        if unsafe { CascFindNextFile(self.handle, &mut data) } {
            Some(data)
        } else {
            self.done = true;
            None
        }
    }
}

impl Drop for Find {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CascFindClose(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A struct laid out for Windows (`MAX_PATH` 260) silently misreads every field
    /// on Linux. These are the offsets the C compiler produces here.
    #[test]
    fn find_data_layout_matches_c() {
        assert_eq!(std::mem::size_of::<CascFindData>(), 1104);
        assert_eq!(std::mem::align_of::<CascFindData>(), 8);
        use std::mem::offset_of;
        assert_eq!(offset_of!(CascFindData, ckey), 1024);
        assert_eq!(offset_of!(CascFindData, ekey), 1040);
        assert_eq!(offset_of!(CascFindData, tag_bit_mask), 1056);
        assert_eq!(offset_of!(CascFindData, file_size), 1064);
        assert_eq!(offset_of!(CascFindData, plain_name), 1072);
        assert_eq!(offset_of!(CascFindData, file_data_id), 1080);
        assert_eq!(offset_of!(CascFindData, file_available), 1096);
        assert_eq!(offset_of!(CascFindData, name_type), 1100);
    }

    #[test]
    fn opening_a_directory_that_is_not_a_storage_errors_cleanly() {
        let dir = std::env::temp_dir().join("casc-sys-not-a-storage");
        std::fs::create_dir_all(&dir).unwrap();
        let err = Storage::open(&dir).expect_err("must not open an empty directory");
        let msg = err.to_string();
        // The message names the folder the caller asked for, not some parent.
        assert!(msg.contains("no CASC storage found at"), "{msg}");
        assert!(msg.contains("CascOpenStorage failed"), "{msg}");
        assert!(msg.contains(&dir.display().to_string()), "{msg}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// CascLib walks *up* from the path it is given. Cairn must not do that itself:
    /// pointing at the wrong folder has to be an error, not a silent redirect to
    /// whatever storage happens to live in the parent.
    #[test]
    fn a_parent_directory_storage_is_not_picked_up_implicitly() {
        let root = std::env::temp_dir().join("casc-sys-no-parent");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join(".build.info"), b"Branch!STRING:0\r\nus|eu\r\n").unwrap();

        let err = Storage::open(&child).expect_err("must not open the parent's storage");
        assert!(
            err.to_string().contains("no CASC storage found at"),
            "{err}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
