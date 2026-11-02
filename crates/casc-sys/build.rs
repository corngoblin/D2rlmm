//! Builds CascLib from the vendored sources in `vendor/CascLib` and links it
//! statically into this crate.
//!
//! The source list mirrors CascLib's own `CMakeLists.txt`. `src/overwatch/cmf-key.cpp`
//! is deliberately absent: it is not part of the library's source list upstream and
//! does not compile standalone (it expects types another translation unit defines).
//!
//! zlib: we link the system zlib rather than compiling CascLib's bundled copy. The
//! header set in `src/zlib` is still what `CascCommon.h` includes unless
//! `CASC_USE_SYSTEM_ZLIB` is defined, which is why that define is unconditional here.

use std::env;
use std::path::PathBuf;

const CPP_SOURCES: &[&str] = &[
    // common
    "common/Common.cpp",
    "common/Csv.cpp",
    "common/Directory.cpp",
    "common/FileStream.cpp",
    "common/FileTree.cpp",
    "common/ListFile.cpp",
    "common/Mime.cpp",
    "common/RootHandler.cpp",
    "common/Sockets.cpp",
    // hashes
    "hashes/md5.cpp",
    "hashes/sha1.cpp",
    // overwatch (aes + keys, needed by the BLTE path)
    "overwatch/aes.cpp",
    "overwatch/apm.cpp",
    "overwatch/cmf.cpp",
    // top level
    "CascDecompress.cpp",
    "CascDecrypt.cpp",
    "CascDumpData.cpp",
    "CascFiles.cpp",
    "CascFindFile.cpp",
    "CascIndexFiles.cpp",
    "CascOpenFile.cpp",
    "CascOpenStorage.cpp",
    "CascReadFile.cpp",
    "CascRootFile_Diablo3.cpp",
    "CascRootFile_Install.cpp",
    "CascRootFile_MNDX.cpp",
    "CascRootFile_OW.cpp",
    "CascRootFile_Text.cpp",
    "CascRootFile_TVFS.cpp",
    "CascRootFile_WoW.cpp",
];

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let casclib = manifest.join("../../vendor/CascLib");
    let src = casclib.join("src");

    if !src.join("CascLib.h").exists() {
        panic!(
            "vendored CascLib not found at {}. The repository ships it under vendor/CascLib.",
            casclib.display()
        );
    }

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .warnings(false)
        .include(&src)
        .include(src.join("zlib"))
        .define("CASCLIB_NO_AUTO_LINK_LIBRARY", None)
        .define("CASCLIB_NODEBUG", None)
        // Link the system zlib instead of compiling the bundled copy.
        .define("CASC_USE_SYSTEM_ZLIB", None)
        .flag_if_supported("-fPIC")
        .flag_if_supported("-Wno-deprecated-declarations");

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag("/std:c++14").flag("/EHsc").flag("/utf-8");
    } else {
        build.flag("-std=c++14");
    }

    for f in CPP_SOURCES {
        build.file(src.join(f));
    }
    // The one C file in the library (jenkins hash lookup).
    build.file(src.join("jenkins").join("lookup3.c"));

    build.compile("casclib");

    println!("cargo:rustc-link-lib=z");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", src.display());
}
